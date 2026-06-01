# Documentation: CLAUDE.md + ARCHITECTURE.md — Design

**Date:** 2026-06-01
**Status:** Approved, pending spec review

## Goal

Create two new documentation files so an AI agent (and human contributors) can
work effectively on the OpenLogi repository:

1. **`CLAUDE.md`** (repo root) — short, AI-agent-oriented orientation.
2. **`docs/ARCHITECTURE.md`** — detailed 6-crate architecture reference.

The primary objective is the `CLAUDE.md`; `ARCHITECTURE.md` is the detailed
companion it points to.

## Constraints & Decisions

- **Language:** English, for consistency with the existing README, docs, and
  code comments.
- **Style:** `CLAUDE.md` stays concise (~120–150 lines) and points to detailed
  docs rather than duplicating them. `ARCHITECTURE.md` carries the depth.
- **Coverage:** All six workspace crates, with depth proportional to
  complexity (HID++ flow and core get the most).
- **Do not modify** the existing `docs/USAGE.md`, `docs/CONFIGURATION.md`, or
  `docs/DEVELOPMENT.md`. They are accurate and serve their roles. `CLAUDE.md`
  links to them instead of repeating their content.
- **No code changes.** This is a documentation-only task. Content is derived
  from the current source — no new behavior is described.

## Source of Truth

All content is grounded in the current source tree (read during brainstorming):

- `crates/openlogi-core/src/{lib,binding,config,device,paths}.rs`
- `crates/openlogi-hid/src/{lib,transport,route,inventory,write,adjustable_dpi,smartshift,reprog_controls,gesture,thumbwheel}.rs`
- `crates/openlogi-hook/src/{lib,macos}.rs`
- `crates/openlogi-assets/src/lib.rs`
- `crates/openlogi-cli/src/{lib,cmd/*}.rs`
- `crates/openlogi-gui/src/{main,state,hook_runtime,watchers/*}.rs`
- `docs/DEVELOPMENT.md`, `Cargo.toml`, `README.md`

Where the spec and source disagree at write time, **source wins** — re-read the
file and correct the doc.

---

## File A: `CLAUDE.md` (repo root)

Target ~120–150 lines. Sections:

### 1. What this is
One paragraph: OpenLogi is a 6-crate Rust workspace — a native, local-first
alternative to Logitech Options+ that talks to Logitech mice over HID++ (Bolt
receiver / Bluetooth-direct / wired). macOS is the supported platform today;
Linux/Windows are stubs. Two binaries: `openlogi` (CLI) and `openlogi-gui`
(GPUI desktop app).

### 2. Build / run / test
Exact commands (from `docs/DEVELOPMENT.md` — do not invent):

- CLI: `cargo run -p openlogi --release -- list`
- GUI: `cargo run -p openlogi-gui --release`
- **Pre-commit gate (must pass before committing):**
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - or the equivalent `devenv tasks run openlogi:check`
- Notes: GPUI needs Xcode 16+ with the Metal Toolchain; after a `devenv.nix`
  change, run `direnv reload`. Logging is controlled by the `OPENLOGI_LOG`
  env filter (default `info`).

### 3. Crate map
A table of the six crates with a one-line role each, plus the dependency
direction (core is the foundation with no internal deps; hid/hook/cli/gui
depend on core; cli and gui sit at the top). Source: workspace `Cargo.toml`
member list + each crate's `Cargo.toml` deps.

| Crate | Role |
|---|---|
| `openlogi-core` | Types, TOML config, paths, button/action catalog. I/O-free except config file r/w. No `hidpp`/`async-hid`/platform APIs. |
| `openlogi-hid` | HID++ over `hidpp` + `async-hid`: enumerate, DPI (`0x2201`), SmartShift (`0x2111`), control capture (`0x1b04`/`0x2150`). |
| `openlogi-hook` | macOS `CGEventTap` mouse hook + Accessibility + frontmost-app detection. Stub elsewhere. |
| `openlogi-assets` | Device-render registry schema + cached HTTP fetch from assets.openlogi.org. |
| `openlogi-cli` | CLI command tree (`list` / `diag` / `assets`) + `run()`. |
| `openlogi-gui` | `openlogi-gui` binary — GPUI + gpui-component desktop app. |

### 4. Conventions
- Rust Edition 2024, MSRV 1.85.
- Workspace lints; clippy run with `-D warnings`.
- `unsafe` is confined to `openlogi-hook` (CGEventTap/FFI) and the core
  `binding.rs` dock SPI; every `unsafe`/lint allow carries a `reason`.
- Tracing via the `tracing` crate; level through `OPENLOGI_LOG`.

### 5. Critical gotchas (most valuable for an AI agent)
- **hidpp 0.2 feature registry is empty for our features.** Resolve features
  via `device.root().get_feature(ID)` + `device.add_feature::<F>()`, NOT
  `enumerate_features()` (its `versions: &[]` means our IDs never register, so
  `get_feature::<F>()` by TypeId returns `None`). Applies to every
  self-implemented feature: `0x2201`, `0x2111`, `0x1b04`, `0x2150`. See
  `write::open_feature`.
- **SmartShift `0x2111` function IDs are shifted** vs the older `0x2110`:
  getStatus = function 1, setStatus = function 2.
- **Directly-attached devices are addressed at index `0xff`** (`DIRECT_DEVICE_INDEX`).
  `probe_direct` has a phantom-device guard (requires a battery OR a control
  feature) so a Bolt receiver's secondary interface isn't mistaken for a mouse
  — don't remove it.
- **`Action::execute` has no automated test** (it would need to intercept the
  OS event queue). Smoke-test manually.
- **DPI writes read back and verify**; a mismatch logs a warning but still
  returns `Ok` (the request reached the device).
- **A few actions are stubs** (media keys log their intended NX key rather than
  posting it).
- **The hook callback runs on a background thread**, not the GPUI thread, and
  must return quickly — blocking it stalls system-wide input.

### 6. Stability contracts
`Action`, `ButtonId`, and `GestureDirection` variant *names* are the on-disk
`config.toml` schema (serde external tagging). New variants may be appended
freely; renaming/removing one is a migration event requiring a
`Config::SCHEMA_VERSION` bump.

### 7. Pointers
Link to: `docs/ARCHITECTURE.md` (detailed architecture), `docs/DEVELOPMENT.md`
(full dev workflow + packaging), `docs/USAGE.md` (CLI), `docs/CONFIGURATION.md`
(config file).

---

## File B: `docs/ARCHITECTURE.md`

Sections, depth proportional to complexity:

### 1. Overview
Crate dependency diagram (core at the bottom, no internal deps; hid/hook/cli/gui
depend on core; cli & gui at the top). Layering principles: `openlogi-core` is
deliberately I/O-free and never depends on `hidpp`/`async-hid`/platform APIs;
the protocol and platform crates do not leak their types into core (core mirrors
the HID++ types it needs — e.g. `DeviceKind`, `BatteryStatus`).

### 2. openlogi-core
- Serializable data model (`device.rs`): `DeviceInventory`, `PairedDevice`,
  `DeviceModelInfo`, and `config_key()` (`{ext_model_id:x}{model_ids[0]:04x}`,
  e.g. `"2b042"`) — the join key for config + asset registry.
- Config (`config.rs`): TOML at XDG path, `schema_version`, per-device
  `DeviceConfig`, per-app overlay (`effective_bindings`), atomic save (0600 on
  Unix), schema migration gate.
- Bindings (`binding.rs`): `ButtonId`, `Action` catalog (37 actions), categories,
  `detect_swipe`, gesture directions, `Action::execute` (macOS CGEvent posting:
  state the action count as "37 built-in" per the README only if it matches
  `Action::catalog().len()` at write time — otherwise describe it as "the action
  catalog" without a hard number, since `catalog()` excludes `CustomShortcut`;
  keys, clicks, scroll, Dock SPI for Mission Control/Exposé/Show Desktop/Launchpad,
  device-side actions deferred to the hook/HID layer).
- Paths (`paths.rs`): XDG base dirs on every OS (config at `~/.config/openlogi`).

### 3. openlogi-hid — the central HID++ flow
This is the most detailed section. Layers, bottom-up:
- **Transport** (`transport.rs`): `RawHidChannel` over `async-hid`; pre-filter
  to Logitech VID + HID++ long-report usage page (`0xff00`/`0x0002`), hardcode
  `supports_short_long_hidpp() = Some((true, true))` to dodge the
  Linux-only descriptor path.
- **Route** (`route.rs`): the single addressing seam. `DeviceRoute::Bolt`
  (receiver channel + pairing slot) vs `DeviceRoute::Direct` (own channel at
  index `0xff`). `open_route_channel` is the one place the Bolt-vs-direct
  branch lives; both write and capture paths go through it. Direct routes
  pre-filter on VID/PID before paying the ~100ms channel-open cost.
- **Feature wrappers**: `adjustable_dpi` (`0x2201`), `smartshift` (`0x2111`),
  `reprog_controls` (`0x1b04`), `thumbwheel` (`0x2150`) — re-implemented because
  hidpp 0.2 ships no typed wrappers; the root-feature lookup workaround.
- **Inventory** (`inventory.rs`): `enumerate()` merges two data sources per Bolt
  receiver (device-arrival events for online PIDs + per-slot pairing register
  for sleeping devices); `probe_direct` for BT/wired with the phantom-device
  guard; codename slicing workaround.
- **Write** (`write.rs`): `set_dpi` / `toggle_smartshift` re-open per call;
  read-back verify; `SharedChannel` for reuse by the capture session.
- **Capture** (`gesture.rs`): `run_capture_session` holds one channel open,
  diverts gesture button (raw-XY) / DPI-ModeShift / thumb wheel, one message
  listener, restores on shutdown; mid-swipe commit (160 ms gate); thumb wheel
  diverted only when its click is bound (re-synthesizes scroll).

### 4. openlogi-hook
macOS `CGEventTap` on a dedicated `CFRunLoop` thread; `Hook::start/stop`,
`has_accessibility`/`prompt_accessibility`, `frontmost_bundle_id`. Non-macOS:
`Hook` is uninhabited via `Infallible`, so `start` only ever returns
`HookError::Unsupported`. `MouseEvent` / `EventDisposition` (PassThrough vs
Suppress). The tap only sees standard buttons 0–4.

### 5. openlogi-assets
Registry schema (`index`, `manifest`, `metadata`) + I/O-light HTTP
(`http`: `AssetClient`, sha256 verify, cached fetch). Two consumers: CLI bulk
sync at packaging time, GUI per-device runtime fetch.

### 6. openlogi-cli
`run()` sets up tracing + clap, defaults to `list` when no subcommand.
Command tree: `list`, `diag` (e.g. `features` / `dpi`), `assets` (sync).

### 7. openlogi-gui
GPUI app. `AppState` GPUI global holds cross-view state (current device,
bindings, DPI, accessibility). `hook_runtime` is the bridge: mirrors the binding
map from `AppState`, installs the hook lazily, dispatches both hook events
(Middle/Back/Forward remap; L/R always pass through) and gesture events; routes
device-side actions (DPI cycle, SmartShift) to background HID writes reusing the
capture `SharedChannel`. Watchers (`watchers/`): accessibility, foreground_app
(per-app overlay, 1 Hz), inventory, pairing, gesture. Module groups: `state/`,
`mouse_model/`, `components/`, `platform/`, `asset/`, `windows/`.

### 8. End-to-end flows
Two or three cross-crate walkthroughs:
- **Startup & inventory:** GUI collects HID++ inventory synchronously on the
  main thread → builds device list → resolves assets/DPI per device.
- **Side-button press → remap:** CGEventTap (hook) → `hook_runtime` dispatch →
  `Action::execute` posts the CGEvent (or Suppress).
- **DPI slider release → device write:** GUI → background `set_dpi`/`set_dpi_on`
  → HID++ `0x2201` write + read-back verify.

---

## Out of Scope (YAGNI)

- No changes to existing docs, code, or behavior.
- No Linux/Windows documentation beyond noting the stubs.
- No per-function API reference (that's what rustdoc/`//!` comments are for —
  ARCHITECTURE.md links to the modules, not every signature).

## Success Criteria

- A fresh AI agent reading `CLAUDE.md` knows how to build/test, where each
  crate lives, and the non-obvious traps before touching code.
- `ARCHITECTURE.md` lets a reader trace the HID++ flow and the cross-crate
  flows end to end without reading every source file.
- Every command, feature ID, path, and constant in the docs matches the
  current source.
