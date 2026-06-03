# Wheel Scroll Settings — Design

**Date:** 2026-06-03
**Status:** Approved (design); plan pending
**Scope:** Global, software-side (CGEventTap) scroll engine for OpenLogi —
per-axis direction inversion + Mos-style smooth scrolling, configured from a new
GUI panel.

## 1. Goal

Add user-configurable wheel scroll behavior to OpenLogi, covering both the main
wheel and the thumb wheel:

- **Invert scroll direction**, independently per axis (vertical = main wheel,
  horizontal = thumb wheel).
- **Smooth scrolling** — a faithful port of [Mos](https://github.com/Caldis/Mos)'s
  approach: intercept the wheel event, swallow it, and re-emit interpolated
  synthetic scroll frames driven by a display-link frame clock.

The work is **software-side**, in the macOS event-tap layer — not HID++ device
commands. This is reliable across any mouse and matches how Mos works. At the
event-tap level a Logitech thumb wheel already surfaces as the horizontal scroll
axis (Axis 2), so "main wheel vs thumb wheel" maps cleanly onto "vertical vs
horizontal".

### Non-goals (v1)

Deliberately deferred (YAGNI): per-app overrides, allowlist mode, modifier
hotkeys (dash/toggle/block), simulate-trackpad phase emission, a CLI command,
and any HID++ device-side wheel configuration (`0x2121` / `0x2150`).

## 2. Decisions (locked)

| Decision | Choice |
|---|---|
| Inversion mechanism | Software event-tap (Mos-style), **not** HID++ flags |
| Feature scope | Invert **and** full Mos smooth scrolling |
| Settings scope | **Global**, applies to all mice (trackpads excluded) |
| Tuning params | Full Mos set: speed, step, duration, dead-zone + per-axis smooth toggles |
| UI surface | GUI panel (modeled on the existing DPI panel) |
| Engine location | New module inside `openlogi-hook` (Approach A) |

### Why Approach A (engine inside `openlogi-hook`)

The existing hook already:

- Owns an active (`CGEventTapOptions::Default`) CGEventTap at the HID location on
  a dedicated background thread with a CFRunLoop (`crates/openlogi-hook/src/macos.rs`).
- **Already includes `CGEventType::ScrollWheel` in its event mask** and translates
  it to `MouseEvent::Scroll { delta_x, delta_y }` (`macos.rs:160`).
- Opts in to `unsafe_code = "allow"` crate-wide, so adding CVDisplayLink /
  `CGEventPostToPid` FFI is consistent with the crate's existing role.
- Requires Accessibility permission — which a scroll tap needs anyway. No new
  permission surface.

Rejected alternatives:

- **B — separate `openlogi-scroll` crate:** would duplicate the tap/runloop
  plumbing and re-declare an `unsafe` opt-in for little benefit.
- **C — timer-thread frame clock instead of CVDisplayLink:** less FFI, but not
  vsync-aligned → judder. Mos chose CVDisplayLink deliberately; we follow it.

## 3. Architecture

Four layers, lowest to highest.

```
openlogi-core   ScrollSettings (serde) in AppSettings  ── config.toml [app_settings.scroll]
      │
openlogi-hook   scroll.rs engine + macos.rs tap hooks  ── reads ScrollSettings via arc-swap
      │                                                    CVDisplayLink frame loop, CGEventPostToPid
openlogi-gui    components/scroll_panel.rs              ── edits settings, pushes to hook, persists
```

### 3.1 Config — `openlogi-core/src/config.rs`

New struct, nested in `AppSettings` as a `scroll` field. All fields
`#[serde(default)]` so existing `config.toml` files load unchanged. **No
`SCHEMA_VERSION` bump** — this only *adds* optional fields; the stability
contract governs `Action`/`ButtonId`/`GestureDirection` variant names, none of
which change here.

```rust
pub struct ScrollSettings {
    pub smooth: bool,             // master smooth on/off
    pub reverse_vertical: bool,   // invert main wheel
    pub reverse_horizontal: bool, // invert thumb wheel
    pub smooth_vertical: bool,    // smooth the vertical axis
    pub smooth_horizontal: bool,  // smooth the horizontal axis
    pub speed: f64,               // distance multiplier   (Mos: 1..=10, def 2.70)
    pub step: f64,                // min effective quantum  (Mos: 0.01..=100, def 33.6)
    pub duration: f64,            // smoothing tail length  (Mos: 1..=5, def 4.35)
    pub dead_zone: f64,           // residual-frame cutoff  (Mos def 1.00)
}
```

Defaults mirror Mos's defaults. Add:

- `AppSettings { ..., #[serde(default)] pub scroll: ScrollSettings }`
- `Config::scroll_settings(&self) -> ScrollSettings`
- `Config::set_scroll_settings(&mut self, ScrollSettings)`
- include `scroll` in `AppSettings::is_default` / `Default`.

`duration` is the user-facing knob; the engine converts it to a lerp coefficient
(`durationTransition` in Mos): `transition = 1 - sqrt(duration / 5.2)`, rounded
to 3 dp. Conversion lives in the engine, not config.

### 3.2 Engine — `openlogi-hook`

#### Extend the tap callback contract (`macos.rs`)

Today the tap translates each event to a value `MouseEvent` and the callback
returns `PassThrough | Suppress` (Keep | Drop). That value-based path cannot
*mutate* the `CGEvent` (needed for invert) or *swallow-and-re-emit* (needed for
smooth). So the **scroll transform is owned inside `macos.rs`**, operating on the
live `&CGEvent` and `CGEventTapProxy` before the generic callback. The existing
`cb(MouseEvent::Scroll{..})` dispatch is preserved for binding-driven scroll
actions, but it no longer decides the wheel event's fate.

New per-scroll-event flow in the tap callback (runs on the hook thread, must
return fast):

1. **Skip our own synthetic events** — tagged via `SCROLL_WHEEL_EVENT_…`/
   `eventSourceUserData` marker; pass through untouched.
2. **Skip trackpads** — Mos heuristic: any nonzero `scrollWheelEventScrollPhase`,
   `scrollWheelEventMomentumPhase`, or `scrollWheelEventScrollCount` ⇒ trackpad ⇒
   pass through untouched.
3. **Invert** — if `reverse_vertical`, negate `DELTA_AXIS_1` + point/fixed-pt
   axis-1 fields; same for `reverse_horizontal` on axis 2. (Always applied,
   independent of smoothing.)
4. **Smooth or pass:**
   - If `smooth` and the axis's `smooth_<axis>` is on: feed the (inverted) delta
     into the `SmoothEngine` buffer and **Drop** the original event.
   - Otherwise return the (possibly inverted) event unchanged (Keep).

#### `scroll.rs` — `SmoothEngine`

Port of Mos's `ScrollPoster` + `Interpolator`:

- State: `current: (f64, f64)`, `buffer: (f64, f64)` target, target PID, last
  active time. Guarded for cross-thread access (tap thread writes buffer;
  display-link thread reads/updates current).
- On each input tick: `buffer += delta * speed`; capture target PID from the
  event's `EVENT_TARGET_UNIX_PROCESS_ID` field; ensure the frame loop is running.
- **Frame clock: CVDisplayLink** (new FFI). Each frame:
  - `frame = lerp(current → buffer, transition)`; `current += frame`.
  - If `|output| <= dead_zone` and converged → stop emitting / park the link.
  - Build a synthetic scroll `CGEvent` (continuous/pixel: set
    `POINT_DELTA_AXIS_1/2`, `IS_CONTINUOUS = 1`), **mark it synthetic**, and post
    to the captured PID via `CGEventPostToPid`.
- `step` normalizes sub-`step` input deltas up to the minimum quantum before
  buffering (Mos `normalizeY`/`normalizeX`).

Posting to the captured target PID (not re-injecting at the tap) keeps momentum
frames flowing to the originating app and avoids re-entering our own tap; the
synthetic marker is a second-line guard for step 1 above.

#### Settings hand-off

`ScrollSettings` is shared into the tap/engine via `arc-swap` (lock-free reads on
the hot path — honors the "callback must return quickly" gotcha). A new
`Hook`-level setter (e.g. `Hook::set_scroll_settings(ScrollSettings)`) swaps the
`Arc`. The engine reads the current snapshot at the top of each callback / frame.

New FFI added to `openlogi-hook` (all behind the existing crate-wide
`unsafe_code = "allow"`, each block carrying a `// SAFETY:` note):
`CVDisplayLinkCreateWithActiveCGDisplays`, `…SetOutputCallback`, `…Start`,
`…Stop`, `…Release`; `CGEventPostToPid`; `CGEventCreateCopy` (for the synthetic
template). Read-only event-field getters/setters already come from `core-graphics`.

### 3.3 GUI — `openlogi-gui/src/components/scroll_panel.rs`

Modeled on `components/dpi_panel.rs`:

- Controls: toggles for *Smooth*, *Invert vertical (wheel)*, *Invert horizontal
  (thumb wheel)*, *Smooth vertical*, *Smooth horizontal*; sliders for *speed*,
  *step*, *duration*, *dead-zone* (ranges per Mos).
- On change: update in-memory `ScrollSettings`, call the hook's
  `set_scroll_settings` (so the running engine picks it up live, like the DPI
  panel writes to the device), and `Config::set_scroll_settings(..)` +
  `save_atomic()`.
- On startup: load `ScrollSettings` from config and push once into the hook so
  behavior is active before the panel is opened.
- Placement: alongside `DpiPanel` in the main device page (`app.rs`), following
  the existing `dpi_panel` field/render pattern. (This is a global app setting
  rendered on the device page for discoverability; it is not per-device.)

Wiring to the running hook reuses the existing `hook_runtime` plumbing the GUI
already uses to start/stop the hook.

## 4. Build order

Phased so direction inversion is usable before the smoothing engine lands.

1. **Config:** `ScrollSettings` struct, `AppSettings.scroll`, accessors,
   `Default`/`is_default`, round-trip test.
2. **Invert in tap:** extend `macos.rs` scroll handling to mutate axis fields per
   `reverse_*`; wire `arc-swap` settings + `Hook::set_scroll_settings`. Verify
   inversion end-to-end manually.
3. **Smooth engine:** `scroll.rs` `SmoothEngine` — buffer/lerp, CVDisplayLink
   frame loop, `CGEventPostToPid`, synthetic marker, dead-zone stop. Verify
   smoothing + that synthetic events are not re-processed.
4. **Trackpad / synthetic skip:** add the heuristic guards; verify trackpad
   scrolling is untouched.
5. **GUI panel:** `scroll_panel.rs`, app wiring, live push to hook, persist;
   load-on-startup.

## 5. Risks & mitigations

- **Wedging system input:** the tap is at the HID location; a live tap that
  outlives its permission freezes all input. The existing slice-and-recheck loop
  (`macos.rs` `thread_main`) already mitigates this — the scroll engine must not
  block inside the callback. `arc-swap` reads + handing interpolation to the
  display-link thread keep the callback fast.
- **`Action::execute` / event-injection has no automated test** (CLAUDE gotcha):
  the synthetic-emission path is likewise manually smoke-tested. Pure logic
  (lerp, dead-zone, duration→transition, invert math, trackpad heuristic) is
  unit-tested in isolation.
- **Synthetic re-entry loop:** double-guarded — post to target PID (not our tap)
  *and* the synthetic marker check at callback entry.
- **Momentum target focus change:** posting to the captured PID (Mos's choice)
  keeps frames with the originating app rather than chasing focus.

## 6. Testing

- **Unit (logic-only, in `openlogi-hook`/`openlogi-core`):** `ScrollSettings`
  serde round-trip + defaults; `lerp`; `duration → transition`; per-axis invert
  math; trackpad heuristic classification; dead-zone convergence/stop.
- **Manual smoke (documented):** inversion of wheel + thumb wheel; smooth feel at
  a few speed/duration values; trackpad scrolling unaffected; no runaway
  synthetic loop; input recovers when Accessibility is revoked mid-scroll.
- Pre-commit gate unchanged: `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
