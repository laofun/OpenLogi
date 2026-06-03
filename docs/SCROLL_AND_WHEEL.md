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
