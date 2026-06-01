# OpenLogi Architecture

OpenLogi is a six-crate Cargo workspace — a native, local-first alternative to
Logitech Options+ that controls Logitech mice over HID++. This document traces
the crate layering and the HID++ flow end to end. For the quick orientation an
AI agent needs before touching code, start with [`../CLAUDE.md`](../CLAUDE.md);
for the developer workflow see [`DEVELOPMENT.md`](DEVELOPMENT.md).

## 1. Overview

```
  openlogi-cli   -> openlogi-core, openlogi-hid, openlogi-assets
  openlogi-gui   -> openlogi-core, openlogi-hid, openlogi-hook, openlogi-assets
  openlogi-hid   -> openlogi-core
  openlogi-hook  -> openlogi-core
  openlogi-core     (foundation: no internal deps)
  openlogi-assets   (foundation: no internal deps)
```

Read `->` as "depends on".

`openlogi-core` is the foundation: types, TOML config, paths, and the
button/action catalog. It is deliberately I/O-free (except reading and writing
its own config file) and never depends on `hidpp`, `async-hid`, or any platform
API. To keep that boundary, core **mirrors** the few HID++ types it needs (for
example `DeviceKind`, which mirrors `hidpp::receiver::bolt::BoltDeviceKind`, and
`BatteryStatus`, which mirrors `hidpp 0.2`'s `BatteryStatus`) rather than
importing them from `hidpp` — so the protocol and platform crates never leak
their types upward into core.

`openlogi-hid` and `openlogi-hook` each depend on core and add one capability:
the HID++ protocol and the macOS input hook, respectively. `openlogi-assets` is
a second foundation crate — it has no internal dependencies (not even on core)
and provides the device asset registry schema plus HTTP fetch helpers for the
`assets.openlogi.org` host. `openlogi-cli` and `openlogi-gui` sit at the top and
compose the lower crates into the `openlogi` and `openlogi-gui` binaries: the
CLI builds on core, hid, and assets; the GUI builds on core, hid, hook, and
assets.

## 2. openlogi-core

The serializable data model and all device-agnostic logic.

- **Data model (`device.rs`):** `DeviceInventory`, `PairedDevice`, and
  `DeviceModelInfo`. `DeviceModelInfo::config_key()` returns
  `format!("{:x}{:04x}", extended_model_id, model_ids[0])` (e.g. `"2b042"`) —
  the join key that ties a physical device to its config entry and its asset
  registry record.
- **Config (`config.rs`):** TOML at the XDG path, carrying a `schema_version`
  (`SCHEMA_VERSION = 1`). A `DeviceConfig` holds `button_bindings`,
  `per_app_bindings`, `gesture_bindings`, and `dpi_presets`;
  `effective_bindings` overlays the active app's per-app map on top of the
  device defaults. Saves are atomic (write-temp-then-rename) and `0600` on
  Unix. A schema-version gate guards migration.
- **Bindings (`binding.rs`):** `ButtonId`, the `Action` catalog (37 built-in
  actions), action `Category`, `detect_swipe`, and `GestureDirection`.
  `Action::execute` synthesises the OS event on macOS — `CGEventPost` for
  keys, clicks, and scroll; the Dock SPI `CoreDockSendNotification` for
  Mission Control / App Exposé / Show Desktop / Launchpad. Device-side
  actions (DPI cycle, SmartShift) carry no CGEvent and are deferred to the
  hook/HID layer.
- **Paths (`paths.rs`):** XDG base directories on every OS; the config file
  lives at `~/.config/openlogi/config.toml`.

See [`CONFIGURATION.md`](CONFIGURATION.md) for the on-disk file format.

## 3. openlogi-hid — the HID++ flow

This is the central crate. It re-implements the HID++ feature wrappers OpenLogi
needs on top of the `hidpp` and `async-hid` libraries. Read it bottom-up.

### 3.1 Transport (`transport.rs`)

`RawHidChannel` adapts `async-hid` to the byte channel `hidpp` expects.
Enumeration pre-filters HID nodes to the Logitech vendor id (`0x046d`) and the
HID++ long-report usage page / usage id (`0xff00` / `0x0002`), so non-HID++
interfaces are dropped before any channel is opened.
`supports_short_long_hidpp()` is hardcoded to `Some((true, true))` to avoid
the report-descriptor inspection path, which is Linux-only in the upstream
library.

### 3.2 Route (`route.rs`) — the addressing seam

A controllable device is reached one of two ways:

- `DeviceRoute::Bolt { receiver_uid, slot }` — paired to a Logi Bolt receiver,
  addressed through the receiver's channel at a pairing slot (1..=6).
- `DeviceRoute::Direct { vendor_id, product_id }` — attached straight to the
  host over USB cable or Bluetooth, addressed on its own channel at the HID++
  self-index `DIRECT_DEVICE_INDEX = 0xff`.

`open_route_channel` is the **single place** the Bolt-vs-direct branch lives;
both the write path and the capture session go through it. For a direct route
it pre-filters candidates on vendor/product id before paying the ~100 ms
channel-open cost — otherwise, on a host that also has a Bolt receiver, every
direct write would needlessly open the receiver's channel first.

### 3.3 Feature wrappers — and the `hidpp 0.2` registry workaround

`hidpp 0.2` ships no typed wrappers for the features OpenLogi drives, so each is
re-implemented here:

| Feature | ID | File | Purpose |
|---|---|---|---|
| AdjustableDpi | `0x2201` | `adjustable_dpi.rs` | read/set sensor DPI |
| SmartShift Enhanced | `0x2111` | `smartshift.rs` | ratchet ↔ free-spin |
| ReprogControlsV4 | `0x1b04` | `reprog_controls.rs` | divert + decode buttons |
| Thumbwheel | `0x2150` | `thumbwheel.rs` | divert the MX thumb wheel |

**The feature-resolution workaround (critical).** `hidpp 0.2`'s central feature
registry reports `versions: &[]` for the features OpenLogi cares about, so a
`get_feature::<F>()` keyed by the wrapper's `TypeId` returns `None`.
`open_feature` (in `write.rs`) works around this:
it asks the device's *root* feature for the index of a feature ID
(`device.root().get_feature(F::ID)`, which returns the assigned index
unconditionally) and then attaches the typed wrapper to that index with
`device.add_feature::<F>(info.index)`. It deliberately bypasses
`enumerate_features()`. Every self-implemented feature above goes through this
path.

**Two device-specific traps the wrappers encode:**

- **SmartShift `0x2111` shifts its call table** versus the older `0x2110`:
  `getStatus` is function `1` and `setStatus` is function `2` (on `0x2110`
  they are `0` and `1`). Calling `0x2110`'s function IDs against a `0x2111`
  device hits the wrong functions and the device silently keeps its old state.
- **AdjustableDpi `0x2201`** uses Short messages: function `0` = getSensorCount,
  `2` = getSensorDpi, `3` = setSensorDpi.

### 3.4 Inventory (`inventory.rs`)

`enumerate()` builds the device list. For each Bolt receiver it merges **two**
data sources so it sees both awake and sleeping devices:

1. device-arrival events, which report the product ids currently online, and
2. the receiver's per-slot pairing register, which lists every paired device
   even if it is asleep.

`probe_direct` handles BT/wired devices and carries a **phantom-device guard**:
a candidate is only accepted as a real mouse if it answers for a battery *or* a
control feature (`PERIPHERAL_FEATURE_IDS` = `0x2201` / `0x2202` / `0x1b04`).
This stops a Bolt receiver's secondary HID interface from being mistaken for a
mouse — **do not remove it.** (`MAX_BOLT_SLOTS = 6`; arrival events are
drained for ~1500 ms.)

### 3.5 Write (`write.rs`)

`set_dpi` and `toggle_smartshift` resolve a route to a channel (via
`with_route`/`open_route_channel`), open the feature, and issue the write.
**DPI writes read back and verify:** a mismatch logs a warning but still
returns `Ok`, because the request did reach the device. `SharedChannel` lets
the capture session reuse one open channel instead of paying the channel-open
cost per write.

### 3.6 Capture (`gesture.rs`)

`run_capture_session` is the long-lived session behind gestures and remapped
buttons. It holds **one** channel open, diverts the gesture button (raw-XY),
the DPI/ModeShift buttons, and — only when its click is bound — the thumb
wheel, then runs a single message listener that decodes events into
`CapturedInput` (gesture, button press, scroll). A swipe is committed
mid-gesture, as soon as the hold passes the ~160 ms gate
(`GESTURE_HOLD_FOR_SWIPE`). On shutdown it restores every diverted control to
its native behaviour.

## 4. openlogi-hook

The system input hook. On macOS it installs a `CGEventTap` on a dedicated
`CFRunLoop` thread and exposes `Hook::start` / `Hook::stop`, the Accessibility
helpers `has_accessibility` / `prompt_accessibility`, and `frontmost_bundle_id`
for the per-app overlay. Each event surfaces as a `MouseEvent`, and the
callback returns an `EventDisposition` — `PassThrough` to let the OS deliver the
event, or `Suppress` to swallow it (used when a button is remapped). The tap
only observes the standard buttons 0–4.

**The callback runs on the tap's background thread**, not the GPUI main thread,
and must return quickly — blocking it stalls system-wide input.

On non-macOS targets `Hook` is uninhabited (an `Infallible` field), so it can
never be constructed and `start` only ever returns `HookError::Unsupported`.
The crate still compiles cleanly on every target.

## 5. openlogi-assets

The device-render asset layer. It defines the registry schema (`index`,
`manifest`, `metadata`) and an `AssetClient` (in `http.rs`) that fetches device
renders from `assets.openlogi.org`, verifies each download against its `sha256`,
and caches it on disk. The crate is deliberately I/O-light — parsing, HTTP, and
hashing only, with no opinion on where files land. Two consumers: the CLI's
bulk `assets sync` at packaging time, and the GUI's per-device fetch at startup.

## 6. openlogi-cli

`run()` initialises tracing (filtered by the `OPENLOGI_LOG` env var, default
`info`) and the clap parser, then dispatches the command tree — defaulting to
`list` when no subcommand is given. Commands live under `src/cmd/`:

- `list` — enumerate the connected Logitech HID++ devices.
- `assets sync` — pull the device-render registry from `assets.openlogi.org`.
- `diag` — real-device round-trip smoke tests against the HID++ write path
  (`features`, `dpi`).

See [`USAGE.md`](USAGE.md) for the full CLI reference.

## 7. openlogi-gui

The GPUI + gpui-component desktop app (the `openlogi-gui` binary).

- **`AppState` (`state.rs`)** is a GPUI global holding cross-view state: the
  current device, its bindings, DPI, and Accessibility status.
- **`hook_runtime.rs`** is the runtime bridge between background input events
  and actions. It mirrors the binding map out of `AppState`, installs the OS
  hook lazily, and dispatches both hook events and gesture events. The standard
  buttons Middle / Back / Forward are remapped; the primary left / right clicks
  always pass through, as does any button left on its own native click. Gesture
  buttons never reach the tap — the gesture watcher captures them over HID++.
- **Device-side actions** (DPI cycle, SmartShift) are routed to background HID
  writes (`write_dpi_in_background` / `toggle_smartshift_in_background`) so the
  event callback never blocks. Those writes take a `CaptureChannel` and reuse
  the capture session's open channel (a `SharedChannel`) rather than opening a
  new one per write.
- **Watchers (`watchers/`)** are the polling/event tasks: `accessibility`,
  `foreground_app` (drives the per-app overlay, polling at ~1 Hz), `gesture`,
  `inventory`, and `pairing`.

Other module groups under `src/` organise the views and platform glue
(`state/`, `mouse_model/`, `components/`, `platform/`, `asset/`, `windows/`).
User-facing strings go through the `tr!` i18n macro.
