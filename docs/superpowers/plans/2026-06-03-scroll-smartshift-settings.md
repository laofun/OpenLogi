# Scroll + SmartShift Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add user-facing scroll/wheel documentation, per-axis scroll speed/step tuning, and persistent SmartShift ratchet-mode + 0–100% sensitivity controls.

**Architecture:** Keep scroll behavior app-wide in `openlogi-core::config::ScrollSettings`, live-pushed to the macOS hook through the existing `hook_runtime` path. Keep SmartShift settings device-scoped in `DeviceConfig`, and apply them through `openlogi-hid`/`openlogi-gui::hardware` background workers so the GPUI thread and hook callback never block on HID++ I/O. Rework `ScrollPanel` into grouped sections while preserving its live-push + config-persist pattern.

**Tech Stack:** Rust 2024, serde/TOML config, GPUI + gpui-component sliders/switches, HID++ SmartShift features (`0x2111` + legacy `0x2110` fallback), tracing, cargo test/clippy/fmt.

---

## File structure

- Modify `crates/openlogi-core/src/config.rs`
  - Add per-axis scroll fields with backward-compatible load from old `speed`/`step`.
  - Add `smartshift_ratchet_mode: Option<bool>` to `DeviceConfig`.
  - Add config accessors for SmartShift ratchet mode.
  - Add tests for config migration/round-trip.
- Modify `crates/openlogi-hook/src/scroll.rs`
  - Add an internal `AxisTuning` struct.
  - Change `SmoothEngine::add` and `SharedSmooth::push` to use per-axis speed/step.
  - Add tests proving x/y tuning is independent.
- Modify `crates/openlogi-hook/src/macos.rs`
  - Pass `AxisTuning` built from `ScrollSettings` into `SharedSmooth::push`.
- Modify `crates/openlogi-hid/src/write.rs`
  - Add explicit `set_smartshift_mode` / `set_smartshift_mode_on` helpers so persisted desired state never relies on a blind toggle.
- Modify `crates/openlogi-hid/src/lib.rs`
  - Re-export the new SmartShift mode setters.
- Modify `crates/openlogi-gui/src/hardware.rs`
  - Add background worker for explicit SmartShift mode writes.
  - Add percent/raw sensitivity mapping helpers and tests.
- Modify `crates/openlogi-gui/src/state.rs`
  - Track pending SmartShift mode+sensitivity applies per connection.
  - Add methods for reading/committing active-device SmartShift settings.
- Modify `crates/openlogi-gui/src/main.rs`
  - Dispatch pending SmartShift mode+sensitivity writes from inventory refresh.
- Modify `crates/openlogi-gui/src/components/scroll_panel.rs`
  - Replace flat controls with grouped sections.
  - Add vertical/horizontal speed/step sliders.
  - Add device-scoped SmartShift ratchet-mode toggle and sensitivity percent slider.
- Create `docs/SCROLL_AND_WHEEL.md`
  - User-facing guide for scroll/smooth and SmartShift settings.
- Modify `docs/CONFIGURATION.md`
  - Document `[app_settings.scroll]`, device SmartShift fields, and link to the guide.

---

### Task 1: Scroll config migration and per-axis fields

**Files:**
- Modify: `crates/openlogi-core/src/config.rs:130-207`
- Modify tests: `crates/openlogi-core/src/config.rs:526-566`

- [ ] **Step 1: Write failing tests for per-axis defaults, old config migration, and new round-trip**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/openlogi-core/src/config.rs`, replacing the current `scroll_settings_default_matches_mos` and `scroll_settings_roundtrip` expectations that refer to `speed`/`step`:

```rust
#[test]
fn scroll_settings_default_matches_mos_per_axis() {
    let s = ScrollSettings::default();
    assert!(s.smooth);
    assert!(s.reverse_vertical);
    assert!(s.reverse_horizontal);
    assert!(s.smooth_vertical);
    assert!(s.smooth_horizontal);
    assert!((s.vertical_speed - 2.70).abs() < f64::EPSILON);
    assert!((s.horizontal_speed - 2.70).abs() < f64::EPSILON);
    assert!((s.vertical_step - 33.6).abs() < f64::EPSILON);
    assert!((s.horizontal_step - 33.6).abs() < f64::EPSILON);
    assert!((s.duration - 4.35).abs() < f64::EPSILON);
    assert!((s.dead_zone - 1.00).abs() < f64::EPSILON);
}

#[test]
fn scroll_settings_roundtrip_per_axis() {
    let mut cfg = Config::default();
    let s = ScrollSettings {
        smooth: false,
        reverse_horizontal: false,
        vertical_speed: 4.0,
        horizontal_speed: 6.0,
        vertical_step: 20.0,
        horizontal_step: 44.0,
        ..ScrollSettings::default()
    };
    cfg.set_scroll_settings(s.clone());
    let parsed = write_and_read(&cfg);
    let got = parsed.scroll_settings();
    assert!(!got.smooth);
    assert!(!got.reverse_horizontal);
    assert!((got.vertical_speed - 4.0).abs() < f64::EPSILON);
    assert!((got.horizontal_speed - 6.0).abs() < f64::EPSILON);
    assert!((got.vertical_step - 20.0).abs() < f64::EPSILON);
    assert!((got.horizontal_step - 44.0).abs() < f64::EPSILON);
    assert!(got.reverse_vertical);
}

#[test]
fn old_scroll_speed_and_step_load_into_both_axes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        r#"schema_version = 1

[app_settings.scroll]
speed = 5.5
step = 12.5
"#,
    )
    .expect("write");

    let cfg = Config::load_from_path(&path).expect("load");
    let got = cfg.scroll_settings();
    assert!((got.vertical_speed - 5.5).abs() < f64::EPSILON);
    assert!((got.horizontal_speed - 5.5).abs() < f64::EPSILON);
    assert!((got.vertical_step - 12.5).abs() < f64::EPSILON);
    assert!((got.horizontal_step - 12.5).abs() < f64::EPSILON);
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test -p openlogi-core scroll_settings_default_matches_mos_per_axis scroll_settings_roundtrip_per_axis old_scroll_speed_and_step_load_into_both_axes
```

Expected: FAIL to compile because `ScrollSettings` does not yet have `vertical_speed`, `horizontal_speed`, `vertical_step`, or `horizontal_step` fields.

- [ ] **Step 3: Implement backward-compatible `ScrollSettings` deserialization**

In `crates/openlogi-core/src/config.rs`, replace the `ScrollSettings` derive block with manual `Deserialize` support. Keep `Serialize` derived for the new fields:

```rust
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "these are independent user toggles, not a state machine"
)]
pub struct ScrollSettings {
    /// Master switch for software smoothing; `false` passes events through raw.
    #[serde(default = "default_true")]
    pub smooth: bool,
    /// Invert the vertical scroll direction.
    #[serde(default = "default_true")]
    pub reverse_vertical: bool,
    /// Invert the horizontal scroll direction.
    #[serde(default = "default_true")]
    pub reverse_horizontal: bool,
    /// Apply smoothing to vertical scroll events.
    #[serde(default = "default_true")]
    pub smooth_vertical: bool,
    /// Apply smoothing to horizontal scroll events.
    #[serde(default = "default_true")]
    pub smooth_horizontal: bool,
    /// Vertical-wheel scroll speed multiplier.
    #[serde(default = "default_scroll_speed")]
    pub vertical_speed: f64,
    /// Thumb-wheel / horizontal scroll speed multiplier.
    #[serde(default = "default_scroll_speed")]
    pub horizontal_speed: f64,
    /// Vertical-wheel per-notch scroll step size, in pixels.
    #[serde(default = "default_scroll_step")]
    pub vertical_step: f64,
    /// Thumb-wheel / horizontal per-notch scroll step size, in pixels.
    #[serde(default = "default_scroll_step")]
    pub horizontal_step: f64,
    /// Smoothing animation duration multiplier.
    #[serde(default = "default_scroll_duration")]
    pub duration: f64,
    /// Minimum delta below which a scroll event is ignored.
    #[serde(default = "default_scroll_dead_zone")]
    pub dead_zone: f64,
}

#[derive(Deserialize)]
struct ScrollSettingsWire {
    #[serde(default = "default_true")]
    smooth: bool,
    #[serde(default = "default_true")]
    reverse_vertical: bool,
    #[serde(default = "default_true")]
    reverse_horizontal: bool,
    #[serde(default = "default_true")]
    smooth_vertical: bool,
    #[serde(default = "default_true")]
    smooth_horizontal: bool,
    #[serde(default)]
    speed: Option<f64>,
    #[serde(default)]
    step: Option<f64>,
    #[serde(default)]
    vertical_speed: Option<f64>,
    #[serde(default)]
    horizontal_speed: Option<f64>,
    #[serde(default)]
    vertical_step: Option<f64>,
    #[serde(default)]
    horizontal_step: Option<f64>,
    #[serde(default = "default_scroll_duration")]
    duration: f64,
    #[serde(default = "default_scroll_dead_zone")]
    dead_zone: f64,
}

impl<'de> Deserialize<'de> for ScrollSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ScrollSettingsWire::deserialize(deserializer)?;
        let legacy_speed = wire.speed.unwrap_or_else(default_scroll_speed);
        let legacy_step = wire.step.unwrap_or_else(default_scroll_step);
        Ok(Self {
            smooth: wire.smooth,
            reverse_vertical: wire.reverse_vertical,
            reverse_horizontal: wire.reverse_horizontal,
            smooth_vertical: wire.smooth_vertical,
            smooth_horizontal: wire.smooth_horizontal,
            vertical_speed: wire.vertical_speed.unwrap_or(legacy_speed),
            horizontal_speed: wire.horizontal_speed.unwrap_or(legacy_speed),
            vertical_step: wire.vertical_step.unwrap_or(legacy_step),
            horizontal_step: wire.horizontal_step.unwrap_or(legacy_step),
            duration: wire.duration,
            dead_zone: wire.dead_zone,
        })
    }
}
```

Update `Default for ScrollSettings` to fill the four new per-axis fields:

```rust
impl Default for ScrollSettings {
    fn default() -> Self {
        Self {
            smooth: true,
            reverse_vertical: true,
            reverse_horizontal: true,
            smooth_vertical: true,
            smooth_horizontal: true,
            vertical_speed: default_scroll_speed(),
            horizontal_speed: default_scroll_speed(),
            vertical_step: default_scroll_step(),
            horizontal_step: default_scroll_step(),
            duration: default_scroll_duration(),
            dead_zone: default_scroll_dead_zone(),
        }
    }
}
```

- [ ] **Step 4: Update all code references from shared `speed`/`step` to per-axis fields**

Search:

```bash
rg "\.speed|\.step|speed:|step:" crates/openlogi-core/src crates/openlogi-hook/src crates/openlogi-gui/src
```

For compile-only call sites, temporarily use `vertical_speed` and `vertical_step` in this task; later tasks will wire axis-specific behavior. The key immediate replacements are:

- `settings.speed` -> `settings.vertical_speed` in old vertical-only UI code.
- `settings.step` -> `settings.vertical_step` in old vertical-only UI code.
- Struct literals in tests use the new field names.

- [ ] **Step 5: Run focused config tests and verify pass**

Run:

```bash
cargo test -p openlogi-core scroll_settings
```

Expected: PASS for all `scroll_settings...` tests.

- [ ] **Step 6: Commit**

```bash
git add crates/openlogi-core/src/config.rs
git commit -m "feat(core): split scroll speed and step per axis"
```

---

### Task 2: SmartShift config fields and accessors

**Files:**
- Modify: `crates/openlogi-core/src/config.rs:209-237,464-482`
- Test: `crates/openlogi-core/src/config.rs` test module

- [ ] **Step 1: Write failing config test for persisted ratchet mode**

Add this test in `crates/openlogi-core/src/config.rs` test module:

```rust
#[test]
fn smartshift_ratchet_mode_roundtrip_per_device() {
    let mut cfg = Config::default();
    cfg.set_smartshift_ratchet_mode("0b019", Some(true));
    cfg.set_smartshift_sensitivity("0b019", Some(25));
    cfg.set_smartshift_ratchet_mode("other", Some(false));

    let parsed = write_and_read(&cfg);

    assert_eq!(parsed.smartshift_ratchet_mode("0b019"), Some(true));
    assert_eq!(parsed.smartshift_sensitivity("0b019"), Some(25));
    assert_eq!(parsed.smartshift_ratchet_mode("other"), Some(false));
    assert_eq!(parsed.smartshift_ratchet_mode("missing"), None);
}
```

- [ ] **Step 2: Run test and verify it fails**

Run:

```bash
cargo test -p openlogi-core smartshift_ratchet_mode_roundtrip_per_device
```

Expected: FAIL to compile because `set_smartshift_ratchet_mode` and `smartshift_ratchet_mode` do not exist.

- [ ] **Step 3: Add `smartshift_ratchet_mode` to `DeviceConfig`**

In `DeviceConfig`, after `smartshift_sensitivity`, add:

```rust
/// Persisted desired SmartShift wheel mode. `Some(true)` = ratchet / SmartShift
/// mode; `Some(false)` = free-spin mode; `None` = leave firmware state untouched.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub smartshift_ratchet_mode: Option<bool>,
```

- [ ] **Step 4: Add config accessors**

In `impl Config`, after `set_smartshift_sensitivity`, add:

```rust
/// The persisted desired SmartShift mode for `device_key`. `Some(true)` means
/// ratchet / SmartShift mode, `Some(false)` means free-spin, and `None` means no
/// preference has been stored.
#[must_use]
pub fn smartshift_ratchet_mode(&self, device_key: &str) -> Option<bool> {
    self.devices
        .get(device_key)
        .and_then(|d| d.smartshift_ratchet_mode)
}

/// Set (or clear, with `None`) the desired SmartShift mode for `device_key`.
pub fn set_smartshift_ratchet_mode(&mut self, device_key: &str, value: Option<bool>) {
    self.devices
        .entry(device_key.to_string())
        .or_default()
        .smartshift_ratchet_mode = value;
}
```

- [ ] **Step 5: Run focused test and verify pass**

Run:

```bash
cargo test -p openlogi-core smartshift_ratchet_mode_roundtrip_per_device
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/openlogi-core/src/config.rs
git commit -m "feat(core): persist desired SmartShift wheel mode"
```

---

### Task 3: Per-axis smoothing engine tuning

**Files:**
- Modify: `crates/openlogi-hook/src/scroll.rs:45-170`
- Modify: `crates/openlogi-hook/src/macos.rs:291-306,450-464`

- [ ] **Step 1: Write failing tests for axis-specific speed and step**

Add this public helper struct near `SmoothEngine` in `scroll.rs` tests first if needed by tests:

```rust
#[test]
fn engine_scales_each_axis_with_its_own_speed() {
    let mut e = SmoothEngine::new();
    e.add(
        1.0,
        1.0,
        AxisTuning {
            horizontal_speed: 2.0,
            vertical_speed: 4.0,
            horizontal_step: 1.0,
            vertical_step: 1.0,
        },
    );
    let (dx, dy) = e.advance(1.0, 0.1).expect("frame");
    assert!((dx - 2.0).abs() < f64::EPSILON);
    assert!((dy - 4.0).abs() < f64::EPSILON);
}

#[test]
fn engine_normalizes_each_axis_with_its_own_step() {
    let mut e = SmoothEngine::new();
    e.add(
        0.5,
        0.5,
        AxisTuning {
            horizontal_speed: 1.0,
            vertical_speed: 1.0,
            horizontal_step: 3.0,
            vertical_step: 7.0,
        },
    );
    let (dx, dy) = e.advance(1.0, 0.1).expect("frame");
    assert!((dx - 3.0).abs() < f64::EPSILON);
    assert!((dy - 7.0).abs() < f64::EPSILON);
}
```

- [ ] **Step 2: Run focused tests and verify they fail**

Run:

```bash
cargo test -p openlogi-hook engine_scales_each_axis_with_its_own_speed engine_normalizes_each_axis_with_its_own_step
```

Expected: FAIL to compile because `AxisTuning` does not exist and `SmoothEngine::add` still takes shared speed/step.

- [ ] **Step 3: Add `AxisTuning` and update `SmoothEngine::add`**

In `crates/openlogi-hook/src/scroll.rs`, add near `SmoothEngine`:

```rust
/// Per-axis smoothing distance tuning. Horizontal corresponds to macOS scroll
/// axis 2 (thumb wheel); vertical corresponds to axis 1 (main wheel).
#[derive(Debug, Clone, Copy)]
pub struct AxisTuning {
    pub horizontal_speed: f64,
    pub vertical_speed: f64,
    pub horizontal_step: f64,
    pub vertical_step: f64,
}
```

Change `SmoothEngine::add` to:

```rust
/// Add an input tick (already inverted if applicable). `dx`/`dy` are the raw
/// event deltas; each axis is normalized to its own step then scaled by its own
/// speed.
pub fn add(&mut self, dx: f64, dy: f64, tuning: AxisTuning) {
    self.buffer.0 += normalize(dx, tuning.horizontal_step) * tuning.horizontal_speed;
    self.buffer.1 += normalize(dy, tuning.vertical_step) * tuning.vertical_speed;
}
```

- [ ] **Step 4: Update `SharedSmooth::push` signature**

Change `SharedSmooth::push` from shared `speed, step` to `tuning: AxisTuning`:

```rust
pub fn push(&self, dx: f64, dy: f64, tuning: AxisTuning, pid: i32, start: impl FnOnce()) {
    let Ok(mut st) = self.state.lock() else {
        return;
    };
    st.target_pid = pid;
    st.engine.add(dx, dy, tuning);
    if !st.running {
        st.running = true;
        start();
    }
}
```

Update existing tests in `scroll.rs` to pass this default tuning:

```rust
fn default_tuning() -> AxisTuning {
    AxisTuning {
        horizontal_speed: 1.0,
        vertical_speed: 1.0,
        horizontal_step: 0.0,
        vertical_step: 0.0,
    }
}
```

Replace calls like:

```rust
e.add(0.0, 10.0, 1.0, 0.0);
s.push(0.0, 10.0, 1.0, 0.0, 1234, || started.set(true));
```

with:

```rust
e.add(0.0, 10.0, default_tuning());
s.push(0.0, 10.0, default_tuning(), 1234, || started.set(true));
```

- [ ] **Step 5: Wire `macos.rs` to pass config per-axis tuning**

In `crates/openlogi-hook/src/macos.rs`, update the import:

```rust
use crate::scroll::{self, AxisTuning, MIN_DEAD_ZONE, SharedSmooth};
```

Update the `driver.shared.push(...)` call in `handle_scroll_event` to:

```rust
driver.shared.push(
    if smooth_h { dx } else { 0.0 },
    if smooth_v { dy } else { 0.0 },
    AxisTuning {
        horizontal_speed: cfg.horizontal_speed,
        vertical_speed: cfg.vertical_speed,
        horizontal_step: cfg.horizontal_step,
        vertical_step: cfg.vertical_step,
    },
    pid,
    || {
        // This push armed a previously idle engine — start the frame clock.
        // Running under the SharedSmooth lock orders this Start against a
        // concurrent settle's Stop so they cannot reorder.
        // SAFETY: link created in build_scroll_driver; CVDisplayLinkStart is
        // thread-safe and returns immediately without taking our mutex.
        unsafe { CVDisplayLinkStart(driver.link) };
    },
);
```

- [ ] **Step 6: Run openlogi-hook tests and verify pass**

Run:

```bash
cargo test -p openlogi-hook
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/openlogi-hook/src/scroll.rs crates/openlogi-hook/src/macos.rs
git commit -m "feat(hook): tune smooth scroll speed and step per axis"
```

---

### Task 4: Explicit SmartShift mode write path

**Files:**
- Modify: `crates/openlogi-hid/src/write.rs:386-455`
- Modify: `crates/openlogi-hid/src/lib.rs:35-39`
- Test: existing tests in `crates/openlogi-hid/src/smartshift_backend.rs`

- [ ] **Step 1: Write compile-facing tests for explicit mode helper shape**

In `crates/openlogi-hid/src/write.rs` test module, add a compile-only API test:

```rust
#[test]
fn smartshift_mode_setters_have_expected_api_shape() {
    fn _route_api(route: &DeviceRoute) {
        let fut = set_smartshift_mode(route, SmartShiftMode::Ratchet);
        std::mem::drop(fut);
    }
    fn _shared_api(shared: &SharedChannel) {
        let fut = set_smartshift_mode_on(shared, SmartShiftMode::Free);
        std::mem::drop(fut);
    }
}
```

- [ ] **Step 2: Run focused test and verify it fails**

Run:

```bash
cargo test -p openlogi-hid smartshift_mode_setters_have_expected_api_shape
```

Expected: FAIL to compile because `set_smartshift_mode` and `set_smartshift_mode_on` do not exist.

- [ ] **Step 3: Add route-level explicit mode setter**

In `crates/openlogi-hid/src/write.rs`, after `toggle_smartshift`, add:

```rust
/// Set SmartShift mode on `route` to a known desired state, preserving current
/// sensitivity. Use this for persisted settings; unlike `toggle_smartshift`, it
/// never depends on the current mode matching UI state.
pub async fn set_smartshift_mode(
    route: &DeviceRoute,
    mode: SmartShiftMode,
) -> Result<SmartShiftStatus, WriteError> {
    let index = route.device_index();
    with_route(route, move |channel| async move {
        set_smartshift_mode_on_channel(&channel, index, mode).await
    })
    .await
}

async fn set_smartshift_mode_on_channel(
    channel: &Arc<HidppChannel>,
    index: u8,
    mode: SmartShiftMode,
) -> Result<SmartShiftStatus, WriteError> {
    let mut device = Device::new(Arc::clone(channel), index)
        .await
        .map_err(|_| WriteError::DeviceUnreachable { index })?;
    let smartshift = SmartShift::open(&mut device).await?;
    let SmartShiftStatus { sensitivity, .. } = smartshift.status().await?;
    smartshift.set_mode(mode, sensitivity).await?;
    let status = smartshift.status().await?;
    debug!(index, ?mode, applied = ?status.mode, "wrote SmartShift mode");
    Ok(status)
}
```

- [ ] **Step 4: Add shared-channel explicit mode setter**

Near `toggle_smartshift_on`, add:

```rust
/// Set SmartShift mode on an already-open [`SharedChannel`].
pub async fn set_smartshift_mode_on(
    shared: &SharedChannel,
    mode: SmartShiftMode,
) -> Result<SmartShiftStatus, WriteError> {
    set_smartshift_mode_on_channel(&shared.channel, shared.route.device_index(), mode).await
}
```

- [ ] **Step 5: Re-export helpers**

In `crates/openlogi-hid/src/lib.rs`, update the `pub use write::{...}` list to include:

```rust
set_smartshift_mode, set_smartshift_mode_on,
```

- [ ] **Step 6: Run focused test and verify pass**

Run:

```bash
cargo test -p openlogi-hid smartshift_mode_setters_have_expected_api_shape
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/openlogi-hid/src/write.rs crates/openlogi-hid/src/lib.rs
git commit -m "feat(hid): set SmartShift mode explicitly"
```

---

### Task 5: GUI hardware helpers for SmartShift mode and percent mapping

**Files:**
- Modify: `crates/openlogi-gui/src/hardware.rs:15-158`

- [ ] **Step 1: Write failing tests for sensitivity percent mapping**

At the bottom of `crates/openlogi-gui/src/hardware.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smartshift_percent_maps_to_hid_range() {
        assert_eq!(smartshift_percent_to_raw(0), 1);
        assert_eq!(smartshift_percent_to_raw(100), 255);
        assert_eq!(smartshift_percent_to_raw(10), 26);
    }

    #[test]
    fn smartshift_raw_maps_to_percent() {
        assert_eq!(smartshift_raw_to_percent(1), 0);
        assert_eq!(smartshift_raw_to_percent(255), 100);
        assert_eq!(smartshift_raw_to_percent(25), 9);
    }
}
```

- [ ] **Step 2: Run focused tests and verify they fail**

Run:

```bash
cargo test -p openlogi-gui smartshift_percent_maps_to_hid_range smartshift_raw_maps_to_percent
```

Expected: FAIL to compile because mapping functions do not exist.

- [ ] **Step 3: Add mapping helpers**

In `hardware.rs`, near `WRITE_BUDGET`, add:

```rust
/// Convert user-facing SmartShift sensitivity percent to HID++ 1–255.
#[must_use]
pub fn smartshift_percent_to_raw(percent: u8) -> u8 {
    let clamped = u16::from(percent.min(100));
    let raw = 1 + ((clamped * 254 + 50) / 100);
    u8::try_from(raw).unwrap_or(255)
}

/// Convert HID++ 1–255 SmartShift sensitivity to a user-facing percent.
#[must_use]
pub fn smartshift_raw_to_percent(raw: u8) -> u8 {
    let raw = u16::from(raw.max(1));
    let percent = ((raw - 1) * 100 + 127) / 254;
    u8::try_from(percent).unwrap_or(100)
}
```

- [ ] **Step 4: Add explicit SmartShift mode background worker**

In imports, add `SmartShiftMode`:

```rust
use openlogi_hid::{CaptureChannel, DeviceRoute, DpiInfo, SharedChannel, SmartShiftMode, WriteError};
```

After `toggle_smartshift_in_background`, add:

```rust
/// Spawn an OS thread that sets SmartShift to a known mode on the target device,
/// preserving the current sensitivity. Returns immediately; failures are logged.
pub fn set_smartshift_mode_in_background(
    capture: Option<&CaptureChannel>,
    target: Option<DeviceRoute>,
    mode: SmartShiftMode,
) {
    let Some(target) = target else {
        debug!(?mode, "no target device — SmartShift mode set skipped");
        return;
    };
    let shared = reusable_channel(capture, &target);
    let reused = shared.is_some();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                warn!(error = %e, "tokio runtime init failed; SmartShift mode set skipped");
                return;
            }
        };
        let result = rt.block_on(async {
            tokio::time::timeout(WRITE_BUDGET, async {
                match &shared {
                    Some(shared) => openlogi_hid::set_smartshift_mode_on(shared, mode).await,
                    None => openlogi_hid::set_smartshift_mode(&target, mode).await,
                }
            })
            .await
        });
        let index = target.device_index();
        match result {
            Ok(Ok(status)) => debug!(index, ?mode, applied = ?status.mode, reused, "SmartShift mode set"),
            Ok(Err(e)) => warn!(error = ?e, "SmartShift mode set failed"),
            Err(_) => warn!(index, "SmartShift mode set timed out (device asleep/unresponsive)"),
        }
    });
}
```

- [ ] **Step 5: Run focused tests and verify pass**

Run:

```bash
cargo test -p openlogi-gui smartshift_percent_maps_to_hid_range smartshift_raw_maps_to_percent
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/openlogi-gui/src/hardware.rs
git commit -m "feat(gui): add SmartShift mode and percent helpers"
```

---

### Task 6: AppState SmartShift persistence/apply flow

**Files:**
- Modify: `crates/openlogi-gui/src/state.rs:138-144,273-344,514-523`
- Modify: `crates/openlogi-gui/src/main.rs:248-265`

- [ ] **Step 1: Write unit tests for pending SmartShift applies**

In `crates/openlogi-gui/src/state.rs` tests (create `#[cfg(test)] mod tests` at the bottom if none exists), add tests around a pure helper. First write the helper call as desired API:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn smartshift_pending_collects_mode_and_sensitivity_once() {
        let mut cfg = Config::default();
        cfg.set_smartshift_ratchet_mode("0b019", Some(true));
        cfg.set_smartshift_sensitivity("0b019", Some(25));
        let connected = vec!["0b019".to_string()];
        let applied = HashSet::new();

        let pending = smartshift_pending(&cfg, &connected, &applied);

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "0b019");
        assert_eq!(pending[0].1.ratchet_mode, Some(true));
        assert_eq!(pending[0].1.sensitivity, Some(25));
    }

    #[test]
    fn smartshift_pending_skips_already_evaluated_devices() {
        let mut cfg = Config::default();
        cfg.set_smartshift_ratchet_mode("0b019", Some(false));
        let connected = vec!["0b019".to_string()];
        let applied = HashSet::from(["0b019".to_string()]);

        let pending = smartshift_pending(&cfg, &connected, &applied);

        assert!(pending.is_empty());
    }
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
cargo test -p openlogi-gui smartshift_pending_collects_mode_and_sensitivity_once smartshift_pending_skips_already_evaluated_devices
```

Expected: FAIL to compile because `smartshift_pending` and its return type do not exist.

- [ ] **Step 3: Add `PendingSmartShift` type and helper**

In `state.rs`, near `DpiStatus`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingSmartShift {
    pub ratchet_mode: Option<bool>,
    pub sensitivity: Option<u8>,
}
```

Near existing `pending_smartshift_writes` helper area, add:

```rust
fn smartshift_pending(
    config: &Config,
    connected: &[String],
    applied: &HashSet<String>,
) -> Vec<(String, PendingSmartShift)> {
    connected
        .iter()
        .filter(|key| !applied.contains(*key))
        .filter_map(|key| {
            let pending = PendingSmartShift {
                ratchet_mode: config.smartshift_ratchet_mode(key),
                sensitivity: config.smartshift_sensitivity(key),
            };
            (pending.ratchet_mode.is_some() || pending.sensitivity.is_some())
                .then(|| (key.clone(), pending))
        })
        .collect()
}
```

- [ ] **Step 4: Replace pending sensitivity-only flow with mode+sensitivity flow**

Change `AppState::pending_smartshift_writes` signature to:

```rust
pub fn pending_smartshift_writes(&mut self) -> Vec<(DeviceRoute, PendingSmartShift)> {
```

Inside it, replace `let pending = smartshift_pending(&disk, &connected, &self.smartshift_applied);` usage with the new tuple values:

```rust
let pending = smartshift_pending(&disk, &connected, &self.smartshift_applied);

let mut writes = Vec::new();
for (key, pending) in pending {
    if let Some(mode) = pending.ratchet_mode {
        self.config.set_smartshift_ratchet_mode(&key, Some(mode));
    }
    if let Some(value) = pending.sensitivity {
        self.config.set_smartshift_sensitivity(&key, Some(value));
    }
    if let Some(route) = self
        .device_list
        .iter()
        .find(|r| r.config_key == key)
        .and_then(|r| r.route.clone())
    {
        writes.push((route, pending));
    }
}
```

Keep the existing connected-key pruning and applied marking.

- [ ] **Step 5: Add active-device SmartShift accessors/committers**

In `impl AppState`, near `commit_scroll_settings`, add:

```rust
#[must_use]
pub fn active_smartshift_ratchet_mode(&self) -> Option<bool> {
    self.current_record()
        .and_then(|record| self.config.smartshift_ratchet_mode(&record.config_key))
}

#[must_use]
pub fn active_smartshift_sensitivity(&self) -> Option<u8> {
    self.current_record()
        .and_then(|record| self.config.smartshift_sensitivity(&record.config_key))
}

pub fn commit_active_smartshift_ratchet_mode(&mut self, value: bool) {
    let Some(key) = self.current_record().map(|r| r.config_key.clone()) else {
        debug!("no active device key — SmartShift mode not persisted");
        return;
    };
    self.config.set_smartshift_ratchet_mode(&key, Some(value));
    if let Err(e) = self.config.save_atomic() {
        warn!(error = %e, "could not persist SmartShift mode");
    }
}

pub fn commit_active_smartshift_sensitivity(&mut self, value: u8) {
    let Some(key) = self.current_record().map(|r| r.config_key.clone()) else {
        debug!("no active device key — SmartShift sensitivity not persisted");
        return;
    };
    self.config.set_smartshift_sensitivity(&key, Some(value));
    if let Err(e) = self.config.save_atomic() {
        warn!(error = %e, "could not persist SmartShift sensitivity");
    }
}
```

- [ ] **Step 6: Dispatch mode and sensitivity writes in `main.rs`**

Update the inventory refresh dispatch block from:

```rust
for (route, value) in writes {
    hardware::apply_smartshift_sensitivity_in_background(Some(route), value);
}
```

to:

```rust
for (route, pending) in writes {
    if let Some(ratchet_mode) = pending.ratchet_mode {
        let mode = if ratchet_mode {
            openlogi_hid::SmartShiftMode::Ratchet
        } else {
            openlogi_hid::SmartShiftMode::Free
        };
        hardware::set_smartshift_mode_in_background(None, Some(route.clone()), mode);
    }
    if let Some(value) = pending.sensitivity {
        hardware::apply_smartshift_sensitivity_in_background(Some(route), value);
    }
}
```

- [ ] **Step 7: Run focused tests and verify pass**

Run:

```bash
cargo test -p openlogi-gui smartshift_pending_collects_mode_and_sensitivity_once smartshift_pending_skips_already_evaluated_devices
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/openlogi-gui/src/state.rs crates/openlogi-gui/src/main.rs
git commit -m "feat(gui): persist and apply SmartShift wheel mode"
```

---

### Task 7: Rework ScrollPanel UI with grouped scroll controls

**Files:**
- Modify: `crates/openlogi-gui/src/components/scroll_panel.rs:29-288`

- [ ] **Step 1: Update `ScrollPanel` state fields**

Replace the four slider fields:

```rust
speed: Entity<SliderState>,
step: Entity<SliderState>,
duration: Entity<SliderState>,
dead_zone: Entity<SliderState>,
```

with six scroll sliders and one SmartShift slider:

```rust
vertical_speed: Entity<SliderState>,
horizontal_speed: Entity<SliderState>,
vertical_step: Entity<SliderState>,
horizontal_step: Entity<SliderState>,
duration: Entity<SliderState>,
dead_zone: Entity<SliderState>,
smartshift_sensitivity: Entity<SliderState>,
```

- [ ] **Step 2: Build slider states from per-axis settings**

In `new`, create sliders with the same ranges as existing speed/step and a 0–100 range for SmartShift:

```rust
let vertical_speed = scroll_slider(cx, 1.0, 10.0, 0.1, settings.vertical_speed);
let horizontal_speed = scroll_slider(cx, 1.0, 10.0, 0.1, settings.horizontal_speed);
let vertical_step = scroll_slider(cx, 0.01, 100.0, 0.5, settings.vertical_step);
let horizontal_step = scroll_slider(cx, 0.01, 100.0, 0.5, settings.horizontal_step);
let duration = scroll_slider(cx, 1.0, 5.0, 0.05, settings.duration);
let dead_zone = scroll_slider(
    cx,
    openlogi_hook::MIN_DEAD_ZONE,
    10.0,
    0.1,
    settings.dead_zone,
);
let smartshift_raw = cx
    .try_global::<crate::state::AppState>()
    .and_then(|s| s.active_smartshift_sensitivity())
    .unwrap_or(25);
let smartshift_sensitivity = cx.new(|_| {
    SliderState::new()
        .max(100.0)
        .min(0.0)
        .step(1.0)
        .default_value(f32::from(crate::hardware::smartshift_raw_to_percent(smartshift_raw)))
});
```

Add helper:

```rust
fn scroll_slider(
    cx: &mut Context<ScrollPanel>,
    min: f64,
    max: f64,
    step: f64,
    value: f64,
) -> Entity<SliderState> {
    cx.new(|_| {
        SliderState::new()
            .max(f64_to_f32(max))
            .min(f64_to_f32(min))
            .step(f64_to_f32(step))
            .default_value(f64_to_f32(value))
    })
}
```

- [ ] **Step 3: Subscribe six scroll sliders**

Replace old four subscriptions with six scroll subscriptions:

```rust
let mut subs = Vec::with_capacity(7);
subs.push(cx.subscribe(&vertical_speed, |this, _slider, event: &SliderEvent, cx| {
    if let SliderEvent::Release(v) = event {
        this.settings.vertical_speed = f64::from(v.start());
        this.on_change(cx);
        cx.notify();
    }
}));
subs.push(cx.subscribe(&horizontal_speed, |this, _slider, event: &SliderEvent, cx| {
    if let SliderEvent::Release(v) = event {
        this.settings.horizontal_speed = f64::from(v.start());
        this.on_change(cx);
        cx.notify();
    }
}));
subs.push(cx.subscribe(&vertical_step, |this, _slider, event: &SliderEvent, cx| {
    if let SliderEvent::Release(v) = event {
        this.settings.vertical_step = f64::from(v.start());
        this.on_change(cx);
        cx.notify();
    }
}));
subs.push(cx.subscribe(&horizontal_step, |this, _slider, event: &SliderEvent, cx| {
    if let SliderEvent::Release(v) = event {
        this.settings.horizontal_step = f64::from(v.start());
        this.on_change(cx);
        cx.notify();
    }
}));
```

Keep duration/dead-zone subscriptions with updated field names. Add SmartShift sensitivity release:

```rust
subs.push(cx.subscribe(
    &smartshift_sensitivity,
    |_this, _slider, event: &SliderEvent, cx| {
        if let SliderEvent::Release(v) = event {
            let percent = f32_to_percent(v.start());
            let raw = crate::hardware::smartshift_percent_to_raw(percent);
            let target = cx
                .try_global::<crate::state::AppState>()
                .and_then(|s| s.current_record().and_then(|r| r.route.clone()));
            cx.update_global::<crate::state::AppState, _>(|state, _| {
                state.commit_active_smartshift_sensitivity(raw);
            });
            crate::hardware::apply_smartshift_sensitivity_in_background(target, raw);
            cx.notify();
        }
    },
));
```

Add helper:

```rust
fn f32_to_percent(v: f32) -> u8 {
    v.round().clamp(0.0, 100.0) as u8
}
```

- [ ] **Step 4: Update reset-to-defaults to reset six scroll sliders only**

Change the `thumbs` array to include the six scroll settings and exclude SmartShift:

```rust
let thumbs = [
    (&self.vertical_speed, defaults.vertical_speed),
    (&self.horizontal_speed, defaults.horizontal_speed),
    (&self.vertical_step, defaults.vertical_step),
    (&self.horizontal_step, defaults.horizontal_step),
    (&self.duration, defaults.duration),
    (&self.dead_zone, defaults.dead_zone),
];
```

- [ ] **Step 5: Add section heading helper**

Add near `slider_row`:

```rust
fn section_label(label: SharedString, pal: theme::Palette) -> impl IntoElement {
    div().text_xs().text_color(pal.text_muted).child(label)
}
```

- [ ] **Step 6: Add SmartShift render helpers**

Add helper to render disabled slider text when ratchet mode is off:

```rust
fn smartshift_snapshot(cx: &mut Context<ScrollPanel>) -> (bool, u8, Option<openlogi_hid::DeviceRoute>) {
    cx.try_global::<crate::state::AppState>()
        .and_then(|s| {
            let route = s.current_record().and_then(|r| r.route.clone());
            Some((
                s.active_smartshift_ratchet_mode().unwrap_or(false),
                s.active_smartshift_sensitivity().unwrap_or(25),
                route,
            ))
        })
        .unwrap_or((false, 25, None))
}
```

- [ ] **Step 7: Rework `render` into grouped sections**

Replace the flat `.child(...)` chain in `render` with grouped sections:

```rust
let (ratchet_mode, smartshift_raw, target) = smartshift_snapshot(cx);
let smartshift_percent = crate::hardware::smartshift_raw_to_percent(smartshift_raw);

v_flex()
    .gap_3()
    .w(px(PANEL_W))
    .child(div().text_sm().text_color(pal.text_muted).child("SCROLL"))
    .child(
        v_flex()
            .gap_2()
            .child(section_label(tr!("VERTICAL WHEEL"), pal))
            .child(Self::toggle_row(
                "scroll-invert-v",
                tr!("Invert vertical"),
                self.settings.reverse_vertical,
                |s, v| s.reverse_vertical = v,
                pal,
                cx,
            ))
            .child(Self::toggle_row(
                "scroll-smooth-v",
                tr!("Smooth vertical"),
                self.settings.smooth_vertical,
                |s, v| s.smooth_vertical = v,
                pal,
                cx,
            ))
            .child(slider_row(tr!("Speed"), self.settings.vertical_speed, &self.vertical_speed, pal))
            .child(slider_row(tr!("Step"), self.settings.vertical_step, &self.vertical_step, pal)),
    )
    .child(
        v_flex()
            .gap_2()
            .child(section_label(tr!("THUMB WHEEL / HORIZONTAL"), pal))
            .child(Self::toggle_row(
                "scroll-invert-h",
                tr!("Invert horizontal"),
                self.settings.reverse_horizontal,
                |s, v| s.reverse_horizontal = v,
                pal,
                cx,
            ))
            .child(Self::toggle_row(
                "scroll-smooth-h",
                tr!("Smooth horizontal"),
                self.settings.smooth_horizontal,
                |s, v| s.smooth_horizontal = v,
                pal,
                cx,
            ))
            .child(slider_row(tr!("Speed"), self.settings.horizontal_speed, &self.horizontal_speed, pal))
            .child(slider_row(tr!("Step"), self.settings.horizontal_step, &self.horizontal_step, pal)),
    )
    .child(
        v_flex()
            .gap_2()
            .child(section_label(tr!("SMOOTH FEEL"), pal))
            .child(Self::toggle_row(
                "scroll-smooth",
                tr!("Smooth scrolling"),
                self.settings.smooth,
                |s, v| s.smooth = v,
                pal,
                cx,
            ))
            .child(slider_row(tr!("Duration"), self.settings.duration, &self.duration, pal))
            .child(slider_row(tr!("Dead zone"), self.settings.dead_zone, &self.dead_zone, pal)),
    )
    .child(
        v_flex()
            .gap_2()
            .child(section_label(tr!("SMARTSHIFT"), pal))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(div().text_sm().text_color(pal.text_primary).child(tr!("Ratchet mode")))
                    .child(Switch::new("smartshift-ratchet").checked(ratchet_mode).on_click(cx.listener(
                        move |_this, checked: &bool, _window, cx| {
                            let mode = if *checked {
                                openlogi_hid::SmartShiftMode::Ratchet
                            } else {
                                openlogi_hid::SmartShiftMode::Free
                            };
                            cx.update_global::<crate::state::AppState, _>(|state, _| {
                                state.commit_active_smartshift_ratchet_mode(*checked);
                            });
                            crate::hardware::set_smartshift_mode_in_background(None, target.clone(), mode);
                            cx.notify();
                        },
                    )))
            )
            .child(smartshift_slider_row(
                tr!("SmartShift sensitivity"),
                smartshift_percent,
                &self.smartshift_sensitivity,
                ratchet_mode,
                pal,
            )),
    )
    .child(/* existing reset action */)
```

Add `smartshift_slider_row`:

```rust
fn smartshift_slider_row(
    label: SharedString,
    percent: u8,
    state: &Entity<SliderState>,
    enabled: bool,
    pal: theme::Palette,
) -> impl IntoElement {
    let color = if enabled { pal.text_muted } else { pal.text_disabled };
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .items_baseline()
                .child(div().text_sm().text_color(color).child(label))
                .child(div().text_sm().text_color(color).child(format!("{percent}%"))),
        )
        .child(
            div()
                .opacity(if enabled { 1.0 } else { 0.45 })
                .child(Slider::new(state).horizontal()),
        )
}
```

If `Palette` lacks `text_disabled`, use `pal.text_muted` for both enabled and disabled and rely on opacity.

- [ ] **Step 8: Compile and fix GPUI API mismatches minimally**

Run:

```bash
cargo check -p openlogi-gui
```

Expected: It may fail on exact GPUI styling method names or capture lifetimes. Fix only compile errors, preserving the structure above.

- [ ] **Step 9: Commit**

```bash
git add crates/openlogi-gui/src/components/scroll_panel.rs
git commit -m "feat(gui): group scroll controls by wheel axis"
```

---

### Task 8: Documentation for scroll/wheel settings

**Files:**
- Create: `docs/SCROLL_AND_WHEEL.md`
- Modify: `docs/CONFIGURATION.md:11-35`

- [ ] **Step 1: Create user-facing guide**

Create `docs/SCROLL_AND_WHEEL.md` with:

```markdown
# Scroll and Wheel Settings

OpenLogi's scroll settings are software-side macOS event-tap settings. They do
not change Logitech firmware scroll flags. The app needs macOS Accessibility
permission so it can observe wheel events, transform them, and pass them on.

## Wheel axes

- **Vertical wheel** is macOS scroll axis 1.
- **Thumb wheel / horizontal** is macOS scroll axis 2.

The two axes can be inverted and smoothed independently.

## Invert

Invert flips the selected axis direction before the event reaches the foreground
app. Vertical and horizontal inversion are independent, so changing thumb-wheel
direction does not affect the main wheel.

## Smooth scrolling

When smoothing is enabled for an axis, OpenLogi drops the original wheel event
and re-emits interpolated synthetic scroll frames at the HID tap location. The
synthetic frames are marked so OpenLogi's tap passes them through instead of
feeding them back into the smoothing engine.

Trackpads and already-continuous scroll sources are ignored by this engine.
Their phase/count fields indicate that macOS is already handling continuous
scrolling and momentum.

## Tuning

Each axis has its own distance tuning:

- **Speed** controls the distance multiplier for that wheel.
- **Step** controls the minimum effective scroll quantum for that wheel.

The smoothing feel is shared:

- **Duration** controls how long the interpolation tail feels.
- **Dead zone** controls when the tiny tail is considered finished.

## SmartShift

SmartShift settings are device-scoped and only apply to devices exposing the
SmartShift HID++ feature.

- **Ratchet mode on** sets the wheel to ratchet / SmartShift mode and enables the
  sensitivity slider.
- **Ratchet mode off** sets the wheel to free-spin mode and disables the
  sensitivity slider.
- **SmartShift sensitivity** is shown as `0–100%` in the UI and mapped linearly
  to the HID++ `1–255` threshold. For reference, `10%` is about `26/255`.

Persisted SmartShift mode and sensitivity are applied again when the device
reconnects or the app starts.
```

- [ ] **Step 2: Update `docs/CONFIGURATION.md`**

Replace the paragraph at lines 11-15 with:

```markdown
Per-device settings are keyed by the HID++ identifier (e.g. `2b042` for an
MX Master 4): `button_bindings`, `per_app_bindings` (keyed by bundle id such as
`com.microsoft.VSCode`), `gesture_bindings`, `dpi_presets`, and SmartShift
preferences. The app-wide `[app_settings]` block holds launch/update/menu-bar
preferences plus global scroll settings. See [Scroll and Wheel Settings](SCROLL_AND_WHEEL.md)
for the meaning of the scroll and SmartShift fields.
```

Extend the TOML example to include:

```toml
[app_settings.scroll]
smooth = true
reverse_vertical = true
reverse_horizontal = true
smooth_vertical = true
smooth_horizontal = true
vertical_speed = 2.7
horizontal_speed = 2.7
vertical_step = 33.6
horizontal_step = 33.6
duration = 4.35
dead_zone = 1.0

[devices.2b042]
dpi_presets = [800, 1600, 3200]
smartshift_ratchet_mode = true
smartshift_sensitivity = 25
```

- [ ] **Step 3: Check docs render basics**

Run:

```bash
rg "SCROLL_AND_WHEEL|vertical_speed|smartshift_ratchet_mode" docs/CONFIGURATION.md docs/SCROLL_AND_WHEEL.md
```

Expected: output contains all three terms.

- [ ] **Step 4: Commit**

```bash
git add docs/SCROLL_AND_WHEEL.md docs/CONFIGURATION.md
git commit -m "docs: document scroll and SmartShift settings"
```

---

### Task 9: Full verification and manual smoke checklist

**Files:**
- No code changes expected.

- [ ] **Step 1: Run formatting check**

Run:

```bash
cargo fmt --all -- --check
```

Expected: exit 0.

- [ ] **Step 2: Run clippy gate**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: exit 0. The known future-incompat warning for dependency `block v0.1.6` may appear after successful compilation; it is not a crate warning.

- [ ] **Step 3: Run full test suite**

Run:

```bash
cargo test --workspace
```

Expected: exit 0.

- [ ] **Step 4: Launch GUI for manual smoke**

Run:

```bash
OPENLOGI_LOG=openlogi_hook=debug,openlogi_gui=debug cargo run -p openlogi-gui --release
```

Expected: GUI launches, hook installs when Accessibility is granted.

- [ ] **Step 5: Manual scroll smoke**

In the running GUI:

1. Set Vertical Wheel speed low, scroll main wheel, observe only vertical wheel feels slower.
2. Set Thumb Wheel / Horizontal speed high, use thumb wheel, observe only horizontal feel changes.
3. Toggle Invert vertical, verify main wheel direction flips.
4. Toggle Invert horizontal, verify thumb wheel direction flips.
5. Toggle Smooth vertical and Smooth horizontal independently, verify each axis respects its toggle.
6. Press Reset to defaults, verify only scroll settings reset; SmartShift settings remain unchanged.

- [ ] **Step 6: Manual SmartShift smoke**

In the running GUI with a SmartShift-capable mouse connected:

1. Turn Ratchet mode off, verify wheel goes free-spin and sensitivity slider appears disabled.
2. Turn Ratchet mode on, verify wheel goes ratchet and sensitivity slider appears enabled.
3. Set SmartShift sensitivity near `10%`, verify log applies a raw value around `26`.
4. Quit and relaunch; verify Ratchet mode and sensitivity UI restore from config and apply to the device.

- [ ] **Step 7: Commit any verification-only fixes**

If verification required code/doc fixes, commit them:

```bash
git add <changed-files>
git commit -m "fix: address scroll SmartShift verification findings"
```

If no fixes were required, do not create an empty commit.

---

## Self-review

- Spec coverage:
  - User-facing docs: Task 8.
  - Per-axis speed/step config: Task 1.
  - Per-axis smooth engine behavior: Task 3.
  - Grouped Scroll panel UI: Task 7.
  - SmartShift persisted mode: Tasks 2, 4, 5, 6, 7.
  - SmartShift 0–100% mapping: Task 5.
  - Startup/reconnect apply: Task 6.
  - Verification: Task 9.
- Placeholder scan: no `TBD`, `TODO`, or vague “implement later” steps remain.
- Type consistency:
  - `vertical_speed`, `horizontal_speed`, `vertical_step`, `horizontal_step` introduced in Task 1 and used in Tasks 3 and 7.
  - `smartshift_ratchet_mode` accessors introduced in Task 2 and used in Tasks 6 and 7.
  - `PendingSmartShift` introduced in Task 6 and used in `main.rs` dispatch.
  - `smartshift_percent_to_raw` and `smartshift_raw_to_percent` introduced in Task 5 and used in Task 7.
