# MX Master 2S Reference Notes

**Date:** 2026-06-02
**Device observed:** Logitech MX Master 2S, direct HID++ route `046d:b019`
**External reference:** [`mmaher88/logitune` MX Master 2S descriptor](https://github.com/mmaher88/logitune/blob/master/devices/mx-master-2s/descriptor.json)

These notes capture reusable facts for OpenLogi's MX Master 2S support. Treat
this as reference / validation data, not as a file to copy verbatim. In
particular, do not import hotspot coordinates, images, or full descriptor JSON
without checking license/provenance and confirming the data against OpenLogi's
own assets and hardware behavior.

## Identity

- Device name: `MX Master 2S`.
- USB/HID product id: `0xb019`.
- Observed direct route in OpenLogi: `046d:b019`.
- OpenLogi config key currently used by existing config/assets: `0b019`.

### Observed live identity

```text
model_ids=[b019,4069,0000]
ext=00
transports=equad+btle
config_key=0b019
```

`model_ids[]` can contain transport-specific alternate product IDs.
`config_key()` uses only `model_ids[0]`, so this device remains keyed by `0b019`
regardless of additional model IDs present in the array.

The `4069` ID was not found in the OpenLogi repo or the current external asset registry.
It is treated as an alternate transport / internal PID (likely the BTLE variant),
not a new primary config key. Asset lookup in the GUI considers all nonzero model IDs,
so if the asset registry is later indexed under `4069` it will still match.

### DeviceInformation `0x0003` caveat

On the observed MX Master 2S direct route, HID++ DeviceInformation may produce a
non-useful model key:

```text
extended_model_id = 0x00
model_ids[0]      = 0x0000
DeviceModelInfo::config_key() = "00000"
```

`00000` should not be used as a persisted per-device config key for this device.
For the direct `046d:b019` route, fall back to `0b019` so CLI writes, GUI
bindings, assets, and auto-apply all refer to the same device section.

## Battery

Hardware-confirmed: MX Master 2S exposes **`0x1000 BatteryStatus`** (legacy), not
`0x1004 UnifiedBattery` and not `0x1001 BatteryVoltage`. The original `battery=—` in
`openlogi list` was because the inventory layer only read `0x1004`.

`openlogi diag battery` output on the device:

```text
0x1000 BatteryStatus:  present
0x1001 BatteryVoltage: not found
0x1004 UnifiedBattery: not found
```

### `0x1000` getStatus (function 0) wire format

Cross-checked against Solaar's `hidpp20.decipher_battery_status` (and the
`logitune` C++ port `Battery::parseStatusLegacy`, GPL-3.0 — referenced for
protocol facts only, no code copied):

```text
params[0] = current battery charge percentage (0..=100); 0 = unknown
params[1] = next discharge-level threshold (informational; ignored)
params[2] = status enum (same values as 0x1004 UnifiedBattery):
            0 discharging, 1 recharging, 2 almost-full, 3 full,
            4 slow-recharge, 5 invalid battery, 6 thermal error
```

Implementation notes:

- The function ID for legacy `0x1000` getStatus is **0** (the shifted IDs only apply
  to `0x2111` SmartShift, not battery features).
- `params[0] == 0` means **unknown**, not a literal 0%. Inventory and `diag battery`
  treat a zero reading as "no battery info" rather than surfacing a bogus
  `0% critical`. A degraded just-woken read (all-zero `model_ids` / `unit_id` /
  `transports`) also produces `params[0] == 0`, so this guard covers both cases.
- See `crates/openlogi-hid/src/battery_status.rs`.

## Feature matrix

The external descriptor marks these MX Master 2S features as supported:

- Battery reporting.
- Adjustable DPI.
- SmartShift.
- HiRes wheel.
- Smooth scroll.
- Thumb wheel.
- Reprogrammable controls.

It marks these as unsupported / not expected for this device:

- Extended DPI.
- HiRes scrolling.
- Low-res wheel.
- Gesture V2 / mouse gesture feature variants.
- Haptic / force-sensing / crown features.
- Report-rate and extended report-rate controls.
- Pointer speed, left-right swap, surface tuning, angle snapping.
- RGB/color LED effects.
- Onboard profiles / persistent remappable actions.

OpenLogi implication: prefer legacy feature paths already used for the 2S:
`0x2110` SmartShift, adjustable DPI rather than extended DPI, and reprog-controls
rather than newer gesture/persistent-remap features.

## DPI

External descriptor range:

```text
min  = 200
max  = 4000
step = 50
```

OpenLogi currently uses a broader generic GUI/CLI DPI window in some places.
This range is useful for future per-device slider constraints and validation for
MX Master 2S.

## Reprogrammable controls

External descriptor button/control mapping:

| Control ID | Button index | Default name | Default action type | Configurable |
|---:|---:|---|---|---|
| `0x0050` | 0 | Left click | default | no |
| `0x0051` | 1 | Right click | default | no |
| `0x0052` | 2 | Middle click | default | yes |
| `0x0053` | 3 | Back | default | yes |
| `0x0056` | 4 | Forward | default | yes |
| `0x00c3` | 5 | Gesture button | gesture-trigger | yes |
| `0x00c4` | 6 | Shift wheel mode | smartshift-toggle | yes |
| `0x0000` | 7 | Thumb wheel | default | yes |

OpenLogi implications:

- `0x00c4` should be treated/displayed as the SmartShift / mode-shift button for
  MX Master 2S. If an existing OpenLogi enum name such as `DpiToggle` is used for
  schema compatibility, prefer a device-aware UI label rather than renaming the
  persisted variant.
- `0x00c3` is the gesture trigger.
- Back/Forward/Middle mappings line up with the expected standard controls.

## SmartShift

MX Master 2S uses the original SmartShift feature (`0x2110`), not the Enhanced
`0x2111` call table used by MX Master 3 / 3S class devices.

For sensitivity writes on `0x2110`, use the feature's documented optional fields:

- `wheel_mode = None` means keep the current wheel mode unchanged.
- `auto_disengage = Some(N)` writes the SmartShift sensitivity / threshold.
- `auto_disengage_default = None` leaves the firmware default unchanged.

Do not read the current mode and write it back just to preserve it on `0x2110`;
that is unnecessary and was observed to flip `Ratchet -> Free` on MX Master 2S.
For `0x2111`, `set_status` has no equivalent "keep current" sentinel, so it
still needs read-current-mode + write mode together with sensitivity.

## Gesture defaults

External descriptor default gestures:

| Direction | External action |
|---|---|
| Up | Default / empty payload |
| Down | `Super+D` |
| Left | `Ctrl+Super+Left` |
| Right | `Ctrl+Super+Right` |
| Click | `Super+W` |

Use these only as reference for optional presets. Do not auto-inject them into a
user's `config.toml`: OpenLogi user config should remain explicit and
user-controlled.

## Asset / hotspot data

The external descriptor includes image names and hotspot coordinates for buttons,
scroll wheel, thumb wheel, pointer, and Easy-Switch slots. OpenLogi already has
MX Master 2S local asset metadata under:

```text
crates/openlogi-gui/assets/mx_master_2s/core_metadata.json
crates/openlogi-gui/assets/mx_master_2s/manifest.json
```

Use the external coordinates only as a sanity-check reference. Avoid copying
coordinates or images verbatim without a license/provenance review.
