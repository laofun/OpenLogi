# Configuration

How OpenLogi stores its settings. For install and usage, see the
[README](../README.md).

Config is a TOML file, read on startup and written atomically on change:

- macOS & Linux: `$XDG_CONFIG_HOME/openlogi/config.toml` (default `~/.config/openlogi/config.toml`)
- Windows: `%USERPROFILE%\.config\openlogi\config.toml`

Per-device settings are keyed by the HID++ identifier (e.g. `2b042` for an
MX Master 4): `button_bindings`, `per_app_bindings` (keyed by bundle id such as
`com.microsoft.VSCode`), `gesture_bindings`, `dpi_presets`, and SmartShift
preferences. The app-wide `[app_settings]` block holds `launch_at_login`,
`check_for_updates`, and scroll settings. See [Scroll and Wheel
Settings](SCROLL_AND_WHEEL.md) for how the software-side scroll event-tap
behavior and SmartShift preferences work.

```toml
schema_version = 1
selected_device = "2b042"

[app_settings]
launch_at_login = true
check_for_updates = true

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

[devices.2b042.button_bindings]
Back = "BrowserBack"
Forward = "BrowserForward"

# Per-app overlay: Back becomes Undo only while VS Code is frontmost.
[devices.2b042.per_app_bindings."com.microsoft.VSCode"]
Back = "Undo"
```
