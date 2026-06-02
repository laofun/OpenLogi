# Persist SmartShift Sensitivity + Auto-Apply on Connect — Design

**Date:** 2026-06-02
**Branch:** `feat/smartshift-persist` (based on `feat/smartshift-0x2110`)

## Goal

Persist a device's SmartShift auto-disengage sensitivity in `config.toml` so
the GUI can re-apply it to the firmware whenever that device connects. No new
GUI controls: the value is written to config from the CLI (`--save`) or by
hand-editing, and the GUI applies it automatically.

## Background

The SmartShift backend already exists on `feat/smartshift-0x2110`:

- `openlogi_hid::set_smartshift_sensitivity(route, value) -> SmartShiftStatus`
  writes the auto-disengage threshold (1–255; 255 = permanent ratchet) while
  preserving the current Free/Ratchet mode, and reads the value back.
- `openlogi diag smartshift --sensitivity N` exercises that path from the CLI.

What is missing is **persistence** (the value is lost when the device sleeps or
re-pairs) and **auto-apply** (nothing re-writes it on connect). This feature
adds both.

The firmware exposes no continuous "sensitivity" for either wheel beyond
SmartShift's `auto_disengage` threshold: the HiRes wheel (`0x2121`) only toggles
resolution Low/High plus invert, and the thumbwheel (`0x2150`) only sets
reporting mode plus invert. Those are **out of scope** here (see below).

## Scope

**In scope:**

- A per-device `smartshift_sensitivity` field in `config.toml`.
- A `--save` flag on `diag smartshift` that persists the value after a
  successful firmware write.
- GUI auto-apply: write the persisted value to the device once per connection.

**Out of scope (deferred to a separate spec):**

- HiRes wheel resolution (Low/High) toggle and invert.
- Thumbwheel reporting mode / invert.
- Any GUI slider or control for sensitivity.

## Architecture

Three layers, each independently testable:

1. **Config (`openlogi-core`)** — a new optional `DeviceConfig` field plus
   getter/setter, mirroring the existing `dpi_presets` pattern. Pure data; fully
   unit-testable.
2. **CLI (`openlogi-cli`)** — a `--save` flag that, after the existing firmware
   write + read-back verification, loads the config, sets the field under the
   device's `config_key`, and saves atomically.
3. **GUI (`openlogi-gui`)** — on each inventory refresh, a pure decision
   function selects which connected devices still need their persisted
   sensitivity applied (apply-once-per-connection), and a background hardware
   call writes it.

### Data flow

```
CLI:  diag smartshift --sensitivity N --save
        → set_smartshift_sensitivity(route, N)   [firmware write + verify]
        → Config::load_or_default()
        → set_smartshift_sensitivity(config_key, Some(N))
        → save_atomic()                           [~/.config/openlogi/config.toml]

GUI:  inventory refresh (device connects)
        → smartshift_pending(config, connected_keys, already_applied)
        → for each (config_key, value): apply_smartshift_sensitivity_in_background(route, value)
        → mark config_key applied; prune disconnected keys
```

## Components

### 1. Config schema — `crates/openlogi-core/src/config.rs`

Add to `DeviceConfig` (next to `dpi_presets`):

```rust
/// Persisted SmartShift auto-disengage sensitivity (1–255). Applied to the
/// firmware by the GUI when the device connects. `None` = leave the firmware
/// value untouched.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub smartshift_sensitivity: Option<u8>,
```

Add accessors on `Config`, mirroring `dpi_presets` / `set_dpi_presets`:

```rust
pub fn smartshift_sensitivity(&self, device_key: &str) -> Option<u8> {
    self.devices
        .get(device_key)
        .and_then(|d| d.smartshift_sensitivity)
}

pub fn set_smartshift_sensitivity(&mut self, device_key: &str, value: Option<u8>) {
    self.devices
        .entry(device_key.to_string())
        .or_default()
        .smartshift_sensitivity = value;
}
```

**`SCHEMA_VERSION` stays `1`.** The field is additive and optional, so existing
v1 configs continue to parse (the field defaults to `None`). Bumping the version
would make the strict check in `load_from_path` reject every existing config and
silently drop the user's bindings.

**Value range:** valid stored values are 1–255. `0` is a device no-op
("no change"); the CLI already rejects it, and auto-apply treats `Some(0)`
defensively as "skip" so a hand-edited `0` never triggers a pointless write.

### 2. CLI `--save` — `crates/openlogi-cli/src/cmd/diag/`

**`mod.rs` — expose the config key.** `first_online_device()` currently returns
`(DeviceRoute, String)` and discards `paired.model`. Change it to return
`(DeviceRoute, String, String)` where the third element is
`paired.model.config_key()`. Update the two other callers (`features.rs`,
`dpi.rs`) to `let (route, name, _) = first_online_device().await?;`.

**`smartshift.rs` — add the flag.** Add to `SmartshiftArgs`:

```rust
/// Persist the sensitivity to config.toml under this device's key so the
/// GUI re-applies it on every connect. Only valid together with --sensitivity.
#[arg(long, requires = "sensitivity")]
pub save: bool,
```

In `run()`, inside the `if let Some(n) = args.sensitivity` branch, after the
read-back verification succeeds and before returning:

```rust
if args.save {
    let mut config = Config::load_or_default().context("load config for --save")?;
    config.set_smartshift_sensitivity(&config_key, Some(n));
    config.save_atomic().context("save config")?;
    println!("✓ saved sensitivity {n} to config for device {config_key}");
}
```

`requires = "sensitivity"` makes clap reject `--save` without `--sensitivity` at
parse time, so no runtime guard is needed.

### 3. GUI auto-apply — `crates/openlogi-gui/`

**`hardware.rs` — background write.** Add, mirroring
`toggle_smartshift_in_background`:

```rust
/// Apply a persisted SmartShift sensitivity to `target` off-thread. Used by the
/// auto-apply path when a device connects. Opens a fresh channel from the route
/// (capture is None) — the same approach the DPI panel uses for slider writes.
pub fn apply_smartshift_sensitivity_in_background(
    capture: Option<&CaptureChannel>,
    target: Option<DeviceRoute>,
    value: u8,
)
```

It calls `openlogi_hid::set_smartshift_sensitivity(route, value)`; on error it
logs a `warn!` and returns (no panic, no retry within the call).

**Pure decision function** (in `state.rs` or a small `state/smartshift.rs`),
unit-testable without hardware:

```rust
/// Given the persisted config, the config_keys currently connected, and the set
/// already applied this session, return the (config_key, value) pairs that
/// still need a firmware write. Skips devices with no persisted value and any
/// stored 0 (a device no-op). Caller is responsible for pruning `already_applied`
/// of keys no longer in `connected` so a reconnect re-applies.
fn smartshift_pending(
    config: &Config,
    connected: &[String],
    already_applied: &HashSet<String>,
) -> Vec<(String, u8)>
```

**State.** `AppState` gains `smartshift_applied: HashSet<String>`.

**Insertion point.** In `main.rs`, the inventory-refresh branch
(`Some(new_inv) = inventory_rx.recv()`), after
`state.refresh_inventories(&new_inv, &cache)`:

1. Collect the currently-connected `config_key`s from the refreshed device
   records (each record carries `config_key` + `route`).
2. Prune `smartshift_applied` of keys not in the connected set (so a
   disconnect→reconnect re-applies).
3. Call `smartshift_pending(...)` to get the pending writes.
4. For each `(config_key, value)`, look up the device's `route` and call
   `apply_smartshift_sensitivity_in_background(None, route, value)`, then insert
   the key into `smartshift_applied`.

A key is inserted into `smartshift_applied` whether or not the firmware write
succeeded, so a failing device is not hammered on every poll tick; reconnecting
clears the key and retries.

## Error Handling

- **Config load/save failure in CLI `--save`:** surfaced via `anyhow` context;
  the firmware write has already happened and is reported, so the user sees the
  device changed but the save failed — they can retry.
- **Firmware write failure during auto-apply:** logged at `warn!`; the device is
  marked applied to avoid per-tick spam. Reconnect retries.
- **Hand-edited invalid value (`0`):** `smartshift_pending` skips `Some(0)`;
  values 1–255 are written verbatim (the firmware itself bounds them).

## Testing

- **config.rs (unit):**
  - `smartshift_sensitivity` / `set_smartshift_sensitivity` round-trip.
  - TOML serialization contains `smartshift_sensitivity = N` when set and omits
    it when `None`.
  - A config string without the field loads with the field defaulting to `None`.
- **CLI (unit):** the config-mutation path — given a `config_key` and value,
  `set_smartshift_sensitivity` + serialize produces the expected TOML. The
  firmware write itself is verified manually on hardware.
- **GUI (unit):** `smartshift_pending`:
  - first connect of a device with a persisted value → returns it;
  - same device already in `already_applied` → returns nothing;
  - device with no persisted value → never returned;
  - stored `Some(0)` → skipped;
  - (pruning is exercised by a test that reconnects a key after removing it from
    `already_applied`).
- **Manual hardware (MX Master 2S):** `--sensitivity N --save`, confirm
  `config.toml` updated; restart GUI / re-pair device, confirm the value is
  re-applied (read back via `diag smartshift`).

## Documentation Constraints

- Spec and plan live under `docs/superpowers/`.
- Do **not** modify `docs/USAGE.md`, `docs/CONFIGURATION.md`, or
  `docs/DEVELOPMENT.md`.
- All documentation in English.
