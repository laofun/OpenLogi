# Scroll + SmartShift Settings — Design

**Date:** 2026-06-03
**Status:** Approved (design); implementation plan pending
**Scope:** User-facing documentation plus GUI/config updates for per-axis scroll tuning and persistent SmartShift ratchet/sensitivity controls.

## 1. Goals

1. Add user-facing documentation for the software scroll/smooth engine and SmartShift wheel settings. The existing docs mainly contain internal design notes and fork gotchas; users need a concise guide that explains what each setting does.
2. Rework the Scroll panel so vertical wheel and thumb wheel/horizontal controls are grouped separately.
3. Add per-axis **Speed** and **Step** tuning:
   - Vertical wheel gets its own speed + step.
   - Thumb wheel / horizontal axis gets its own speed + step.
   - Duration and dead zone stay shared because they describe the smoothing tail/cutoff, not one wheel's distance scale.
4. Add SmartShift controls:
   - **Ratchet mode** toggle, persisted and applied on startup/reconnect.
   - **SmartShift sensitivity** slider, shown as 0–100%, enabled only when Ratchet mode is on.

## 2. Non-goals

- No per-app scroll profiles.
- No device-side HID++ wheel customization beyond SmartShift mode/sensitivity.
- No CLI settings command in this phase.
- No fully separate settings window or large navigation redesign.
- No per-axis duration/dead-zone sliders unless future manual testing proves they are needed.

## 3. Documentation

Add a user-facing scroll/wheel guide, proposed path:

- `docs/SCROLL_AND_WHEEL.md`

The guide should cover:

- Vertical wheel vs thumb wheel mapping:
  - vertical wheel = macOS scroll axis 1
  - thumb wheel / horizontal = macOS scroll axis 2
- What inversion does for each axis.
- What smooth scrolling does: OpenLogi swallows hardware wheel events and re-emits interpolated synthetic frames at the HID tap location.
- Why trackpad/continuous scrolling is ignored by this engine.
- Per-axis speed/step and shared duration/dead-zone.
- SmartShift:
  - Ratchet mode vs free-spin mode
  - SmartShift sensitivity as 0–100%, internally mapped to HID++ 1–255
- Requirements/limitations:
  - macOS + Accessibility permission
  - settings are global for scroll behavior
  - SmartShift settings apply to devices that expose the relevant HID++ feature

Update `docs/CONFIGURATION.md` to include a TOML example for `[app_settings.scroll]` and device SmartShift fields, and link to the new guide. `CLAUDE.md` and `docs/FORK-DIVERGENCE.md` remain technical/gotcha references rather than the primary user docs.

## 4. Scroll panel UI

Replace the current flat Scroll panel layout with grouped sections:

```text
SCROLL

VERTICAL WHEEL
  Invert vertical          [toggle]
  Smooth vertical          [toggle]
  Speed                    [slider]
  Step                     [slider]

THUMB WHEEL / HORIZONTAL
  Invert horizontal        [toggle]
  Smooth horizontal        [toggle]
  Speed                    [slider]
  Step                     [slider]

SMOOTH FEEL
  Smooth scrolling         [master toggle]
  Duration                 [slider]
  Dead zone                [slider]

SMARTSHIFT
  Ratchet mode             [toggle]
  SmartShift sensitivity   [0–100% slider; disabled when Ratchet mode is off]

Reset to defaults
```

Notes:

- The master **Smooth scrolling** toggle still gates both axis smoothing toggles.
- The per-axis smooth toggles decide whether each axis enters the smoothing engine.
- Inversion stays independent of smoothing.
- Reset to defaults resets scroll tuning to `ScrollSettings::default()`; SmartShift reset behavior should be explicit in implementation planning because SmartShift is device-scoped while scroll settings are app-wide.

## 5. Scroll config model

Current `ScrollSettings` has shared `speed` and `step`. Replace those with per-axis fields while keeping backward-compatible deserialization.

Proposed shape:

```rust
pub struct ScrollSettings {
    pub smooth: bool,
    pub reverse_vertical: bool,
    pub reverse_horizontal: bool,
    pub smooth_vertical: bool,
    pub smooth_horizontal: bool,

    pub vertical_speed: f64,
    pub horizontal_speed: f64,
    pub vertical_step: f64,
    pub horizontal_step: f64,

    pub duration: f64,
    pub dead_zone: f64,
}
```

Defaults:

- `vertical_speed = 2.70`
- `horizontal_speed = 2.70`
- `vertical_step = 33.6`
- `horizontal_step = 33.6`
- `duration = 4.35`
- `dead_zone = 1.00`

Backward compatibility:

- Old serialized `speed` maps to both `vertical_speed` and `horizontal_speed`.
- Old serialized `step` maps to both `vertical_step` and `horizontal_step`.
- New serialization should write the new per-axis fields and omit default values through the existing `skip_serializing_if` pattern.
- This should not require a `SCHEMA_VERSION` bump if old configs continue to load and behavior is preserved.

## 6. Smooth engine changes

The smoothing engine should normalize and scale each axis independently.

Current conceptual flow:

```rust
buffer.x += normalize(dx, step) * speed;
buffer.y += normalize(dy, step) * speed;
```

New conceptual flow:

```rust
buffer.x += normalize(dx, horizontal_step) * horizontal_speed;
buffer.y += normalize(dy, vertical_step) * vertical_speed;
```

Duration/dead-zone remain shared in `frame_or_stop`.

Implementation can choose either an internal `AxisTuning` struct or explicit parameters. Prefer a small struct if it improves readability, but do not expand public API unnecessarily.

## 7. SmartShift config and behavior

Existing device config already has:

```rust
pub smartshift_sensitivity: Option<u8>
```

Add a persisted desired mode:

```rust
pub smartshift_ratchet_mode: Option<bool>
```

Semantics:

- `Some(true)`: app should apply ratchet/SmartShift mode on startup/reconnect.
- `Some(false)`: app should apply free-spin mode on startup/reconnect.
- `None`: no stored preference; leave firmware state untouched until the user changes it.

UI behavior:

- **Ratchet mode ON**:
  - apply ratchet/SmartShift mode live
  - persist `smartshift_ratchet_mode = Some(true)`
  - enable **SmartShift sensitivity** slider
- **Ratchet mode OFF**:
  - apply free-spin mode live
  - persist `smartshift_ratchet_mode = Some(false)`
  - disable sensitivity slider
- **SmartShift sensitivity**:
  - UI range: `0..=100%`
  - internal range: HID++ `1..=255`
  - linear mapping:
    - `0% -> 1`
    - `100% -> 255`
    - `10% -> about 26`
  - user's comfortable reference range is `20–25 / 255`, about `8–10%`.

Apply behavior should reuse or extend existing SmartShift hardware helpers rather than duplicating HID++ calls in UI components.

## 8. State and persistence

Scroll settings are app-wide and already flow through `AppState::commit_scroll_settings` and `hook_runtime::push_scroll_settings`.

SmartShift settings are device-scoped. The implementation should:

1. Store mode/sensitivity under the active device's `DeviceConfig`.
2. Apply live when the active connected device exposes SmartShift.
3. Apply persisted settings on app startup/reconnect, matching the existing persisted sensitivity flow.
4. Handle missing SmartShift support gracefully: UI can show disabled/unavailable state or no-op with a clear log, but must not crash.

## 9. Testing and verification

Automated tests:

- Config backward compatibility:
  - old `speed`/`step` load into both axis fields
  - new per-axis fields round-trip
- Scroll math:
  - horizontal speed/step affects only x output
  - vertical speed/step affects only y output
  - duration/dead-zone behavior remains unchanged
- SmartShift percent mapping:
  - `0% -> 1`
  - `100% -> 255`
  - `10% -> about 26`
- Device config serialization for `smartshift_ratchet_mode`.

Manual smoke:

- Vertical speed changes only main wheel feel.
- Horizontal speed changes only thumb wheel feel.
- Vertical/horizontal step changes the expected axis only.
- Invert vertical and invert horizontal still work independently.
- Smooth vertical and smooth horizontal still work independently.
- Ratchet mode toggles live between ratchet and free-spin.
- SmartShift sensitivity slider is disabled when Ratchet mode is off.
- SmartShift sensitivity applies live when Ratchet mode is on.
- SmartShift mode/sensitivity persist across restart/reconnect.
- Full gate remains:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`

## 10. Open implementation notes

- The implementation plan should decide whether `Reset to defaults` resets only app-wide scroll settings or also the active device's SmartShift settings. The design recommendation is: keep reset scoped to the Scroll panel's app-wide scroll settings unless the UI visually separates a SmartShift-specific reset.
- The SmartShift mode setter may require a new explicit helper if the current code only toggles the current firmware mode. Avoid relying on a blind toggle for persisted desired state; the app must be able to set a known mode.
- Keep the hook callback non-blocking. SmartShift HID writes should remain background/device-path work, not event-tap work.
