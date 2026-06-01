# CLAUDE.md + ARCHITECTURE.md Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create `CLAUDE.md` (repo root) and `docs/ARCHITECTURE.md` so an AI agent and human contributors can orient on the OpenLogi codebase without reading every source file.

**Architecture:** `CLAUDE.md` is the concise (~120–150 line) AI-agent entry point: what the project is, how to build/test, a crate map, conventions, the non-obvious gotchas, stability contracts, and pointers. `docs/ARCHITECTURE.md` is the detailed companion that traces the 6-crate layering and the HID++ flow end to end. The two link to each other and to the existing `docs/{USAGE,CONFIGURATION,DEVELOPMENT}.md`, which are left untouched.

**Tech Stack:** Markdown only. Verification uses `grep`/`rg` and `cargo test` against the current source tree. No code changes.

---

## Ground rules for this plan

This is a **documentation-only** task. There is no compilable artifact and no unit test for prose, so the TDD loop is adapted:

- **"Write the failing test"** → run the exact `grep`/`rg`/`cargo` command that establishes the ground-truth fact (count, constant, path, command). Record the expected output. If the command's output does **not** match what a section is about to claim, the source has changed since this plan was written — **source wins**: re-read the file and correct the prose, do not copy the plan's stale value.
- **"Implement"** → write that doc section.
- **"Verify it passes"** → re-read the written section and confirm every fact in it matches the command output from the first step, and that every relative link resolves to a real file.
- **"Commit"** → small, frequent commits.

**Constraints (from the approved spec `docs/superpowers/specs/2026-06-01-documentation-claude-md-design.md`):**
- Do **not** modify `docs/USAGE.md`, `docs/CONFIGURATION.md`, or `docs/DEVELOPMENT.md`. Link to them.
- No code changes anywhere.
- English prose (consistency with the existing README/docs/comments).
- `CLAUDE.md` stays concise and points to detail; `ARCHITECTURE.md` carries the depth.

**Source-of-truth files** (re-read at write time; source wins over this plan if they disagree):
- `Cargo.toml`, `README.md`, `docs/DEVELOPMENT.md`
- `crates/openlogi-core/src/{lib,binding,config,device,paths}.rs`
- `crates/openlogi-hid/src/{lib,transport,route,inventory,write,adjustable_dpi,smartshift,reprog_controls,gesture,thumbwheel}.rs`
- `crates/openlogi-hook/src/{lib,macos}.rs`
- `crates/openlogi-assets/src/lib.rs`
- `crates/openlogi-cli/src/{lib,cmd/*}.rs`
- `crates/openlogi-gui/src/{main,state,hook_runtime,watchers/*}.rs`

**File structure produced by this plan:**
- Create: `CLAUDE.md` (repo root) — AI-agent orientation, ~120–150 lines.
- Create: `docs/ARCHITECTURE.md` — detailed 6-crate + HID++ flow reference.

`ARCHITECTURE.md` is built first (Tasks 1–9) because `CLAUDE.md` links to it; `CLAUDE.md` is assembled last (Tasks 10–11) so its "Pointers" section can reference real, already-written anchors.

---

## Task 1: Scaffold `docs/ARCHITECTURE.md` with the overview + crate dependency layering

**Files:**
- Create: `docs/ARCHITECTURE.md`
- Read for facts: `Cargo.toml`, each `crates/*/Cargo.toml`, `crates/openlogi-core/src/device.rs`

- [ ] **Step 1: Establish ground truth — workspace members and core's internal-dependency-free status**

Run:
```bash
rg -n 'members' -A8 Cargo.toml
echo '--- core internal deps (expect: none of openlogi-hid/hook/cli/gui/assets) ---'
rg -n 'openlogi-' crates/openlogi-core/Cargo.toml || echo 'no internal deps'
echo '--- who depends on core ---'
rg -n 'openlogi-core' crates/*/Cargo.toml
echo '--- does core mirror hidpp types instead of depending on hidpp? ---'
rg -n 'hidpp|async-hid' crates/openlogi-core/Cargo.toml || echo 'core does NOT depend on hidpp/async-hid'
```
Expected: six members (`openlogi-core`, `openlogi-hid`, `openlogi-assets`, `openlogi-cli`, `openlogi-gui`, `openlogi-hook`); `openlogi-core/Cargo.toml` prints `no internal deps`; `openlogi-hid`, `openlogi-hook`, `openlogi-cli`, `openlogi-gui` (and as applicable `openlogi-assets`) list `openlogi-core`; core prints `core does NOT depend on hidpp/async-hid`.

If any expectation fails, re-read the files and write what is actually true.

- [ ] **Step 2: Write the file header + Overview section**

Create `docs/ARCHITECTURE.md` with exactly this content:

```markdown
# OpenLogi Architecture

OpenLogi is a six-crate Cargo workspace — a native, local-first alternative to
Logitech Options+ that controls Logitech mice over HID++. This document traces
the crate layering and the HID++ flow end to end. For the quick orientation an
AI agent needs before touching code, start with [`../CLAUDE.md`](../CLAUDE.md);
for the developer workflow see [`DEVELOPMENT.md`](DEVELOPMENT.md).

## 1. Overview

```
        openlogi-cli      openlogi-gui        (top: the two binaries' logic)
              \               /  \
               \             /    \
        openlogi-hid   openlogi-hook   openlogi-assets
                    \      |      /
                     \     |     /
                      openlogi-core           (foundation: no internal deps)
```

`openlogi-core` is the foundation: types, TOML config, paths, and the
button/action catalog. It is deliberately I/O-free (except reading and writing
its own config file) and never depends on `hidpp`, `async-hid`, or any platform
API. To keep that boundary, core **mirrors** the few HID++ types it needs (for
example `DeviceKind`, `BatteryStatus`) rather than importing them from `hidpp`
— so the protocol and platform crates never leak their types upward into core.

`openlogi-hid`, `openlogi-hook`, and `openlogi-assets` each depend on core and
add one capability: the HID++ protocol, the macOS input hook, and the device
asset registry, respectively. `openlogi-cli` and `openlogi-gui` sit at the top
and compose the lower crates into the `openlogi` and `openlogi-gui` binaries.
```

- [ ] **Step 3: Verify the written section against source**

Run:
```bash
sed -n '1,40p' docs/ARCHITECTURE.md
test -f CLAUDE.md && echo 'CLAUDE.md exists' || echo 'CLAUDE.md not yet created (link will resolve after Task 11)'
test -f docs/DEVELOPMENT.md && echo 'DEVELOPMENT.md link OK'
```
Expected: the header and Overview print; `DEVELOPMENT.md link OK`. (`CLAUDE.md` is created in Task 11; the forward link is intentional.) Confirm the diagram's dependency arrows match Step 1's output.

- [ ] **Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: scaffold ARCHITECTURE.md overview + crate layering"
```

---

## Task 2: ARCHITECTURE.md — `openlogi-core` section

**Files:**
- Modify: `docs/ARCHITECTURE.md` (append section 2)
- Read for facts: `crates/openlogi-core/src/{device,config,binding,paths}.rs`

- [ ] **Step 1: Establish ground truth — config_key, schema version, action catalog count, config path**

Run:
```bash
rg -n 'fn config_key' -A4 crates/openlogi-core/src/device.rs
rg -n 'SCHEMA_VERSION' crates/openlogi-core/src/config.rs
echo '--- catalog length (expected 37 per README; source wins) ---'
cargo test -p openlogi-core catalog -- --nocolor 2>&1 | tail -20
rg -n 'effective_bindings|fn save|0o600|0600' crates/openlogi-core/src/config.rs
rg -n 'config|\.config' crates/openlogi-core/src/paths.rs | head
```
Expected: `config_key()` formats `{extended_model_id:x}{model_ids[0]:04x}`; `SCHEMA_VERSION` is `1`; the `catalog` tests pass (the catalog has 37 entries and excludes `CustomShortcut`); `config.rs` shows `effective_bindings`, an atomic `save`, and `0o600` on Unix; `paths.rs` resolves config to `~/.config/openlogi`.

**Action-count rule:** Write "37 built-in actions" only if `Action::catalog().len()` is 37 at write time. To confirm the exact number, add a throwaway check if the test output is not explicit:
```bash
rg -n 'Action::' crates/openlogi-core/src/binding.rs | rg -n 'catalog' -A60 | rg -c 'Action::'
```
If the count is not 37, write "the action catalog" (no hard number) instead — `catalog()` deliberately excludes `CustomShortcut`.

- [ ] **Step 2: Append section 2 to `docs/ARCHITECTURE.md`**

Append:
```markdown

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

See [Configuration](CONFIGURATION.md) for the on-disk file format.
```
If Step 1 showed the catalog count is not 37, replace "the `Action` catalog (37 built-in actions)" with "the `Action` catalog".

- [ ] **Step 3: Verify**

Run:
```bash
rg -n 'config_key|SCHEMA_VERSION = 1|37 built-in|effective_bindings|~/.config/openlogi' docs/ARCHITECTURE.md
test -f docs/CONFIGURATION.md && echo 'CONFIGURATION.md link OK'
```
Expected: the claims print; `CONFIGURATION.md link OK`. Each printed claim must match Step 1's command output.

- [ ] **Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: ARCHITECTURE.md openlogi-core section"
```

---

## Task 3: ARCHITECTURE.md — `openlogi-hid`, part 1 (transport + route)

**Files:**
- Modify: `docs/ARCHITECTURE.md` (begin section 3)
- Read for facts: `crates/openlogi-hid/src/{transport,route}.rs`

- [ ] **Step 1: Establish ground truth — transport filters and the route seam**

Run:
```bash
rg -n 'LOGITECH_VID|0x046d|USAGE_PAGE|0xff00|0x0002|supports_short_long_hidpp' crates/openlogi-hid/src/transport.rs
rg -n 'DIRECT_DEVICE_INDEX|enum DeviceRoute|Bolt|Direct|fn open_route_channel|~100ms|100ms' crates/openlogi-hid/src/route.rs
```
Expected: `transport.rs` pre-filters Logitech VID `0x046d` + HID++ usage page `0xff00` / usage id `0x0002` and hardcodes `supports_short_long_hidpp() = Some((true, true))`; `route.rs` defines `DIRECT_DEVICE_INDEX = 0xff`, `DeviceRoute::{Bolt, Direct}`, and `open_route_channel` as the single branch point, with a VID/PID pre-filter on direct routes before the channel-open cost.

- [ ] **Step 2: Append the section 3 heading + transport + route subsections**

Append:
```markdown

## 3. openlogi-hid — the HID++ flow

This is the central crate. It re-implements the HID++ feature wrappers OpenLogi
needs on top of the `hidpp` and `async-hid` libraries. Read it bottom-up.

### 3.1 Transport (`transport.rs`)

`RawHidChannel` adapts `async-hid` to the byte channel `hidpp` expects.
Enumeration pre-filters HID nodes to the Logitech vendor id (`0x046d`) and the
HID++ long-report usage page / usage id (`0xff00` / `0x0002`), so non-HID++
interfaces are dropped before any channel is opened. `supports_short_long_hidpp()`
is hardcoded to `Some((true, true))` to avoid the report-descriptor inspection
path, which is Linux-only in the upstream library.

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
```

- [ ] **Step 3: Verify**

Run:
```bash
rg -n '0x046d|0xff00|0x0002|DIRECT_DEVICE_INDEX = 0xff|open_route_channel|~100 ms' docs/ARCHITECTURE.md
```
Expected: each constant and the seam description print. Confirm against Step 1.

- [ ] **Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: ARCHITECTURE.md HID++ transport + route"
```

---

## Task 4: ARCHITECTURE.md — `openlogi-hid`, part 2 (feature wrappers)

**Files:**
- Modify: `docs/ARCHITECTURE.md` (continue section 3)
- Read for facts: `crates/openlogi-hid/src/{adjustable_dpi,smartshift,reprog_controls,thumbwheel,write}.rs`

- [ ] **Step 1: Establish ground truth — feature IDs, function IDs, and the registry workaround**

Run:
```bash
rg -n 'FEATURE_ID|0x2201|FN_|FUNCTION_|getSensor|setSensor' crates/openlogi-hid/src/adjustable_dpi.rs
rg -n '0x2111|FUNCTION_GET_STATUS|FUNCTION_SET_STATUS|0x2110' crates/openlogi-hid/src/smartshift.rs
rg -n '0x1b04|0x2150' crates/openlogi-hid/src/reprog_controls.rs crates/openlogi-hid/src/thumbwheel.rs
rg -n 'fn open_feature|get_feature|add_feature|enumerate_features|root\(\)' crates/openlogi-hid/src/write.rs
```
Expected: AdjustableDpi `0x2201` (getSensorCount fn 0, getSensorDpi fn 2, setSensorDpi fn 3); SmartShift `0x2111` with `FUNCTION_GET_STATUS = 1` / `FUNCTION_SET_STATUS = 2` (shifted vs `0x2110`); ReprogControlsV4 `0x1b04`; Thumbwheel `0x2150`; `write::open_feature` resolves via `device.root().get_feature(ID)` + `add_feature`, **not** `enumerate_features`.

- [ ] **Step 2: Append the feature-wrappers subsection**

Append:
```markdown

### 3.3 Feature wrappers — and the `hidpp 0.2` registry workaround

`hidpp 0.2` ships no typed wrappers for the features OpenLogi drives, so each is
re-implemented here:

| Feature | ID | File | Purpose |
|---|---|---|---|
| AdjustableDpi | `0x2201` | `adjustable_dpi.rs` | read/set sensor DPI |
| SmartShift Enhanced | `0x2111` | `smartshift.rs` | ratchet ↔ free-spin |
| ReprogControlsV4 | `0x1b04` | `reprog_controls.rs` | divert + decode buttons |
| Thumbwheel | `0x2150` | `thumbwheel.rs` | divert the MX thumb wheel |

**The feature-resolution workaround (critical).** `hidpp 0.2`'s feature
registry is effectively empty for these IDs — its `enumerate_features()` reports
`versions: &[]`, so a later `get_feature::<F>()` keyed by `TypeId` returns
`None`. `write::open_feature` works around this: it asks the device's *root*
feature for the index of a feature ID (`device.root().get_feature(ID)`) and then
registers the typed wrapper with `device.add_feature::<F>()`. Every
self-implemented feature above goes through this path.

**Two device-specific traps the wrappers encode:**

- **SmartShift `0x2111` shifts its call table** versus the older `0x2110`:
  `getStatus` is function `1` and `setStatus` is function `2` (on `0x2110`
  they are `0` and `1`). Calling `0x2110`'s function IDs against a `0x2111`
  device hits the wrong functions and the device silently keeps its old state.
- **AdjustableDpi `0x2201`** uses Short messages: function `0` = getSensorCount,
  `2` = getSensorDpi, `3` = setSensorDpi.
```
If Step 1 shows different IDs, write what the source says.

- [ ] **Step 3: Verify**

Run:
```bash
rg -n '0x2201|0x2111|0x1b04|0x2150|root\(\).*get_feature|add_feature|enumerate_features|function .1|function .2' docs/ARCHITECTURE.md
```
Expected: the table IDs, the workaround description (root get_feature + add_feature, NOT enumerate_features), and the SmartShift function shift all print and match Step 1.

- [ ] **Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: ARCHITECTURE.md HID++ feature wrappers + registry workaround"
```

---

## Task 5: ARCHITECTURE.md — `openlogi-hid`, part 3 (inventory, write, capture)

**Files:**
- Modify: `docs/ARCHITECTURE.md` (finish section 3)
- Read for facts: `crates/openlogi-hid/src/{inventory,write,gesture}.rs`

- [ ] **Step 1: Establish ground truth — inventory merge, phantom guard, write verify, capture session**

Run:
```bash
rg -n 'fn enumerate|MAX_BOLT_SLOTS|ARRIVAL_DRAIN|probe_direct|PERIPHERAL_FEATURE_IDS|read_codename' crates/openlogi-hid/src/inventory.rs
rg -n 'fn set_dpi|fn toggle_smartshift|read.?back|verify|SharedChannel|with_route' crates/openlogi-hid/src/write.rs
rg -n 'fn run_capture_session|GESTURE_HOLD_FOR_SWIPE|160|CapturedInput|divert|restore' crates/openlogi-hid/src/gesture.rs
```
Expected: `inventory::enumerate` merges arrival events + the per-slot pairing register, `probe_direct` has a phantom-device guard keyed on `PERIPHERAL_FEATURE_IDS` (`0x2201`/`0x2202`/`0x1b04`), `MAX_BOLT_SLOTS = 6`, `ARRIVAL_DRAIN ≈ 1500ms`; `write` re-opens per call, reads back and verifies DPI, exposes `SharedChannel`; `gesture::run_capture_session` holds one channel, diverts controls, listens once, restores on shutdown, with a ~160 ms swipe-hold gate.

- [ ] **Step 2: Append inventory + write + capture subsections**

Append:
```markdown

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
mouse — **do not remove it.** (`MAX_BOLT_SLOTS = 6`; arrival events are drained
for ~1500 ms.)

### 3.5 Write (`write.rs`)

`set_dpi` and `toggle_smartshift` resolve a route to a channel (via
`open_route_channel`), open the feature, and issue the write. **DPI writes read
back and verify:** a mismatch logs a warning but still returns `Ok`, because the
request did reach the device. `SharedChannel` lets the capture session reuse one
open channel instead of paying the channel-open cost per write.

### 3.6 Capture (`gesture.rs`)

`run_capture_session` is the long-lived session behind gestures and remapped
buttons. It holds **one** channel open, diverts the gesture button (raw-XY),
the DPI/ModeShift buttons, and — only when its click is bound — the thumb wheel,
then runs a single message listener that decodes events into `CapturedInput`
(gesture, button press, scroll). A swipe is committed once the hold passes the
~160 ms gate, mid-swipe. On shutdown it restores every diverted control to its
native behaviour.
```

- [ ] **Step 3: Verify**

Run:
```bash
rg -n 'phantom-device guard|PERIPHERAL_FEATURE_IDS|read back and verify|SharedChannel|run_capture_session|160 ms|MAX_BOLT_SLOTS = 6' docs/ARCHITECTURE.md
```
Expected: all print and match Step 1. If `ARRIVAL_DRAIN` or the hold gate differs from the prose, correct the prose.

- [ ] **Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: ARCHITECTURE.md HID++ inventory, write, capture"
```

---

## Task 6: ARCHITECTURE.md — `openlogi-hook`

**Files:**
- Modify: `docs/ARCHITECTURE.md` (append section 4)
- Read for facts: `crates/openlogi-hook/src/{lib,macos}.rs`

- [ ] **Step 1: Establish ground truth**

Run:
```bash
rg -n 'fn start|fn stop|has_accessibility|prompt_accessibility|frontmost_bundle_id|MouseEvent|EventDisposition|PassThrough|Suppress|CGEventTap|CFRunLoop' crates/openlogi-hook/src/macos.rs
rg -n 'Infallible|Unsupported|cfg\(.*macos' crates/openlogi-hook/src/lib.rs
```
Expected: `macos.rs` runs a `CGEventTap` on a dedicated `CFRunLoop` thread and exposes `Hook::start/stop`, `has_accessibility`, `prompt_accessibility`, `frontmost_bundle_id`, plus `MouseEvent` and `EventDisposition::{PassThrough, Suppress}`; non-macOS `Hook` is uninhabited via `Infallible` so `start` returns only `HookError::Unsupported`.

- [ ] **Step 2: Append section 4**

Append:
```markdown

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
```

- [ ] **Step 3: Verify**

Run:
```bash
rg -n 'CGEventTap|CFRunLoop|PassThrough|Suppress|background thread|Infallible|HookError::Unsupported|buttons 0..4|buttons 0–4' docs/ARCHITECTURE.md
```
Expected: all present and matching Step 1.

- [ ] **Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: ARCHITECTURE.md openlogi-hook section"
```

---

## Task 7: ARCHITECTURE.md — `openlogi-assets` + `openlogi-cli`

**Files:**
- Modify: `docs/ARCHITECTURE.md` (append sections 5–6)
- Read for facts: `crates/openlogi-assets/src/lib.rs`, `crates/openlogi-cli/src/{lib,cmd/*}.rs`

- [ ] **Step 1: Establish ground truth**

Run:
```bash
rg -n 'index|manifest|metadata|AssetClient|sha256|fetch|assets.openlogi.org' crates/openlogi-assets/src/lib.rs | head -30
rg -n 'fn run|Command|list|diag|assets|Subcommand|OPENLOGI_LOG|default' crates/openlogi-cli/src/lib.rs
ls crates/openlogi-cli/src/cmd/
```
Expected: assets exposes a registry schema (`index` / `manifest` / `metadata`) and an `AssetClient` doing sha256-verified cached HTTP fetch from `assets.openlogi.org`; `cli::run` sets up tracing (`OPENLOGI_LOG`, default `info`) + clap and defaults to `list`; the `cmd/` directory contains `list`, `diag`, `assets`.

- [ ] **Step 2: Append sections 5 and 6**

Append:
```markdown

## 5. openlogi-assets

The device-render asset layer. It defines the registry schema (`index`,
`manifest`, `metadata`) and an `AssetClient` that fetches device renders from
`assets.openlogi.org`, verifying each download against its sha256 and caching
it on disk. Two consumers: the CLI's bulk `assets sync` at packaging time, and
the GUI's per-device fetch at runtime.

## 6. openlogi-cli

`run()` initialises tracing (filtered by the `OPENLOGI_LOG` env var, default
`info`) and the clap parser, then dispatches the command tree — defaulting to
`list` when no subcommand is given. Commands live under `src/cmd/`:

- `list` — enumerate connected/paired devices.
- `diag` — diagnostics (e.g. `features`, `dpi`) for inspecting a device.
- `assets` — sync the device-render registry.

See [Usage](USAGE.md) for the full CLI reference.
```
If `ls` shows different subcommands, write the actual set.

- [ ] **Step 3: Verify**

Run:
```bash
rg -n 'assets.openlogi.org|sha256|AssetClient|OPENLOGI_LOG|defaulting to .list|list|diag|assets' docs/ARCHITECTURE.md | head
test -f docs/USAGE.md && echo 'USAGE.md link OK'
```
Expected: claims print; `USAGE.md link OK`.

- [ ] **Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: ARCHITECTURE.md assets + cli sections"
```

---

## Task 8: ARCHITECTURE.md — `openlogi-gui`

**Files:**
- Modify: `docs/ARCHITECTURE.md` (append section 7)
- Read for facts: `crates/openlogi-gui/src/{main,state,hook_runtime}.rs`, `crates/openlogi-gui/src/watchers/mod.rs`

- [ ] **Step 1: Establish ground truth**

Run:
```bash
rg -n 'AppState|global|Global' crates/openlogi-gui/src/state.rs | head
rg -n 'hook|Middle|Back|Forward|pass.?through|SharedChannel|set_dpi|gesture' crates/openlogi-gui/src/hook_runtime.rs | head -30
rg -n 'accessibility|foreground_app|inventory|pairing|gesture|Hz|1000|interval' crates/openlogi-gui/src/watchers/mod.rs | head
ls crates/openlogi-gui/src/
```
Expected: `state.rs` defines an `AppState` GPUI global; `hook_runtime.rs` mirrors the binding map, remaps Middle/Back/Forward while L/R pass through, and routes device-side actions to background HID writes reusing the capture `SharedChannel`; `watchers/` holds accessibility, foreground_app (per-app overlay, ~1 Hz), inventory, pairing, gesture; the `src/` listing shows the module groups.

- [ ] **Step 2: Append section 7**

Append:
```markdown

## 7. openlogi-gui

The GPUI + gpui-component desktop app (the `openlogi-gui` binary).

- **`AppState` (`state.rs`)** is a GPUI global holding cross-view state: the
  current device, its bindings, DPI, and Accessibility status.
- **`hook_runtime.rs`** is the bridge between the input layers and actions. It
  mirrors the binding map out of `AppState`, installs the hook lazily, and
  dispatches both hook events and gesture events. The standard mouse buttons
  Middle / Back / Forward are remapped; left and right always pass through.
  Device-side actions (DPI cycle, SmartShift) are routed to background HID
  writes that reuse the capture session's `SharedChannel`.
- **Watchers (`watchers/`)** are the polling/event tasks: `accessibility`,
  `foreground_app` (drives the per-app overlay, polling at ~1 Hz), `inventory`,
  `pairing`, and `gesture`.

Other module groups under `src/` organise the views and platform glue
(`state/`, `mouse_model/`, `components/`, `platform/`, `asset/`, `windows/`).
Strings go through the `tr!` i18n macro.
```
If `ls` shows different module groups, write the actual set.

- [ ] **Step 3: Verify**

Run:
```bash
rg -n 'AppState|hook_runtime|Middle / Back / Forward|pass through|SharedChannel|foreground_app|1 Hz|tr!' docs/ARCHITECTURE.md
```
Expected: all present and matching Step 1.

- [ ] **Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: ARCHITECTURE.md openlogi-gui section"
```

---

## Task 9: ARCHITECTURE.md — end-to-end flows

**Files:**
- Modify: `docs/ARCHITECTURE.md` (append section 8)
- Read for facts: `crates/openlogi-gui/src/{hook_runtime,watchers/inventory.rs}`, `crates/openlogi-hid/src/write.rs`, `crates/openlogi-core/src/binding.rs`

- [ ] **Step 1: Establish ground truth — confirm the three flows cross the crates as described**

Run:
```bash
rg -n 'enumerate|inventory|main thread|spawn|block' crates/openlogi-gui/src/watchers/inventory.rs | head
rg -n 'Action::execute|Suppress|PassThrough' crates/openlogi-gui/src/hook_runtime.rs | head
rg -n 'fn set_dpi|fn set_dpi_on|verify|read.?back' crates/openlogi-hid/src/write.rs
```
Expected: inventory is gathered on startup (the inventory watcher / startup path), hook_runtime calls `Action::execute` or returns `Suppress`, and `write::set_dpi`/`set_dpi_on` performs the `0x2201` write + read-back verify. If the GUI gathers inventory off the main thread, adjust the first flow's wording accordingly.

- [ ] **Step 2: Append section 8**

Append:
```markdown

## 8. End-to-end flows

Three walkthroughs that cross crate boundaries.

**Startup & inventory.** On launch the GUI gathers the HID++ inventory
(`openlogi-hid::enumerate` — merging Bolt arrival events, the pairing register,
and direct probes), builds the device list, and resolves each device's assets
(`openlogi-assets`) and current DPI/SmartShift state. The result populates
`AppState`.

**Side-button press → remap.** The `CGEventTap` in `openlogi-hook` fires its
callback on the tap thread → `hook_runtime` looks up the binding for that button
in its mirrored map → for a remapped button it runs `Action::execute`
(`openlogi-core`) to synthesise the replacement event and returns `Suppress`;
for an unbound or pass-through button (left/right) it returns `PassThrough` and
the OS delivers the original event.

**DPI change → device write.** Changing DPI in the GUI calls
`openlogi-hid::write::set_dpi` / `set_dpi_on` on a background task, which opens
the AdjustableDpi feature (`0x2201`), writes the new value, then reads it back
to verify — logging a warning on mismatch but reporting success because the
request reached the device.
```

- [ ] **Step 3: Verify**

Run:
```bash
rg -n 'Startup & inventory|Side-button press|DPI change|Action::execute|Suppress|PassThrough|set_dpi|read.? *back|0x2201' docs/ARCHITECTURE.md
```
Expected: the three flow headings and their key terms print and are consistent with the earlier sections (same function/feature names).

- [ ] **Step 4: Commit**

```bash
git add docs/ARCHITECTURE.md
git commit -m "docs: ARCHITECTURE.md end-to-end flows"
```

---

## Task 10: `CLAUDE.md` — sections 1–4 (what it is, build/test, crate map, conventions)

**Files:**
- Create: `CLAUDE.md`
- Read for facts: `docs/DEVELOPMENT.md`, `Cargo.toml`, `README.md`, each `crates/*/Cargo.toml`

- [ ] **Step 1: Establish ground truth — build commands, edition/MSRV, the binaries, unsafe locations**

Run:
```bash
rg -n 'cargo fmt|cargo clippy|cargo test|cargo run|devenv tasks run|OPENLOGI_LOG|Metal Toolchain|direnv reload' docs/DEVELOPMENT.md
rg -n 'edition|rust-version|version =' Cargo.toml | head
rg -n 'name =' crates/openlogi-cli/Cargo.toml crates/openlogi-gui/Cargo.toml
echo '--- unsafe locations (expect hook + core binding dock SPI) ---'
rg -rln 'unsafe' crates/*/src | head
```
Expected: the exact pre-commit commands (`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, or `devenv tasks run openlogi:check`); Edition 2024 / MSRV 1.85; the `openlogi` and `openlogi-gui` binaries; `unsafe` concentrated in `openlogi-hook` and `openlogi-core/src/binding.rs`.

- [ ] **Step 2: Create `CLAUDE.md` with sections 1–4**

Create `CLAUDE.md`:
```markdown
# CLAUDE.md

Orientation for AI agents working in this repository. Read this first, then
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the detailed design.

## 1. What this is

OpenLogi is a six-crate Rust workspace — a native, local-first alternative to
Logitech Options+ that controls Logitech mice over HID++ (via a Logi Bolt
receiver, a direct Bluetooth link, or a USB cable). macOS is the supported
platform today; Linux and Windows are stubs that compile but do nothing. The
workspace builds two binaries: `openlogi` (CLI) and `openlogi-gui` (a GPUI
desktop app).

## 2. Build / run / test

```sh
cargo run -p openlogi --release -- list     # CLI
cargo run -p openlogi-gui --release         # desktop app
```

**Pre-commit gate — all of these must pass before committing:**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

(Equivalent: `devenv tasks run openlogi:check`.) Notes: GPUI needs Xcode 16+
with the Metal Toolchain component; after editing `devenv.nix`, run
`direnv reload`. Logging is controlled by the `OPENLOGI_LOG` env filter
(default `info`). Full workflow and packaging: [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md).

## 3. Crate map

Dependency direction: `openlogi-core` is the foundation (no internal deps);
`openlogi-hid`, `openlogi-hook`, and `openlogi-assets` depend on core;
`openlogi-cli` and `openlogi-gui` sit on top.

| Crate | Role |
|---|---|
| `openlogi-core` | Types, TOML config, paths, button/action catalog. I/O-free except its own config file. No `hidpp`/`async-hid`/platform APIs. |
| `openlogi-hid` | HID++ over `hidpp` + `async-hid`: enumerate, DPI (`0x2201`), SmartShift (`0x2111`), control capture (`0x1b04`/`0x2150`). |
| `openlogi-hook` | macOS `CGEventTap` mouse hook + Accessibility + frontmost-app detection. Stub elsewhere. |
| `openlogi-assets` | Device-render registry schema + cached, sha256-verified HTTP fetch from `assets.openlogi.org`. |
| `openlogi-cli` | CLI command tree (`list` / `diag` / `assets`) + `run()`. |
| `openlogi-gui` | The `openlogi-gui` binary — GPUI + gpui-component desktop app. |

## 4. Conventions

- Rust Edition 2024, MSRV 1.85.
- Workspace lints; clippy is run with `-D warnings` (warnings fail the build).
- `unsafe` is confined to `openlogi-hook` (CGEventTap / FFI) and the dock SPI in
  `openlogi-core/src/binding.rs`; every `unsafe` block and lint `allow` carries
  a `reason`.
- Tracing via the `tracing` crate; level set through `OPENLOGI_LOG`.
```
If Step 1's command set differs (e.g. devenv task renamed), write the actual commands.

- [ ] **Step 3: Verify the commands are copied verbatim from DEVELOPMENT.md**

Run:
```bash
for cmd in 'cargo fmt --all -- --check' 'cargo clippy --workspace --all-targets -- -D warnings' 'cargo test --workspace' 'devenv tasks run openlogi:check'; do
  grep -qF "$cmd" docs/DEVELOPMENT.md && grep -qF "$cmd" CLAUDE.md && echo "OK: $cmd" || echo "MISMATCH: $cmd"
done
test -f docs/ARCHITECTURE.md && echo 'ARCHITECTURE.md link OK'
test -f docs/DEVELOPMENT.md && echo 'DEVELOPMENT.md link OK'
```
Expected: four `OK:` lines and both link-OK lines. Any `MISMATCH` means re-copy the command exactly from `docs/DEVELOPMENT.md`.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add CLAUDE.md (what it is, build/test, crate map, conventions)"
```

---

## Task 11: `CLAUDE.md` — sections 5–7 (gotchas, stability contracts, pointers) + final review

**Files:**
- Modify: `CLAUDE.md` (append sections 5–7)
- Read for facts: `crates/openlogi-hid/src/{write,smartshift,route,inventory}.rs`, `crates/openlogi-core/src/{binding,config}.rs`

- [ ] **Step 1: Re-confirm every gotcha and the stability contract against source**

Run:
```bash
rg -n 'enumerate_features|get_feature|add_feature|root\(\)' crates/openlogi-hid/src/write.rs
rg -n 'FUNCTION_GET_STATUS = 1|FUNCTION_SET_STATUS = 2' crates/openlogi-hid/src/smartshift.rs
rg -n 'DIRECT_DEVICE_INDEX = 0xff|PERIPHERAL_FEATURE_IDS' crates/openlogi-hid/src/route.rs crates/openlogi-hid/src/inventory.rs
rg -n 'fn execute|stub|log|warn|trace' crates/openlogi-core/src/binding.rs | head
rg -n 'SCHEMA_VERSION|serde|rename|tag' crates/openlogi-core/src/{binding,config}.rs | head
rg -n 'background thread|callback' crates/openlogi-hook/src/macos.rs | head
echo '--- confirm Action::execute has no test ---'
rg -n 'fn .*execute' crates/openlogi-core/src/binding.rs
rg -n 'execute' crates/openlogi-core/src/binding.rs | rg -n 'test' || echo 'no test references execute (expected)'
```
Expected: feature resolution uses root `get_feature` + `add_feature` (not `enumerate_features`); SmartShift `0x2111` getStatus=1/setStatus=2; `DIRECT_DEVICE_INDEX = 0xff` with the `PERIPHERAL_FEATURE_IDS` phantom guard; `Action::execute` has no test; some media-key actions log instead of posting; the hook callback runs on a background thread; `Action`/`ButtonId`/`GestureDirection` names are the serde schema gated by `SCHEMA_VERSION`.

- [ ] **Step 2: Append sections 5–7 to `CLAUDE.md`**

Append:
```markdown

## 5. Critical gotchas

The non-obvious traps. Details and code references in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

- **`hidpp 0.2`'s feature registry is empty for our features.** Resolve a
  feature via `device.root().get_feature(ID)` + `device.add_feature::<F>()`,
  **not** `enumerate_features()` (its `versions: &[]` means a typed
  `get_feature::<F>()` returns `None`). Applies to `0x2201`, `0x2111`,
  `0x1b04`, `0x2150`. See `write::open_feature`.
- **SmartShift `0x2111` function IDs are shifted** vs the older `0x2110`:
  getStatus = function `1`, setStatus = function `2`. Using `0x2110`'s IDs
  silently no-ops the device.
- **Directly-attached devices are addressed at index `0xff`**
  (`DIRECT_DEVICE_INDEX`). `probe_direct`'s phantom-device guard (requires a
  battery *or* a control feature) keeps a Bolt receiver's secondary interface
  from being mistaken for a mouse — **don't remove it.**
- **`Action::execute` has no automated test** (it would have to intercept the
  OS event queue). Smoke-test it manually.
- **DPI writes read back and verify**; a mismatch logs a warning but still
  returns `Ok` (the request reached the device).
- **A few actions are stubs** — some media keys log their intended NX key
  instead of posting it.
- **The hook callback runs on a background thread**, not the GPUI thread, and
  must return quickly — blocking it stalls system-wide input.

## 6. Stability contracts

The variant *names* of `Action`, `ButtonId`, and `GestureDirection` are the
on-disk `config.toml` schema (serde external tagging). Appending new variants
is safe; renaming or removing one is a migration event that requires bumping
`Config::SCHEMA_VERSION`.

## 7. Pointers

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — detailed architecture and the HID++ flow.
- [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) — full dev workflow + packaging.
- [`docs/USAGE.md`](docs/USAGE.md) — the CLI reference.
- [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) — the config file format.
```
If Step 1 contradicts any bullet, correct that bullet to match source.

- [ ] **Step 3: Verify — facts, links, and length budget**

Run:
```bash
echo '--- all four pointer targets must exist ---'
for f in docs/ARCHITECTURE.md docs/DEVELOPMENT.md docs/USAGE.md docs/CONFIGURATION.md; do
  test -f "$f" && echo "OK: $f" || echo "MISSING: $f"
done
echo '--- line count (target ~120-150) ---'
wc -l CLAUDE.md
echo '--- gotcha keywords present ---'
rg -n 'enumerate_features|0x2111|0xff|no automated test|read back and verify|background thread|SCHEMA_VERSION' CLAUDE.md
```
Expected: four `OK:` lines, `CLAUDE.md` line count roughly 120–150 (modestly over is fine; trim padding if well above), and every gotcha keyword present. No `MISSING` lines.

- [ ] **Step 4: Final cross-file consistency pass**

Run:
```bash
echo '--- feature IDs must read identically in both files ---'
rg -n '0x2201|0x2111|0x1b04|0x2150' CLAUDE.md docs/ARCHITECTURE.md
echo '--- the registry workaround must be described the same way in both ---'
rg -n 'enumerate_features|add_feature|root\(\)' CLAUDE.md docs/ARCHITECTURE.md
echo '--- confirm existing docs were NOT modified ---'
git status --porcelain docs/USAGE.md docs/CONFIGURATION.md docs/DEVELOPMENT.md
```
Expected: the feature IDs and the workaround description are consistent across `CLAUDE.md` and `docs/ARCHITECTURE.md`; the last command prints **nothing** (the three existing docs are untouched). If any of the three shows as modified, revert it with `git checkout -- <file>`.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: complete CLAUDE.md (gotchas, stability contracts, pointers)"
```

---

## Self-Review (run by the plan author)

**1. Spec coverage** — every spec section maps to a task:
- Spec File A §1–7 (CLAUDE.md) → Tasks 10 (§1–4) + 11 (§5–7). ✓
- Spec File B §1 Overview → Task 1. ✓
- §2 openlogi-core → Task 2. ✓
- §3 openlogi-hid (transport/route/features/inventory/write/capture) → Tasks 3, 4, 5. ✓
- §4 openlogi-hook → Task 6. ✓
- §5 openlogi-assets + §6 openlogi-cli → Task 7. ✓
- §7 openlogi-gui → Task 8. ✓
- §8 end-to-end flows → Task 9. ✓
- Constraint "do not modify existing docs" → enforced in Task 11 Step 4. ✓
- Action-count rule (37 only if `catalog().len()` matches) → Task 2 Step 1. ✓

**2. Placeholder scan** — no `TBD`/`TODO`/"handle edge cases"/"similar to Task N". Every doc step contains the actual prose to write. ✓

**3. Type/fact consistency** — feature IDs (`0x2201`/`0x2111`/`0x1b04`/`0x2150`), `DIRECT_DEVICE_INDEX = 0xff`, `SCHEMA_VERSION`, the `root().get_feature` + `add_feature` workaround, and SmartShift function IDs (1/2) are stated identically in CLAUDE.md and ARCHITECTURE.md; Task 11 Step 4 explicitly cross-checks them. ✓

## Out of scope (YAGNI)

- No changes to existing docs, code, or behaviour.
- No Linux/Windows documentation beyond noting the stubs.
- No per-function API reference — rustdoc / module `//!` comments cover that.
