# Persist SmartShift Sensitivity + Auto-Apply on Connect — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist a device's SmartShift auto-disengage sensitivity in `config.toml` and have the GUI re-apply it to the firmware whenever that device connects.

**Architecture:** Three independently-testable layers. (1) `openlogi-core` gains an optional per-device `smartshift_sensitivity` field plus accessors, mirroring `dpi_presets`. (2) `openlogi-cli` gets a `--save` flag on `diag smartshift` that persists the value after the existing firmware write + read-back. (3) `openlogi-gui` reads the persisted value fresh from disk on each inventory refresh, applies it once per connection via a background HID++ write, and syncs it back into the in-memory config so a later full-file save doesn't clobber it.

**Tech Stack:** Rust (edition 2024), `serde` + `toml` for config, `clap` derive for the CLI, `tokio` one-shot runtimes for background HID++ writes, GPUI globals for GUI state.

**Reference spec:** `docs/superpowers/specs/2026-06-02-smartshift-sensitivity-persist-design.md`

**Standing constraints:**
- All documentation in English; do **not** modify `docs/USAGE.md`, `docs/CONFIGURATION.md`, `docs/DEVELOPMENT.md`.
- "Source wins" — if a referenced line number has drifted, trust the current source and adapt.
- Commit messages on this branch carry the trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Do not push without explicit user request.
- Build env: full Xcode 16.4 is installed. If `DEVELOPER_DIR` points at the old CLT path, `unset DEVELOPER_DIR` before `cargo build`/`clippy` so the GUI crate links against the real Xcode toolchain.

---

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `crates/openlogi-core/src/config.rs` | Config schema + accessors | Add `smartshift_sensitivity` field + 2 accessors + 3 tests |
| `crates/openlogi-cli/src/cmd/diag/mod.rs` | Shared device picker | `first_online_device()` returns the config key as a 3rd tuple element |
| `crates/openlogi-cli/src/cmd/diag/features.rs` | `diag features` | Update destructuring of `first_online_device()` |
| `crates/openlogi-cli/src/cmd/diag/dpi.rs` | `diag dpi` | Update destructuring of `first_online_device()` |
| `crates/openlogi-cli/src/cmd/diag/smartshift.rs` | `diag smartshift` | Add `--save` flag + persist logic + parse tests |
| `crates/openlogi-gui/src/hardware.rs` | Background HID++ writes | Add `apply_smartshift_sensitivity_in_background` |
| `crates/openlogi-gui/src/state.rs` | App-wide GUI state | Add `smartshift_applied` field, pure `smartshift_pending` fn, `pending_smartshift_writes` method + tests |
| `crates/openlogi-gui/src/main.rs` | Inventory-refresh loop | Dispatch pending writes after `refresh_inventories` |

---

## Task 1: Config field + accessors (`openlogi-core`)

**Files:**
- Modify: `crates/openlogi-core/src/config.rs` (struct ~126-149, accessors after line 363, tests after line 493)
- Test: same file, `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing tests**

Add these three tests to the `mod tests` block in `crates/openlogi-core/src/config.rs` (after `empty_dpi_presets_skip_serialization`, mirroring the `dpi_presets` tests):

```rust
    #[test]
    fn smartshift_sensitivity_roundtrip_per_device() {
        let mut cfg = Config::default();
        cfg.set_smartshift_sensitivity("2b042", Some(20));
        cfg.set_smartshift_sensitivity("4082d", Some(255));

        let parsed = write_and_read(&cfg);

        assert_eq!(parsed.smartshift_sensitivity("2b042"), Some(20));
        assert_eq!(parsed.smartshift_sensitivity("4082d"), Some(255));
        assert_eq!(parsed.smartshift_sensitivity("unknown"), None);
    }

    #[test]
    fn smartshift_sensitivity_none_skips_serialization() {
        let mut cfg = Config::default();
        // A binding makes the device block exist even with no sensitivity.
        cfg.set_binding("2b042", ButtonId::Back, Action::Copy);
        cfg.set_smartshift_sensitivity("2b042", Some(30));
        cfg.set_smartshift_sensitivity("2b042", None); // clear

        let body = toml::to_string_pretty(&cfg).expect("serialize");
        assert!(
            !body.contains("smartshift_sensitivity"),
            "None must be omitted; got: {body}"
        );
    }

    #[test]
    fn smartshift_sensitivity_present_in_toml_when_set() {
        let mut cfg = Config::default();
        cfg.set_smartshift_sensitivity("2b042", Some(42));

        let body = toml::to_string_pretty(&cfg).expect("serialize");
        assert!(
            body.contains("smartshift_sensitivity = 42"),
            "got: {body}"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p openlogi-core smartshift_sensitivity`
Expected: FAIL to compile — `no method named 'set_smartshift_sensitivity' / 'smartshift_sensitivity' found for struct 'Config'`.

- [ ] **Step 3: Add the struct field**

In `crates/openlogi-core/src/config.rs`, inside `pub struct DeviceConfig` (currently ending at the `dpi_presets` field, line ~148), add a new field immediately after `pub dpi_presets: Vec<u32>,`:

```rust
    /// Persisted SmartShift auto-disengage sensitivity (1–255). Applied to the
    /// firmware by the GUI when the device connects. `None` = leave the
    /// firmware value untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smartshift_sensitivity: Option<u8>,
```

- [ ] **Step 4: Add the accessors**

In the same file, immediately after the `set_dpi_presets` method (closing brace at line ~363, before the `}` that closes the `impl Config` block), add:

```rust
    /// The persisted SmartShift auto-disengage sensitivity for `device_key`, or
    /// `None` if the device has none configured. Values are 1–255; the GUI
    /// applies this to the firmware on connect.
    #[must_use]
    pub fn smartshift_sensitivity(&self, device_key: &str) -> Option<u8> {
        self.devices
            .get(device_key)
            .and_then(|d| d.smartshift_sensitivity)
    }

    /// Set (or clear, with `None`) the persisted SmartShift sensitivity for
    /// `device_key`. `None` omits the field on save thanks to
    /// `skip_serializing_if`; the device block itself is kept.
    pub fn set_smartshift_sensitivity(&mut self, device_key: &str, value: Option<u8>) {
        self.devices
            .entry(device_key.to_string())
            .or_default()
            .smartshift_sensitivity = value;
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p openlogi-core smartshift_sensitivity`
Expected: PASS (3 tests).

- [ ] **Step 6: Full-crate test + clippy**

Run: `cargo test -p openlogi-core && cargo clippy -p openlogi-core --all-targets -- -D warnings`
Expected: all tests pass, clippy clean.

- [ ] **Step 7: Commit**

```bash
git add crates/openlogi-core/src/config.rs
git commit -m "feat(core): persist per-device SmartShift sensitivity in config

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: CLI `--save` flag (`openlogi-cli`)

**Files:**
- Modify: `crates/openlogi-cli/src/cmd/diag/mod.rs:41-64` (`first_online_device`)
- Modify: `crates/openlogi-cli/src/cmd/diag/features.rs:16`
- Modify: `crates/openlogi-cli/src/cmd/diag/dpi.rs:17`
- Modify: `crates/openlogi-cli/src/cmd/diag/smartshift.rs` (args + run + tests)
- Test: `crates/openlogi-cli/src/cmd/diag/smartshift.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing CLI parse tests**

Append a test module to the end of `crates/openlogi-cli/src/cmd/diag/smartshift.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Wrap the `Args` group in a throwaway `Parser` so we can exercise
    /// clap's `requires` / value parsing without a real device.
    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: SmartshiftArgs,
    }

    #[test]
    fn save_requires_sensitivity() {
        // `--save` alone must be rejected at parse time by `requires`.
        let parsed = TestCli::try_parse_from(["t", "--save"]);
        assert!(parsed.is_err(), "save without sensitivity should fail");
    }

    #[test]
    fn save_with_sensitivity_parses() {
        let cli =
            TestCli::try_parse_from(["t", "--sensitivity", "20", "--save"]).expect("should parse");
        assert_eq!(cli.args.sensitivity, Some(20));
        assert!(cli.args.save);
    }

    #[test]
    fn sensitivity_without_save_parses() {
        let cli = TestCli::try_parse_from(["t", "--sensitivity", "20"]).expect("should parse");
        assert_eq!(cli.args.sensitivity, Some(20));
        assert!(!cli.args.save);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p openlogi-cli save_requires_sensitivity`
Expected: FAIL to compile — `no field 'save' on type 'SmartshiftArgs'`.

- [ ] **Step 3: Add the `--save` flag to `SmartshiftArgs`**

In `crates/openlogi-cli/src/cmd/diag/smartshift.rs`, add a field to `pub struct SmartshiftArgs` after the `sensitivity` field (line ~20):

```rust
    /// Persist the sensitivity to config.toml under this device's key so the
    /// GUI re-applies it on every connect. Only valid together with
    /// --sensitivity.
    #[arg(long, requires = "sensitivity")]
    pub save: bool,
```

- [ ] **Step 4: Run the parse tests to verify they pass**

Run: `cargo test -p openlogi-cli save_requires_sensitivity save_with_sensitivity_parses sensitivity_without_save_parses`
Expected: PASS (3 tests).

- [ ] **Step 5: Thread the config key out of `first_online_device`**

In `crates/openlogi-cli/src/cmd/diag/mod.rs`, change the signature and body of `first_online_device` (lines 41-64). Replace the whole function with:

```rust
/// Shared device picker: enumerate inventories, return the [`DeviceRoute`],
/// display name, and `config_key` of the first online paired device (the same
/// selection rule the GUI uses for its initial target). Builds a Bolt route
/// when the device is behind a receiver, a direct route otherwise (USB cable /
/// Bluetooth). The `config_key` is empty for a device that does not expose
/// HID++ feature 0x0003 (DeviceInformation) — `--save` callers should treat an
/// empty key as "cannot persist".
pub(crate) async fn first_online_device() -> Result<(DeviceRoute, String, String)> {
    use anyhow::anyhow;
    let inventories = openlogi_hid::enumerate().await?;
    inventories
        .into_iter()
        .find_map(|inv| {
            let paired = inv.paired.into_iter().find(|p| p.online)?;
            let route = match inv.receiver.unique_id {
                Some(receiver_uid) => DeviceRoute::Bolt {
                    receiver_uid,
                    slot: paired.slot,
                },
                None => DeviceRoute::Direct {
                    vendor_id: inv.receiver.vendor_id,
                    product_id: inv.receiver.product_id,
                },
            };
            let config_key = paired
                .model_info
                .as_ref()
                .map(|m| m.config_key())
                .unwrap_or_default();
            let name = paired
                .codename
                .unwrap_or_else(|| format!("Slot {}", paired.slot));
            Some((route, name, config_key))
        })
        .ok_or_else(|| anyhow!("no online HID++ device found — is a Logi mouse paired?"))
}
```

- [ ] **Step 6: Update the two other callers**

In `crates/openlogi-cli/src/cmd/diag/features.rs:16`, change:

```rust
    let (route, name) = first_online_device().await?;
```
to:
```rust
    let (route, name, _) = first_online_device().await?;
```

In `crates/openlogi-cli/src/cmd/diag/dpi.rs:17`, change:

```rust
    let (route, name) = first_online_device().await?;
```
to:
```rust
    let (route, name, _) = first_online_device().await?;
```

- [ ] **Step 7: Wire the persist logic into `smartshift::run`**

In `crates/openlogi-cli/src/cmd/diag/smartshift.rs`:

First, add the config import near the top (after the existing `use crate::cmd::diag::first_online_device;`, line ~6):

```rust
use openlogi_core::config::Config;
```

Then change the destructuring at the start of `run` (line ~24) from:

```rust
    let (route, name) = first_online_device().await?;
```
to:
```rust
    let (route, name, config_key) = first_online_device().await?;
```

Then, inside the `if let Some(n) = args.sensitivity` branch, replace the final success block (currently lines ~62-66):

```rust
        println!(
            "✓ SmartShift sensitivity set to {n} (mode {:?} preserved)",
            after.mode
        );
        return Ok(());
```
with:
```rust
        println!(
            "✓ SmartShift sensitivity set to {n} (mode {:?} preserved)",
            after.mode
        );

        if args.save {
            if config_key.is_empty() {
                anyhow::bail!(
                    "cannot --save: device did not report a model id (HID++ 0x0003); \
                     the GUI keys config by model id"
                );
            }
            let mut config = Config::load_or_default().context("load config for --save")?;
            config.set_smartshift_sensitivity(&config_key, Some(n));
            config.save_atomic().context("save config")?;
            println!("✓ saved sensitivity {n} to config for device {config_key}");
        }
        return Ok(());
```

- [ ] **Step 8: Run the crate tests + clippy**

Run: `cargo test -p openlogi-cli && cargo clippy -p openlogi-cli --all-targets -- -D warnings`
Expected: all tests pass (including the 3 new parse tests), clippy clean.

- [ ] **Step 9: Commit**

```bash
git add crates/openlogi-cli/src/cmd/diag/
git commit -m "feat(cli): add --save to diag smartshift to persist sensitivity

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: GUI auto-apply (`openlogi-gui`)

This task has three sub-parts: the background hardware write (3a), the state decision logic + tests (3b), and the main-loop wiring (3c). Each ends in its own commit.

### Task 3a: Background hardware write

**Files:**
- Modify: `crates/openlogi-gui/src/hardware.rs` (add fn after `toggle_smartshift_in_background`, line ~84)

- [ ] **Step 1: Add `apply_smartshift_sensitivity_in_background`**

In `crates/openlogi-gui/src/hardware.rs`, add this function immediately after `toggle_smartshift_in_background` (after its closing brace, line ~84):

```rust
/// Spawn an OS thread that writes a persisted SmartShift auto-disengage
/// `value` (1–255) to the device at `target` via
/// `openlogi_hid::set_smartshift_sensitivity`, preserving the current mode.
/// Used by the GUI auto-apply path when a device connects. Returns
/// immediately; failures (incl. devices exposing neither `0x2111` nor the
/// older `0x2110` SmartShift feature) are logged, never retried within the
/// call.
///
/// `target == None` is a no-op (dev environment without a real device).
pub fn apply_smartshift_sensitivity_in_background(
    capture: Option<&CaptureChannel>,
    target: Option<DeviceRoute>,
    value: u8,
) {
    let Some(target) = target else {
        debug!(value, "no target device — SmartShift sensitivity apply skipped");
        return;
    };
    // Auto-apply opens a fresh channel (capture = None / non-matching is fine);
    // the read here keeps the call shape identical to the toggle/DPI writers.
    let shared = reusable_channel(capture, &target);
    let reused = shared.is_some();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                warn!(error = %e, "tokio runtime init failed; SmartShift sensitivity apply skipped");
                return;
            }
        };
        let result = rt.block_on(async {
            tokio::time::timeout(
                WRITE_BUDGET,
                openlogi_hid::set_smartshift_sensitivity(&target, value),
            )
            .await
        });
        let index = target.device_index();
        match result {
            Ok(Ok(status)) => debug!(
                index,
                value,
                applied = status.sensitivity,
                reused,
                "SmartShift sensitivity applied"
            ),
            Ok(Err(e)) => warn!(error = ?e, "SmartShift sensitivity apply failed"),
            Err(_) => warn!(
                index,
                "SmartShift sensitivity apply timed out (device asleep/unresponsive)"
            ),
        }
    });
}
```

> Note: `set_smartshift_sensitivity` has no `SharedChannel` (`_on`) variant, so this always opens a fresh channel from the route. `reused` will be `false` in practice; the `reusable_channel` probe is kept only to match the existing writers' shape and to log honestly if a future shared variant is added.

- [ ] **Step 2: Build + clippy (no test — verified via 3b/3c and manual hardware)**

Run: `cargo clippy -p openlogi-gui --all-targets -- -D warnings`
Expected: clippy clean (a build error here means the signature or imports are wrong).

> `DeviceRoute`, `CaptureChannel`, `SharedChannel`, `debug`, `warn`, and `WRITE_BUDGET` are already imported/defined at the top of `hardware.rs` — no new `use` needed.

- [ ] **Step 3: Commit**

```bash
git add crates/openlogi-gui/src/hardware.rs
git commit -m "feat(gui): add background SmartShift sensitivity writer

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 3b: State decision logic + tests

**Files:**
- Modify: `crates/openlogi-gui/src/state.rs` (imports line ~17-24, `AppState` struct ~42-90, constructor ~135-149, methods after `current_record` ~212, new `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing unit tests for `smartshift_pending`**

Append a test module to the end of `crates/openlogi-gui/src/state.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use openlogi_core::config::Config;
    use std::collections::HashSet;

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn pending_returns_persisted_value_on_first_connect() {
        let mut cfg = Config::default();
        cfg.set_smartshift_sensitivity("2b042", Some(20));
        let pending = smartshift_pending(&cfg, &keys(&["2b042"]), &HashSet::new());
        assert_eq!(pending, vec![("2b042".to_string(), 20)]);
    }

    #[test]
    fn pending_skips_already_applied() {
        let mut cfg = Config::default();
        cfg.set_smartshift_sensitivity("2b042", Some(20));
        let applied: HashSet<String> = ["2b042".to_string()].into_iter().collect();
        let pending = smartshift_pending(&cfg, &keys(&["2b042"]), &applied);
        assert!(pending.is_empty());
    }

    #[test]
    fn pending_skips_device_without_value() {
        let cfg = Config::default(); // no persisted sensitivity for 2b042
        let pending = smartshift_pending(&cfg, &keys(&["2b042"]), &HashSet::new());
        assert!(pending.is_empty());
    }

    #[test]
    fn pending_skips_stored_zero() {
        let mut cfg = Config::default();
        cfg.set_smartshift_sensitivity("2b042", Some(0)); // hand-edited no-op
        let pending = smartshift_pending(&cfg, &keys(&["2b042"]), &HashSet::new());
        assert!(pending.is_empty());
    }

    #[test]
    fn pending_reapplies_after_key_pruned_from_applied() {
        let mut cfg = Config::default();
        cfg.set_smartshift_sensitivity("2b042", Some(20));
        // Simulate a reconnect: the key was applied, then pruned (disconnect).
        let applied = HashSet::new();
        let pending = smartshift_pending(&cfg, &keys(&["2b042"]), &applied);
        assert_eq!(pending, vec![("2b042".to_string(), 20)]);
    }

    #[test]
    fn pending_handles_multiple_devices() {
        let mut cfg = Config::default();
        cfg.set_smartshift_sensitivity("2b042", Some(20));
        cfg.set_smartshift_sensitivity("4082d", Some(255));
        let applied: HashSet<String> = ["2b042".to_string()].into_iter().collect();
        let pending = smartshift_pending(&cfg, &keys(&["2b042", "4082d"]), &applied);
        assert_eq!(pending, vec![("4082d".to_string(), 255)]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p openlogi-gui smartshift_pending`
Expected: FAIL to compile — `cannot find function 'smartshift_pending' in this scope`.

- [ ] **Step 3: Add imports**

In `crates/openlogi-gui/src/state.rs`, change the `std::collections` import (line 17) from:

```rust
use std::collections::BTreeMap;
```
to:
```rust
use std::collections::{BTreeMap, HashSet};
```

And add a `DeviceRoute` import next to the existing `openlogi_hid` use. There is currently no `openlogi_hid` import in `state.rs`; add this line after the `use openlogi_hook::Hook;` line (line 23):

```rust
use openlogi_hid::DeviceRoute;
```

- [ ] **Step 4: Add the `smartshift_applied` field**

In `pub struct AppState`, add a field after `gesture_hook_bindings` (the last field, line ~89, before the closing `}` at line 90):

```rust
    /// `config_key`s whose persisted SmartShift sensitivity has already been
    /// evaluated for the current connection (applied, or determined to need no
    /// write). Pruned to the connected set on each refresh so a
    /// disconnect→reconnect re-applies. Prevents per-poll-tick re-writes and
    /// re-reads of the config file.
    smartshift_applied: HashSet<String>,
```

- [ ] **Step 5: Initialise the field in the constructor**

In `with_runtime_shared`, the `let mut state = Self { ... }` literal (lines ~135-149), add the field initialiser after `gesture_hook_bindings,`:

```rust
            smartshift_applied: HashSet::new(),
```

- [ ] **Step 6: Add the pure decision function**

In `crates/openlogi-gui/src/state.rs`, add this free function. Place it just below the `AppState` `impl` block's closing brace is not required — put it at module scope, e.g. immediately before the `#[cfg(test)] mod tests` you added in Step 1:

```rust
/// Given the persisted config, the `config_key`s currently connected, and the
/// set already evaluated this session, return the `(config_key, value)` pairs
/// that still need a firmware write. Skips devices with no persisted value and
/// any stored `0` (a device no-op). Pure — no I/O, fully unit-testable.
///
/// The caller ([`AppState::pending_smartshift_writes`]) is responsible for
/// pruning `already_applied` of keys no longer connected so a reconnect
/// re-applies, and for marking keys applied after dispatch.
fn smartshift_pending(
    config: &Config,
    connected: &[String],
    already_applied: &HashSet<String>,
) -> Vec<(String, u8)> {
    connected
        .iter()
        .filter(|key| !already_applied.contains(*key))
        .filter_map(|key| {
            config
                .smartshift_sensitivity(key)
                .filter(|&v| v != 0)
                .map(|v| (key.clone(), v))
        })
        .collect()
}
```

- [ ] **Step 7: Add the `AppState::pending_smartshift_writes` method**

In `crates/openlogi-gui/src/state.rs`, inside `impl AppState`, add this method after `current_record` (after its closing brace, line ~212):

```rust
    /// Compute and register the SmartShift sensitivities that must be written
    /// to freshly-connected devices, returning the `(route, value)` pairs the
    /// caller dispatches to [`crate::hardware::apply_smartshift_sensitivity_in_background`].
    ///
    /// Behaviour:
    /// - **Disk is the source of truth.** Reads the persisted value from a
    ///   fresh [`Config::load_or_default`] rather than the in-memory copy: the
    ///   CLI's `--save` may have written it after the GUI loaded its copy.
    /// - **Apply once per connection.** Every connected `config_key` is marked
    ///   in `smartshift_applied` after evaluation (whether or not it had a
    ///   value or the dispatch later succeeds), so a sleeping/failing device is
    ///   not hammered every poll tick. Pruning the set to the connected keys
    ///   means a disconnect→reconnect re-applies.
    /// - **No clobber.** Applied values are synced back into the in-memory
    ///   `self.config` so the GUI's next full-file `save_atomic()` preserves
    ///   them instead of overwriting the file without them.
    /// - **Steady-state cheap.** When every connected key is already evaluated,
    ///   returns early without touching the disk.
    ///
    /// Call this on every inventory refresh, after `refresh_inventories`.
    pub fn pending_smartshift_writes(&mut self) -> Vec<(DeviceRoute, u8)> {
        // Connected = online records that carry a route (offline records can't
        // be written to).
        let connected: Vec<String> = self
            .device_list
            .iter()
            .filter(|r| r.online && r.route.is_some())
            .map(|r| r.config_key.clone())
            .collect();

        // Drop applied keys that are no longer connected so a reconnect retries.
        self.smartshift_applied
            .retain(|key| connected.contains(key));

        // Steady state: nothing new connected since last evaluation — skip the
        // disk read entirely.
        if connected
            .iter()
            .all(|k| self.smartshift_applied.contains(k))
        {
            return Vec::new();
        }

        let disk = Config::load_or_default().unwrap_or_default();
        let pending = smartshift_pending(&disk, &connected, &self.smartshift_applied);

        let mut writes = Vec::new();
        for (key, value) in pending {
            // Sync into the in-memory config so a later full-file save keeps it.
            self.config.set_smartshift_sensitivity(&key, Some(value));
            if let Some(route) = self
                .device_list
                .iter()
                .find(|r| r.config_key == key)
                .and_then(|r| r.route.clone())
            {
                writes.push((route, value));
            }
        }

        // Mark every connected key evaluated (including no-value devices) so we
        // don't re-read the config file on every subsequent poll tick.
        for key in connected {
            self.smartshift_applied.insert(key);
        }

        writes
    }
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p openlogi-gui smartshift_pending`
Expected: PASS (6 tests).

- [ ] **Step 9: Crate test + clippy**

Run: `cargo test -p openlogi-gui && cargo clippy -p openlogi-gui --all-targets -- -D warnings`
Expected: all tests pass, clippy clean.

> If clippy flags `smartshift_applied` or `pending_smartshift_writes` as dead code, that is expected until Task 3c wires the method in — the module-level `#![allow(dead_code, ...)]` at the top of `state.rs` (line 12) already covers fields; the method is `pub` so it is exempt. Do **not** add new allows.

- [ ] **Step 10: Commit**

```bash
git add crates/openlogi-gui/src/state.rs
git commit -m "feat(gui): decide pending SmartShift sensitivity writes per connection

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 3c: Wire into the inventory-refresh loop

**Files:**
- Modify: `crates/openlogi-gui/src/main.rs:249-257` (the `inventory_rx.recv()` arm)

- [ ] **Step 1: Dispatch pending writes after `refresh_inventories`**

In `crates/openlogi-gui/src/main.rs`, replace the `cx.update(|cx| { ... });` block inside the `Some(new_inv) = inventory_rx.recv()` arm (lines ~249-257) with:

```rust
                        let writes = cx.update(|cx| {
                            let cache = asset::AssetResolver::new();
                            let writes = cx.update_global::<AppState, _>(|state, _| {
                                state.refresh_inventories(&new_inv, &cache);
                                state.scanning = false;
                                state.pending_smartshift_writes()
                            });
                            #[cfg(target_os = "macos")]
                            platform::tray::set_device_status(&tray_status(cx));
                            writes
                        });
                        // Off the GPUI thread: re-apply each connected device's
                        // persisted SmartShift sensitivity (fresh channel,
                        // capture = None). Once per connection — see
                        // AppState::pending_smartshift_writes.
                        for (route, value) in writes {
                            hardware::apply_smartshift_sensitivity_in_background(
                                None,
                                Some(route),
                                value,
                            );
                        }
```

> `cx.update(...)` and `cx.update_global::<AppState, _>(...)` both return their closure's value, so threading `writes` out is just returning it from each closure. The hardware dispatch runs outside the `cx.update` closure (no GPUI borrow held during the thread spawn). `hardware` is already in scope via `mod hardware;` (main.rs:31).

- [ ] **Step 2: Build the GUI**

Run: `cargo build -p openlogi-gui`
Expected: builds clean. (If `DEVELOPER_DIR` is set to the old CLT path, run `unset DEVELOPER_DIR` first.)

- [ ] **Step 3: Crate clippy**

Run: `cargo clippy -p openlogi-gui --all-targets -- -D warnings`
Expected: clippy clean — and `smartshift_applied` / `pending_smartshift_writes` are now reachable, so any dead-code worry from 3b is resolved.

- [ ] **Step 4: Commit**

```bash
git add crates/openlogi-gui/src/main.rs
git commit -m "feat(gui): auto-apply persisted SmartShift sensitivity on connect

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Final verification

- [ ] **Step 1: Full workspace gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all tests pass; clippy clean (a benign `block v0.1.6` future-incompat warning from a transitive dependency is pre-existing and acceptable).

- [ ] **Step 2: Manual hardware check (MX Master 2S — requires a real device)**

```bash
# Persist a value:
cargo run -p openlogi-cli -- diag smartshift --sensitivity 20 --save
# Confirm config.toml gained the field:
cat ~/.config/openlogi/config.toml   # expect: smartshift_sensitivity = 20 under [devices.<key>]
# Change it on the device out-of-band, then launch the GUI and confirm it
# re-applies (read back):
cargo run -p openlogi-gui            # connect the mouse; within ~2s it re-applies
cargo run -p openlogi-cli -- diag smartshift   # read-back shows sensitivity = 20
```

- [ ] **Step 3: Manual clobber check**

With the GUI running, run `cargo run -p openlogi-cli -- diag smartshift --sensitivity 30 --save`, then change a setting in the GUI (forcing a `save_atomic()`), and confirm `smartshift_sensitivity = 30` survives in `config.toml`.

---

## Spec coverage check (self-review — done at plan-writing time)

- Config field + accessors → Task 1. ✓
- `SCHEMA_VERSION` stays 1 (additive optional field) → unchanged; verified no edit to line 25. ✓
- Value range 1–255; `Some(0)` skipped → `smartshift_pending` filters `v != 0` (Task 3b), CLI rejects `0` (pre-existing, smartshift.rs:28). ✓
- CLI `--save` with `requires = "sensitivity"` → Task 2. ✓
- `first_online_device` returns config key; callers updated → Task 2 Steps 5-6. ✓
- GUI background write fn → Task 3a. ✓
- Pure decision fn + state field + disk-source-of-truth + clobber sync + prune → Task 3b. ✓
- Insertion point in `main.rs` recv arm → Task 3c. ✓
- Apply-once + mark-on-failure (no per-tick hammer) → `pending_smartshift_writes` marks all connected keys + steady-state early return → Task 3b Step 7. ✓
- All tests from the spec's Testing section → Task 1 (3 config tests), Task 2 (3 parse tests), Task 3b (6 `smartshift_pending` tests), manual hardware/clobber checks in Final verification. ✓
- Docs constraint (no edits to USAGE/CONFIGURATION/DEVELOPMENT) → no task touches them. ✓
