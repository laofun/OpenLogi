# Wheel Scroll Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a global, software-side scroll engine to OpenLogi — per-axis direction inversion (main wheel = vertical, thumb wheel = horizontal) plus Mos-style smooth scrolling — configured from a new GUI panel.

**Architecture:** All scroll transformation lives in `openlogi-hook`'s existing macOS CGEventTap. The tap mutates wheel events in place for inversion and, when smoothing is on, swallows the original event and re-emits interpolated synthetic frames driven by a `CVDisplayLink`, posted to the originating app via `CGEventPostToPid`. Settings are a new global `ScrollSettings` struct in `openlogi-core`, shared into the running tap through an `arc-swap` cell, and edited from a GUI panel modeled on the DPI panel.

**Tech Stack:** Rust (edition 2024), `core-graphics`/`core-foundation` FFI, `arc-swap`, GPUI + gpui-component, `serde`/`toml`.

**Design reference:** `docs/superpowers/specs/2026-06-03-wheel-scroll-settings-design.md`

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/openlogi-core/src/config.rs` | `ScrollSettings` struct + `AppSettings.scroll` field + accessors | Modify |
| `crates/openlogi-hook/Cargo.toml` | add `arc-swap` dependency | Modify |
| `crates/openlogi-hook/src/scroll.rs` | pure scroll math + `SmoothEngine` interpolation state (unit-tested) | Create |
| `crates/openlogi-hook/src/lib.rs` | `Hook` owns `Arc<ArcSwap<ScrollSettings>>`; `set_scroll_settings`; `mod scroll` | Modify |
| `crates/openlogi-hook/src/macos.rs` | scroll-event handling in the tap: invert + smooth glue, CVDisplayLink, `CGEventPostToPid` | Modify |
| `crates/openlogi-gui/src/components/scroll_panel.rs` | GUI panel: toggles + sliders, push to hook, persist | Create |
| `crates/openlogi-gui/src/components.rs` | register `scroll_panel` module | Modify |
| `crates/openlogi-gui/src/hook_runtime.rs` | expose the started `Hook` so settings can be pushed live | Modify (read in Task 7) |

**Key shared types (defined once, used across tasks — names are load-bearing):**

```rust
// openlogi-core::config
pub struct ScrollSettings {
    pub smooth: bool,
    pub reverse_vertical: bool,
    pub reverse_horizontal: bool,
    pub smooth_vertical: bool,
    pub smooth_horizontal: bool,
    pub speed: f64,
    pub step: f64,
    pub duration: f64,
    pub dead_zone: f64,
}

// openlogi-hook::scroll  (pure, unit-tested)
pub fn duration_to_transition(duration: f64) -> f64;
pub fn lerp(src: f64, dest: f64, trans: f64) -> f64;
pub fn normalize(value: f64, step: f64) -> f64;
pub fn is_trackpad(scroll_phase: f64, momentum_phase: f64, scroll_count: i64) -> bool;
pub struct SmoothEngine { /* buffer, current */ }
impl SmoothEngine {
    pub fn new() -> Self;
    pub fn add(&mut self, dx: f64, dy: f64, speed: f64, step: f64);
    pub fn advance(&mut self, transition: f64, dead_zone: f64) -> Option<(f64, f64)>;
    pub fn is_idle(&self) -> bool;
}
```

**Reference: macOS scroll `CGEventField` numbers** (core-graphics 0.23 only names some; pass the rest as raw `u32`):

| Field | Const / raw |
|---|---|
| line delta axis 1 / 2 | `EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1` (11) / `_2` (12) |
| point delta axis 1 / 2 | `EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1` (96) / `_2` (97) |
| fixed-pt delta axis 1 / 2 | raw `93` / `94` |
| is-continuous | `EventField::SCROLL_WHEEL_EVENT_IS_CONTINUOUS` (88) |
| scroll phase | raw `99` |
| momentum phase | raw `123` |
| scroll count | raw `100` |
| event target pid | `EventField::EVENT_TARGET_UNIX_PROCESS_ID` (40) |
| event source user data | `EventField::EVENT_SOURCE_USER_DATA` (42) |

---

## Task 1: `ScrollSettings` config type

**Files:**
- Modify: `crates/openlogi-core/src/config.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/openlogi-core/src/config.rs`:

```rust
#[test]
fn scroll_settings_default_matches_mos() {
    let s = ScrollSettings::default();
    assert!(s.smooth);
    assert!(s.reverse_vertical);
    assert!(s.reverse_horizontal);
    assert!(s.smooth_vertical);
    assert!(s.smooth_horizontal);
    assert!((s.speed - 2.70).abs() < f64::EPSILON);
    assert!((s.step - 33.6).abs() < f64::EPSILON);
    assert!((s.duration - 4.35).abs() < f64::EPSILON);
    assert!((s.dead_zone - 1.00).abs() < f64::EPSILON);
}

#[test]
fn scroll_settings_roundtrip() {
    let mut cfg = Config::default();
    let mut s = ScrollSettings::default();
    s.smooth = false;
    s.reverse_horizontal = false;
    s.speed = 5.0;
    cfg.set_scroll_settings(s.clone());

    let parsed = write_and_read(&cfg);
    let got = parsed.scroll_settings();
    assert_eq!(got.smooth, false);
    assert_eq!(got.reverse_horizontal, false);
    assert!((got.speed - 5.0).abs() < f64::EPSILON);
    // Untouched fields keep their defaults.
    assert!(got.reverse_vertical);
}

#[test]
fn config_without_scroll_section_loads() {
    // A config file predating ScrollSettings must still parse, with defaults.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(&path, "schema_version = 1\n").expect("write");
    let cfg = Config::load_from_path(&path).expect("load");
    assert_eq!(cfg.scroll_settings(), ScrollSettings::default());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p openlogi-core scroll_settings`
Expected: FAIL — `cannot find type ScrollSettings` / `no method set_scroll_settings`.

- [ ] **Step 3: Add the type, the field, and the accessors**

In `crates/openlogi-core/src/config.rs`, add the struct after `AppSettings` (near line 124):

```rust
/// Global, software-side scroll behavior applied by the macOS event-tap
/// engine in `openlogi-hook`. Not tied to any device — these apply to every
/// non-trackpad scroll event. Defaults mirror Mos.
///
/// All fields are `#[serde(default)]` so a config predating this section
/// loads with these defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "these are independent user toggles, not a state machine"
)]
pub struct ScrollSettings {
    /// Master smooth-scrolling switch.
    #[serde(default = "default_true")]
    pub smooth: bool,
    /// Invert the vertical axis (main wheel).
    #[serde(default = "default_true")]
    pub reverse_vertical: bool,
    /// Invert the horizontal axis (thumb wheel).
    #[serde(default = "default_true")]
    pub reverse_horizontal: bool,
    /// Smooth the vertical axis when `smooth` is on.
    #[serde(default = "default_true")]
    pub smooth_vertical: bool,
    /// Smooth the horizontal axis when `smooth` is on.
    #[serde(default = "default_true")]
    pub smooth_horizontal: bool,
    /// Distance multiplier (Mos range 1..=10).
    #[serde(default = "default_scroll_speed")]
    pub speed: f64,
    /// Minimum effective wheel quantum (Mos range 0.01..=100).
    #[serde(default = "default_scroll_step")]
    pub step: f64,
    /// Smoothing tail length, user-facing (Mos range 1..=5). The engine maps
    /// this to a lerp coefficient.
    #[serde(default = "default_scroll_duration")]
    pub duration: f64,
    /// Residual-frame cutoff below which a smoothing run stops.
    #[serde(default = "default_scroll_dead_zone")]
    pub dead_zone: f64,
}

fn default_scroll_speed() -> f64 {
    2.70
}
fn default_scroll_step() -> f64 {
    33.6
}
fn default_scroll_duration() -> f64 {
    4.35
}
fn default_scroll_dead_zone() -> f64 {
    1.00
}

impl Default for ScrollSettings {
    fn default() -> Self {
        Self {
            smooth: true,
            reverse_vertical: true,
            reverse_horizontal: true,
            smooth_vertical: true,
            smooth_horizontal: true,
            speed: default_scroll_speed(),
            step: default_scroll_step(),
            duration: default_scroll_duration(),
            dead_zone: default_scroll_dead_zone(),
        }
    }
}

impl ScrollSettings {
    /// `skip_serializing_if` helper: true when nothing diverges from default.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
```

Add the field to `AppSettings` (struct around line 58; insert before the closing brace, after `language`):

```rust
    /// Global software-side scroll behavior (invert + smooth). See
    /// [`ScrollSettings`].
    #[serde(default, skip_serializing_if = "ScrollSettings::is_default")]
    pub scroll: ScrollSettings,
```

Add `scroll: ScrollSettings::default(),` to the `AppSettings` `Default` impl (around line 109).

Add the accessors inside `impl Config` (after `set_smartshift_sensitivity`, around line 388):

```rust
    /// The global scroll settings (invert + smooth). Returns defaults when the
    /// config has no `[app_settings.scroll]` section.
    #[must_use]
    pub fn scroll_settings(&self) -> ScrollSettings {
        self.app_settings.scroll.clone()
    }

    /// Replace the global scroll settings.
    pub fn set_scroll_settings(&mut self, settings: ScrollSettings) {
        self.app_settings.scroll = settings;
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p openlogi-core scroll && cargo test -p openlogi-core`
Expected: PASS (new tests + existing config tests unaffected).

- [ ] **Step 5: Commit**

```bash
git add crates/openlogi-core/src/config.rs
git commit -m "feat(core): add global ScrollSettings to config"
```

---

## Task 2: Pure scroll math + `SmoothEngine` (`openlogi-hook`)

This task holds **all unit-testable logic**: the duration→coefficient mapping, the interpolation step, input normalization, the trackpad heuristic, and the buffer/dead-zone state machine — with no FFI.

**Files:**
- Modify: `crates/openlogi-hook/Cargo.toml`
- Create: `crates/openlogi-hook/src/scroll.rs`
- Modify: `crates/openlogi-hook/src/lib.rs` (add `mod scroll;`)

- [ ] **Step 1: Add the `arc-swap` dependency**

In `crates/openlogi-hook/Cargo.toml`, under `[dependencies]`, add:

```toml
arc-swap = "1"
```

- [ ] **Step 2: Write the failing test file**

Create `crates/openlogi-hook/src/scroll.rs` with logic + tests in one file:

```rust
//! Pure scroll math and interpolation state for the macOS smooth-scroll
//! engine. No FFI lives here — the CGEvent/CVDisplayLink glue is in `macos.rs`.
//! This module is unit-tested in isolation.

/// Mos's upper limit for the duration knob (`5.0 + 0.2`).
const DURATION_UPPER_LIMIT: f64 = 5.2;

/// Map the user-facing `duration` (Mos range 1..=5) to a lerp coefficient.
/// Larger duration → smaller coefficient → slower convergence → longer tail.
/// Mirrors Mos `generateDurationTransition`.
#[must_use]
pub fn duration_to_transition(duration: f64) -> f64 {
    let val = 1.0 - (duration / DURATION_UPPER_LIMIT).sqrt();
    (val * 1000.0).round() / 1000.0
}

/// One interpolation step: the fraction of the remaining distance to travel
/// this frame. Mirrors Mos `Interpolator.lerp`.
#[must_use]
pub fn lerp(src: f64, dest: f64, trans: f64) -> f64 {
    (dest - src) * trans
}

/// Raise sub-`step` input magnitudes up to the minimum quantum, preserving
/// sign. A zero input stays zero. Mirrors Mos `normalize`.
#[must_use]
pub fn normalize(value: f64, step: f64) -> f64 {
    if value == 0.0 {
        return 0.0;
    }
    if value.abs() < step {
        return step.copysign(value);
    }
    value
}

/// Classify an event as trackpad-origin from its phase/count fields. Any
/// nonzero phase or scroll-count signals a trackpad (or already-continuous
/// source) that the engine must leave untouched. Mirrors Mos `isTrackpad`.
#[must_use]
pub fn is_trackpad(scroll_phase: f64, momentum_phase: f64, scroll_count: i64) -> bool {
    scroll_phase != 0.0 || momentum_phase != 0.0 || scroll_count != 0
}

/// Accumulates scroll deltas and emits per-frame interpolated output until it
/// converges. The tap thread calls [`Self::add`]; the display-link thread
/// calls [`Self::advance`].
#[derive(Debug, Default)]
pub struct SmoothEngine {
    /// Target accumulated distance.
    buffer: (f64, f64),
    /// Distance already emitted.
    current: (f64, f64),
}

impl SmoothEngine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an input tick (already inverted if applicable). `dx`/`dy` are the
    /// raw event deltas; they are normalized to `step` then scaled by `speed`.
    pub fn add(&mut self, dx: f64, dy: f64, speed: f64, step: f64) {
        self.buffer.0 += normalize(dx, step) * speed;
        self.buffer.1 += normalize(dy, step) * speed;
    }

    /// Advance one frame. Returns the `(dx, dy)` to post this frame, or `None`
    /// once the run has settled (output below `dead_zone` on both axes), at
    /// which point the engine resets so the next burst starts clean.
    pub fn advance(&mut self, transition: f64, dead_zone: f64) -> Option<(f64, f64)> {
        let step = (
            lerp(self.current.0, self.buffer.0, transition),
            lerp(self.current.1, self.buffer.1, transition),
        );
        if step.0.abs() <= dead_zone && step.1.abs() <= dead_zone {
            // Settled: drop the residue and park.
            self.buffer = (0.0, 0.0);
            self.current = (0.0, 0.0);
            return None;
        }
        self.current.0 += step.0;
        self.current.1 += step.1;
        Some(step)
    }

    /// True when there is no in-flight scroll (nothing buffered or emitted).
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.buffer == (0.0, 0.0) && self.current == (0.0, 0.0)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, reason = "exact-default comparisons are intentional")]
mod tests {
    use super::*;

    #[test]
    fn duration_transition_matches_mos_formula() {
        // duration 5.2 → coefficient 0; duration 0 → 1.
        assert_eq!(duration_to_transition(5.2), 0.0);
        assert_eq!(duration_to_transition(0.0), 1.0);
        // Mos default 4.35 → 1 - sqrt(4.35/5.2) ≈ 0.085.
        assert_eq!(duration_to_transition(4.35), 0.085);
    }

    #[test]
    fn lerp_moves_a_fraction_toward_dest() {
        assert_eq!(lerp(0.0, 100.0, 0.1), 10.0);
        assert_eq!(lerp(50.0, 100.0, 0.5), 25.0);
        assert_eq!(lerp(100.0, 100.0, 0.5), 0.0);
    }

    #[test]
    fn normalize_raises_small_inputs_keeps_sign() {
        assert_eq!(normalize(0.0, 10.0), 0.0);
        assert_eq!(normalize(3.0, 10.0), 10.0);
        assert_eq!(normalize(-3.0, 10.0), -10.0);
        assert_eq!(normalize(50.0, 10.0), 50.0);
    }

    #[test]
    fn trackpad_detected_from_any_phase_field() {
        assert!(!is_trackpad(0.0, 0.0, 0));
        assert!(is_trackpad(1.0, 0.0, 0));
        assert!(is_trackpad(0.0, 2.0, 0));
        assert!(is_trackpad(0.0, 0.0, 1));
    }

    #[test]
    fn engine_converges_and_then_idles() {
        let mut e = SmoothEngine::new();
        e.add(0.0, 10.0, 1.0, 0.0); // buffer.1 = 10
        let mut emitted = 0.0;
        // High transition converges fast; a generous dead_zone ends it.
        let mut frames = 0;
        while let Some((_, dy)) = e.advance(0.5, 0.5) {
            emitted += dy;
            frames += 1;
            assert!(frames < 100, "must converge");
        }
        assert!(e.is_idle());
        // Most of the buffered distance was emitted before parking.
        assert!(emitted > 8.0 && emitted <= 10.0, "emitted = {emitted}");
    }

    #[test]
    fn idle_engine_emits_nothing() {
        let mut e = SmoothEngine::new();
        assert!(e.advance(0.1, 1.0).is_none());
        assert!(e.is_idle());
    }
}
```

- [ ] **Step 3: Register the module**

In `crates/openlogi-hook/src/lib.rs`, add near the other `mod` declarations (around line 200, next to `#[cfg(target_os = "macos")] mod macos;`):

```rust
mod scroll;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p openlogi-hook scroll`
Expected: PASS (6 tests). If `duration_to_transition(4.35)` is not exactly `0.085`, compute the rounded value and update the assertion to the printed value — the formula is the source of truth.

- [ ] **Step 5: Commit**

```bash
git add crates/openlogi-hook/Cargo.toml crates/openlogi-hook/src/scroll.rs crates/openlogi-hook/src/lib.rs
git commit -m "feat(hook): pure scroll math + SmoothEngine interpolation state"
```

---

## Task 3: Thread `ScrollSettings` into the `Hook`

Give `Hook` an `arc-swap` cell holding the current `ScrollSettings`, a setter, and pass a clone into the macOS tap. The tap doesn't *use* it yet (Tasks 4–6) — this task only wires the plumbing and keeps everything compiling.

**Files:**
- Modify: `crates/openlogi-hook/src/lib.rs`
- Modify: `crates/openlogi-hook/src/macos.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/openlogi-hook/src/tests.rs`:

```rust
// `set_scroll_settings` must be callable on a `Hook` value. We can't start a
// real tap without Accessibility in CI, so this test only guards the API
// shape via a compile-time reference to the method.
#[test]
fn hook_exposes_scroll_setter() {
    fn _assert_api(h: &crate::Hook, s: openlogi_core::config::ScrollSettings) {
        h.set_scroll_settings(s);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p openlogi-hook hook_exposes_scroll_setter`
Expected: FAIL — `no method named set_scroll_settings`.

- [ ] **Step 3: Add the cell, setter, and macos plumbing**

In `crates/openlogi-hook/src/lib.rs`:

Add imports near the top:

```rust
use std::sync::Arc;
use arc_swap::ArcSwap;
use openlogi_core::config::ScrollSettings;
```

Add a field to `Hook` (the struct around line 81). It must exist on **all** platforms so the setter compiles everywhere:

```rust
pub struct Hook {
    #[cfg(target_os = "macos")]
    inner: Option<macos::HookInner>,
    /// Current scroll settings, read live by the macOS tap. Shared via
    /// `ArcSwap` so the tap callback reads it lock-free on the hot path.
    scroll: Arc<ArcSwap<ScrollSettings>>,
    #[cfg(not(target_os = "macos"))]
    never: std::convert::Infallible,
}
```

> Note: the `#[cfg(not(target_os = "macos"))] never: Infallible` field makes `Hook` uninhabited off-macOS, so the struct literal there is still unreachable. Keep both cfg fields; `scroll` is unconditional.

Update `Hook::start` to build the cell and pass a clone to `macos::start`:

```rust
    pub fn start(
        cb: impl Fn(MouseEvent) -> EventDisposition + Send + Sync + 'static,
    ) -> Result<Self, HookError> {
        let scroll = Arc::new(ArcSwap::from_pointee(ScrollSettings::default()));
        #[cfg(target_os = "macos")]
        {
            macos::start(cb, scroll.clone()).map(|inner| Self {
                inner: Some(inner),
                scroll,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (cb, scroll);
            Err(HookError::Unsupported)
        }
    }
```

Add the setter to `impl Hook` (after `prompt_accessibility`):

```rust
    /// Replace the live scroll settings read by the macOS scroll engine.
    /// Cheap and lock-free; safe to call from the GPUI thread on every edit.
    pub fn set_scroll_settings(&self, settings: ScrollSettings) {
        self.scroll.store(Arc::new(settings));
    }
```

In `crates/openlogi-hook/src/macos.rs`, change `start` and `thread_main` to accept and forward the cell (it is stored for Tasks 4–6; mark it used to avoid warnings for now):

```rust
use std::sync::Arc;
use arc_swap::ArcSwap;
use openlogi_core::config::ScrollSettings;

pub(crate) fn start(
    cb: impl Fn(MouseEvent) -> EventDisposition + Send + Sync + 'static,
    scroll: Arc<ArcSwap<ScrollSettings>>,
) -> Result<HookInner, HookError> {
    // ... unchanged accessibility check + Arc<cb> wrap ...
    let thread = thread::Builder::new()
        .name("openlogi-hook".into())
        .spawn(move || thread_main(cb, scroll, rl_tx))
        .map_err(|e| HookError::MacOsTap(e.to_string()))?;
    // ... unchanged ...
}
```

Update `thread_main`'s signature to `fn thread_main(cb: ..., scroll: Arc<ArcSwap<ScrollSettings>>, rl_tx: ...)` and, for this task only, add `let _ = &scroll;` near the top so it compiles unused. (Task 4 removes that line and uses it.)

- [ ] **Step 4: Run the test + full build to verify they pass**

Run: `cargo test -p openlogi-hook hook_exposes_scroll_setter && cargo build -p openlogi-hook`
Expected: PASS + clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/openlogi-hook/src/lib.rs crates/openlogi-hook/src/macos.rs
git commit -m "feat(hook): thread live ScrollSettings into the tap via arc-swap"
```

---

## Task 4: Invert scroll direction in the tap

Mutate the wheel event's axis fields in place when `reverse_*` is set, before the event passes through. Inversion works independently of smoothing. The CGEvent is shared (`&CGEvent`) but its setters take `&self` (interior mutability), so in-place mutation in the tap callback is valid.

> No automated test: this drives the live OS event stream (same constraint the CLAUDE notes call out for `Action::execute`). Verified manually.

**Files:**
- Modify: `crates/openlogi-hook/src/macos.rs`

- [ ] **Step 1: Add a scroll-handling helper and call it from the tap closure**

In `crates/openlogi-hook/src/macos.rs`, add a helper. Negate all three delta representations per axis so apps reading line, point, or fixed-point deltas all see the inversion (matches Mos `reverseY`/`reverseX`):

```rust
/// Raw CGEventField numbers that core-graphics 0.23 does not name.
const FIELD_FIXED_PT_DELTA_AXIS_1: core_graphics::event::CGEventField = 93;
const FIELD_FIXED_PT_DELTA_AXIS_2: core_graphics::event::CGEventField = 94;

/// Negate axis 1 (vertical) and/or axis 2 (horizontal) deltas in place.
fn apply_invert(event: &CGEvent, reverse_vertical: bool, reverse_horizontal: bool) {
    if reverse_vertical {
        negate_axis(
            event,
            EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1,
            EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1,
            FIELD_FIXED_PT_DELTA_AXIS_1,
        );
    }
    if reverse_horizontal {
        negate_axis(
            event,
            EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2,
            EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2,
            FIELD_FIXED_PT_DELTA_AXIS_2,
        );
    }
}

fn negate_axis(
    event: &CGEvent,
    line_field: EventField,
    point_field: core_graphics::event::CGEventField,
    fixed_field: core_graphics::event::CGEventField,
) {
    let line = event.get_integer_value_field(line_field);
    event.set_integer_value_field(line_field, -line);
    let point = event.get_double_value_field(point_field);
    event.set_double_value_field(point_field, -point);
    let fixed = event.get_double_value_field(fixed_field);
    event.set_double_value_field(fixed_field, -fixed);
}
```

> `EventField` is an enum that the named methods accept directly; the point/fixed fields are passed as raw `CGEventField` (a `u32`). If the named-method overloads only accept `EventField`, use the raw-`u32` setter overload for all six fields uniformly — confirm against the imported `EventField`/`CGEventField` types when the file is open.

- [ ] **Step 2: Handle scroll specially in the tap closure**

In `thread_main`, remove the temporary `let _ = &scroll;` from Task 3. Clone `scroll` into the tap closure and branch on `ScrollWheel` *before* the generic translate/cb path:

```rust
    let scroll_for_tap = scroll.clone();
    let tap_result = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        event_types,
        move |_proxy: CGEventTapProxy, etype: CGEventType, event: &CGEvent| {
            if etype == CGEventType::ScrollWheel {
                let cfg = scroll_for_tap.load();
                apply_invert(event, cfg.reverse_vertical, cfg.reverse_horizontal);
                // Smoothing is added in Tasks 5–6; for now, always keep the
                // (possibly inverted) event.
                return CallbackResult::Keep;
            }
            let Some(mouse_event) = translate(etype, event) else {
                return CallbackResult::Keep;
            };
            match cb(mouse_event) {
                EventDisposition::PassThrough => CallbackResult::Keep,
                EventDisposition::Suppress => CallbackResult::Drop,
            }
        },
    );
```

> This makes the scroll engine, not the generic `cb`, the authority over wheel events. The `MouseEvent::Scroll` dispatch path in `hook_runtime.rs` (which only returns `PassThrough`) is now bypassed for the wheel; nothing in the app currently acts on `MouseEvent::Scroll`, so behavior is unchanged except for the new inversion.

- [ ] **Step 3: Build**

Run: `cargo build -p openlogi-hook`
Expected: clean build.

- [ ] **Step 4: Manual smoke test (documented, not automated)**

Build and run the GUI with a Logitech mouse connected and Accessibility granted:

```bash
cargo run -p openlogi-gui --release
```

In a scratch test (Task 7 wires the UI), temporarily default `ScrollSettings { smooth: false, .. }` via config or a one-off `set_scroll_settings`, then scroll: vertical wheel direction should reverse, and the thumb wheel (horizontal) should reverse independently. Trackpad two-finger scroll is also affected for now (Task 6 excludes it).

- [ ] **Step 5: Commit**

```bash
git add crates/openlogi-hook/src/macos.rs
git commit -m "feat(hook): invert scroll direction per axis in the tap"
```

---

## Task 5: Smooth-scroll engine — CVDisplayLink + postToPid glue

Wire `SmoothEngine` (Task 2) into the tap: when `smooth` is on for an axis, feed the inverted delta into a shared engine, swallow the original event, and emit interpolated synthetic frames from a `CVDisplayLink` callback, posting each to the originating app's PID. Synthetic events are tagged so the tap ignores them.

> FFI-heavy, no automated test. Verified manually. All `unsafe` blocks carry a `// SAFETY:` comment (crate already opts into `unsafe_code = "allow"`).

**Files:**
- Modify: `crates/openlogi-hook/src/macos.rs`
- Modify: `crates/openlogi-hook/src/scroll.rs` (add the shared-state wrapper)

- [ ] **Step 1: Add a thread-safe engine wrapper in `scroll.rs`**

Append to `crates/openlogi-hook/src/scroll.rs`:

```rust
use std::sync::Mutex;

/// Shared smoothing state plus the captured target PID. The tap thread writes
/// (`push`); the display-link thread drains (`frame`).
#[derive(Debug, Default)]
pub struct SharedSmooth {
    engine: Mutex<SmoothEngine>,
    /// PID of the app the original scroll targeted, captured on the last input.
    target_pid: std::sync::atomic::AtomicI32,
}

impl SharedSmooth {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an input tick and remember its target PID. Returns the PID so the
    /// caller can decide whether to (re)start the display link.
    pub fn push(&self, dx: f64, dy: f64, speed: f64, step: f64, pid: i32) {
        self.target_pid
            .store(pid, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut e) = self.engine.lock() {
            e.add(dx, dy, speed, step);
        }
    }

    /// Compute the next frame. Returns `(dx, dy, pid)` to post, or `None` when
    /// settled (the caller should stop the display link).
    pub fn frame(&self, transition: f64, dead_zone: f64) -> Option<(f64, f64, i32)> {
        let mut e = self.engine.lock().ok()?;
        let (dx, dy) = e.advance(transition, dead_zone)?;
        let pid = self.target_pid.load(std::sync::atomic::Ordering::Relaxed);
        Some((dx, dy, pid))
    }
}
```

Make the wrapper visible to `macos.rs`: it is `pub` in a private module, which is fine within the crate. Add a quick test:

```rust
#[test]
fn shared_smooth_drains_to_none() {
    let s = SharedSmooth::new();
    s.push(0.0, 10.0, 1.0, 0.0, 1234);
    let mut got = 0.0;
    while let Some((_, dy, pid)) = s.frame(0.5, 0.5) {
        assert_eq!(pid, 1234);
        got += dy;
    }
    assert!(got > 8.0);
}
```

- [ ] **Step 2: Add the CVDisplayLink + post FFI to `macos.rs`**

Add the FFI declarations and a `DisplayLink` owner. The display-link callback is a bare `extern "C"` fn receiving an `Arc<SharedSmooth>` pointer as its `user_info`.

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use core_graphics::event::{CGEvent, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use crate::scroll::{self, SharedSmooth};

type CVReturn = i32;
type CVDisplayLinkRef = *mut std::ffi::c_void;
#[allow(non_camel_case_types)]
type CVTimeStamp = std::ffi::c_void;

type CVDisplayLinkOutputCallback = extern "C" fn(
    CVDisplayLinkRef,
    *const CVTimeStamp,
    *const CVTimeStamp,
    u64,
    *mut u64,
    *mut std::ffi::c_void,
) -> CVReturn;

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVDisplayLinkCreateWithActiveCGDisplays(out: *mut CVDisplayLinkRef) -> CVReturn;
    fn CVDisplayLinkSetOutputCallback(
        link: CVDisplayLinkRef,
        cb: CVDisplayLinkOutputCallback,
        user_info: *mut std::ffi::c_void,
    ) -> CVReturn;
    fn CVDisplayLinkStart(link: CVDisplayLinkRef) -> CVReturn;
    fn CVDisplayLinkStop(link: CVDisplayLinkRef) -> CVReturn;
    fn CVDisplayLinkRelease(link: CVDisplayLinkRef);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventPostToPid(pid: i32, event: core_foundation::base::CFTypeRef);
}

/// Marker stamped on our synthetic events (`eventSourceUserData`) so the tap
/// skips them instead of re-smoothing.
const SYNTHETIC_MARKER: i64 = 0x4F4C_4753; // "OLGS"
const FIELD_SCROLL_PHASE: core_graphics::event::CGEventField = 99;
const FIELD_MOMENTUM_PHASE: core_graphics::event::CGEventField = 123;
const FIELD_SCROLL_COUNT: core_graphics::event::CGEventField = 100;
```

Add the engine-driver state (lives for the tap thread's lifetime) and the callback:

```rust
/// Owns the running display link and the shared smoothing state.
struct ScrollDriver {
    shared: Arc<SharedSmooth>,
    scroll: Arc<ArcSwap<ScrollSettings>>,
    link: CVDisplayLinkRef,
    running: AtomicBool,
}

// SAFETY: CVDisplayLinkRef is a CoreVideo object pointer; CoreVideo permits
// start/stop from any thread. The Arc fields are Send+Sync.
unsafe impl Send for ScrollDriver {}
unsafe impl Sync for ScrollDriver {}

extern "C" fn display_link_cb(
    _link: CVDisplayLinkRef,
    _now: *const CVTimeStamp,
    _out: *const CVTimeStamp,
    _flags: u64,
    _flags_out: *mut u64,
    user_info: *mut std::ffi::c_void,
) -> CVReturn {
    // SAFETY: `user_info` is the `*const ScrollDriver` we passed to
    // CVDisplayLinkSetOutputCallback; it outlives the link (we stop the link
    // before dropping the driver).
    let driver = unsafe { &*(user_info as *const ScrollDriver) };
    let cfg = driver.scroll.load();
    let transition = scroll::duration_to_transition(cfg.duration);
    match driver.shared.frame(transition, cfg.dead_zone) {
        Some((dx, dy, pid)) => post_synthetic_scroll(dx, dy, pid),
        None => {
            // Settled: stop the link until the next input restarts it.
            driver.running.store(false, Ordering::Release);
            // SAFETY: valid link ref; CVDisplayLinkStop is thread-safe.
            unsafe { CVDisplayLinkStop(driver.link) };
        }
    }
    0 // kCVReturnSuccess
}

/// Build one continuous (pixel) synthetic scroll event, tag it, and post it to
/// `pid`. Axis-1 = vertical, axis-2 = horizontal.
fn post_synthetic_scroll(dx: f64, dy: f64, pid: i32) {
    use core_foundation::base::TCFType as _;
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else {
        return;
    };
    // wheel1 = vertical, wheel2 = horizontal (rounded to whole pixels).
    let Ok(event) = CGEvent::new_scroll_event(
        source,
        core_graphics::event::ScrollEventUnit::PIXEL,
        2,
        dy.round() as i32,
        dx.round() as i32,
        0,
    ) else {
        return;
    };
    event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, SYNTHETIC_MARKER);
    event.set_double_value_field(EventField::SCROLL_WHEEL_EVENT_IS_CONTINUOUS, 1.0);
    // SAFETY: `event` is a live CGEventRef for the duration of the call;
    // CGEventPostToPid copies what it needs.
    unsafe { CGEventPostToPid(pid, event.as_concrete_TypeRef().cast()) };
}
```

> If `dy.round() as i32` triggers a clippy cast lint, wrap with the crate's existing `#[allow(clippy::cast_possible_truncation, reason = "...")]` pattern (see `macos.rs` `translate`).

- [ ] **Step 3: Construct the driver and feed it from the tap closure**

In `thread_main`, before building the tap, create the driver and the display link:

```rust
    let shared = Arc::new(SharedSmooth::new());
    let driver = Box::new(ScrollDriver {
        shared: shared.clone(),
        scroll: scroll.clone(),
        link: std::ptr::null_mut(),
        running: AtomicBool::new(false),
    });
    // Leak the driver for the thread's lifetime; the process owns it until exit.
    let driver: &'static mut ScrollDriver = Box::leak(driver);
    // SAFETY: out-pointer receives a fresh link; callback/user_info set before start.
    unsafe {
        CVDisplayLinkCreateWithActiveCGDisplays(&mut driver.link);
        CVDisplayLinkSetOutputCallback(
            driver.link,
            display_link_cb,
            (driver as *const ScrollDriver) as *mut std::ffi::c_void,
        );
    }
```

Update the `ScrollWheel` branch in the tap closure to smooth when enabled:

```rust
            if etype == CGEventType::ScrollWheel {
                // Skip our own synthetic frames.
                if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA)
                    == SYNTHETIC_MARKER
                {
                    return CallbackResult::Keep;
                }
                let cfg = scroll_for_tap.load();
                apply_invert(event, cfg.reverse_vertical, cfg.reverse_horizontal);

                let dy = event.get_double_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1);
                let dx = event.get_double_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2);
                let smooth_v = cfg.smooth && cfg.smooth_vertical && dy != 0.0;
                let smooth_h = cfg.smooth && cfg.smooth_horizontal && dx != 0.0;
                if smooth_v || smooth_h {
                    let pid = event
                        .get_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID)
                        as i32;
                    driver.shared.push(
                        if smooth_h { dx } else { 0.0 },
                        if smooth_v { dy } else { 0.0 },
                        cfg.speed,
                        cfg.step,
                        pid,
                    );
                    if !driver.running.swap(true, Ordering::AcqRel) {
                        // SAFETY: link created above; safe to start from any thread.
                        unsafe { CVDisplayLinkStart(driver.link) };
                    }
                    return CallbackResult::Drop; // swallow original; frames re-emit it
                }
                return CallbackResult::Keep; // inverted-only or smoothing off
            }
```

> The `driver` reference is `'static` (leaked), so capturing it in the `'static` tap closure is sound. On teardown (`disable_tap`), also stop the link — add `unsafe { CVDisplayLinkStop(driver.link) };` near the existing `disable_tap(&tap)` call so frames stop when Accessibility is revoked.

- [ ] **Step 4: Build + run the engine unit test**

Run: `cargo test -p openlogi-hook scroll && cargo build -p openlogi-hook`
Expected: PASS + clean build.

- [ ] **Step 5: Manual smoke test**

Run the GUI (`cargo run -p openlogi-gui --release`) with `smooth = true`. Scroll the wheel: motion should be smoothly interpolated rather than steppy, and stop cleanly (no drift / no runaway). Switch apps mid-momentum — frames keep going to the app that was scrolled. Confirm no feedback loop (CPU stays low when idle; the synthetic marker is respected).

- [ ] **Step 6: Commit**

```bash
git add crates/openlogi-hook/src/macos.rs crates/openlogi-hook/src/scroll.rs
git commit -m "feat(hook): Mos-style smooth scrolling via CVDisplayLink + postToPid"
```

---

## Task 6: Exclude trackpads and already-continuous events

Use `is_trackpad` (Task 2) so two-finger trackpad scrolling and other continuous sources pass through untouched — no inversion, no smoothing.

**Files:**
- Modify: `crates/openlogi-hook/src/macos.rs`

- [ ] **Step 1: Add the guard at the top of the `ScrollWheel` branch**

Immediately after the synthetic-marker check, before `apply_invert`:

```rust
                // Leave trackpads / continuous sources alone.
                let scroll_phase = event.get_double_value_field(FIELD_SCROLL_PHASE);
                let momentum_phase = event.get_double_value_field(FIELD_MOMENTUM_PHASE);
                let scroll_count = event.get_integer_value_field(FIELD_SCROLL_COUNT);
                if scroll::is_trackpad(scroll_phase, momentum_phase, scroll_count) {
                    return CallbackResult::Keep;
                }
```

- [ ] **Step 2: Build**

Run: `cargo build -p openlogi-hook`
Expected: clean build.

- [ ] **Step 3: Manual smoke test**

With a trackpad: two-finger scroll is unaffected (not inverted, not smoothed). With the mouse wheel: inversion + smoothing still apply. (If you have no trackpad, verify the field reads don't break mouse scrolling.)

- [ ] **Step 4: Commit**

```bash
git add crates/openlogi-hook/src/macos.rs
git commit -m "feat(hook): exclude trackpad / continuous events from the scroll engine"
```

---

## Task 7: GUI scroll panel + live wiring + persistence

A panel modeled on `components/dpi_panel.rs`: toggles for smooth / invert-vertical / invert-horizontal / smooth-vertical / smooth-horizontal, and sliders for speed / step / duration / dead-zone. Every change updates an in-memory `ScrollSettings`, pushes it into the running `Hook` (live), and persists via `Config::set_scroll_settings` + `save_atomic`.

> GPUI render code is verified by running the app, not by unit tests. Before writing, **read** `crates/openlogi-gui/src/components/dpi_panel.rs`, `crates/openlogi-gui/src/hook_runtime.rs`, `crates/openlogi-gui/src/app.rs`, and `crates/openlogi-gui/src/state.rs` to match the local `AppState`/panel conventions and the gpui-component widgets in use (`Slider`/`SliderState`; use the project's existing toggle/switch widget — grep `gpui_component` for `Switch`).

**Files:**
- Modify: `crates/openlogi-gui/src/hook_runtime.rs` — return/retain the `Hook` so the GUI can call `set_scroll_settings` (it already returns `Option<Hook>`; ensure the owner stores it where the panel can reach it, e.g. via `AppState` or an `Arc<Mutex<Option<Hook>>>` shared handle).
- Create: `crates/openlogi-gui/src/components/scroll_panel.rs`
- Modify: `crates/openlogi-gui/src/components.rs` — `pub mod scroll_panel;`
- Modify: `crates/openlogi-gui/src/app.rs` — add a `scroll_panel: Entity<ScrollPanel>` field, build it in `AppView::new`, render it in the body next to `dpi_panel`.

- [ ] **Step 1: Expose the live hook handle**

In the GUI startup (where `hook_runtime::start` is called — grep `hook_runtime::start`), store the returned `Hook` in a shared handle the panel can access. Minimal approach: a process-global `OnceLock<Mutex<Option<Hook>>>` in `hook_runtime.rs`:

```rust
use std::sync::{Mutex, OnceLock};
static LIVE_HOOK: OnceLock<Mutex<Option<Hook>>> = OnceLock::new();

/// Store the running hook so settings can be pushed to it live.
pub fn set_live_hook(hook: Option<Hook>) {
    let cell = LIVE_HOOK.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = cell.lock() {
        *g = hook;
    }
}

/// Push scroll settings to the running hook, if any.
pub fn push_scroll_settings(settings: openlogi_core::config::ScrollSettings) {
    if let Some(cell) = LIVE_HOOK.get() {
        if let Ok(g) = cell.lock() {
            if let Some(h) = g.as_ref() {
                h.set_scroll_settings(settings);
            }
        }
    }
}
```

At startup, after `let hook = hook_runtime::start(...);`, call `hook_runtime::set_live_hook(hook);`. (Adjust if the app already retains the `Hook` elsewhere — if so, add `push_scroll_settings` against that owner instead and skip the `OnceLock`.)

On startup also push the persisted settings once so behavior is active before the panel opens:

```rust
hook_runtime::push_scroll_settings(config.scroll_settings());
```

- [ ] **Step 2: Write the panel**

Create `crates/openlogi-gui/src/components/scroll_panel.rs`. Hold a working `ScrollSettings` copy; on any control change, mutate it, call `hook_runtime::push_scroll_settings(self.settings.clone())`, and persist. Persistence helper (load → set → save, off the UI hot path is unnecessary here — the file write is small):

```rust
fn persist(settings: &openlogi_core::config::ScrollSettings) {
    match openlogi_core::config::Config::load_or_default() {
        Ok(mut cfg) => {
            cfg.set_scroll_settings(settings.clone());
            if let Err(e) = cfg.save_atomic() {
                tracing::warn!(error = %e, "failed to persist scroll settings");
            }
        }
        Err(e) => tracing::warn!(error = %e, "failed to load config to persist scroll settings"),
    }
}
```

Render (sketch — match `dpi_panel.rs` idioms for `v_flex`, labels, `Slider`, and the project's toggle widget; ranges: speed `1..=10`, step `0.01..=100`, duration `1..=5`, dead-zone `0.1..=10`):

```rust
//! Global scroll settings: direction inversion + Mos-style smooth scrolling.
//! Edits push live to the running hook and persist to config.

use gpui::{Context, IntoElement, ParentElement, Render, Styled, Window, div, px};
use gpui_component::{v_flex, h_flex /*, slider::*, Switch */};
use openlogi_core::config::ScrollSettings;
use crate::hook_runtime;
use crate::theme;

const PANEL_W: f32 = 300.;

pub struct ScrollPanel {
    settings: ScrollSettings,
    // ... SliderState entities + Subscriptions, mirroring DpiPanel ...
}

impl ScrollPanel {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let settings = openlogi_core::config::Config::load_or_default()
            .map(|c| c.scroll_settings())
            .unwrap_or_default();
        Self { settings /* , slider states built lazily in render */ }
    }

    fn on_change(&mut self) {
        hook_runtime::push_scroll_settings(self.settings.clone());
        super::scroll_panel::persist(&self.settings);
    }
}

impl Render for ScrollPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = theme::palette(cx);
        v_flex()
            .gap_3()
            .w(px(PANEL_W))
            .child(div().text_sm().text_color(pal.text_muted).child("SCROLL"))
            // toggle row: Smooth scrolling -> sets self.settings.smooth, then self.on_change()
            // toggle row: Invert wheel (vertical) -> reverse_vertical
            // toggle row: Invert thumb wheel (horizontal) -> reverse_horizontal
            // toggle row: Smooth vertical / Smooth horizontal
            // slider row: Speed (1..=10), Step (0.01..=100), Duration (1..=5), Dead zone (0.1..=10)
            // ...build with the same Slider/SliderState pattern as dpi_panel.rs...
    }
}
```

Implement each toggle's `on_click`/each slider's `Release` to set the matching field on `self.settings` and call `self.on_change()`. Follow `dpi_panel.rs`'s `SliderState`/`subscribe` pattern (write on `Release`, not every `Change`).

- [ ] **Step 3: Register and mount the panel**

In `crates/openlogi-gui/src/components.rs` add `pub mod scroll_panel;`.

In `crates/openlogi-gui/src/app.rs`: add `scroll_panel: gpui::Entity<crate::components::scroll_panel::ScrollPanel>` to `AppView`, build it in `AppView::new` (`cx.new(crate::components::scroll_panel::ScrollPanel::new)`), and render it in the right-side config column next to `self.dpi_panel` (mirror the existing `.child(self.dpi_panel.clone())` wiring).

- [ ] **Step 4: Build + full gate**

Run:
```bash
cargo build -p openlogi-gui
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected: clean build, no clippy warnings, all tests pass.

- [ ] **Step 5: Manual smoke test**

Run `cargo run -p openlogi-gui --release`. In the panel:
- Toggle *Invert wheel* → vertical scroll reverses immediately (live).
- Toggle *Invert thumb wheel* → horizontal reverses independently.
- Toggle *Smooth* off → scrolling becomes steppy; on → smooth.
- Drag *Speed* / *Duration* → feel changes accordingly.
- Quit and relaunch → settings persisted and active before opening the panel (`config.toml` has an `[app_settings.scroll]` section).

- [ ] **Step 6: Commit**

```bash
git add crates/openlogi-gui/src/components/scroll_panel.rs \
        crates/openlogi-gui/src/components.rs \
        crates/openlogi-gui/src/app.rs \
        crates/openlogi-gui/src/hook_runtime.rs
git commit -m "feat(gui): scroll settings panel (invert + smooth) with live + persisted state"
```

---

## Final verification

- [ ] Run the full pre-commit gate once more from the repo root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- [ ] Confirm `docs/FORK-DIVERGENCE.md` does not need an entry (this is new fork-local functionality, not an upstream-tracking change) — if the maintainer wants it logged, add a one-line note there.

- [ ] Update `CLAUDE.md` §5 gotchas if the smooth engine introduces a non-obvious trap worth recording (e.g. "the scroll tap swallows wheel events when smoothing is on — `MouseEvent::Scroll` no longer reaches `hook_runtime`").
