# SmartShift Sensitivity Adjustment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `--sensitivity N` flag to `openlogi diag smartshift` that writes the SmartShift auto-disengage sensitivity (on both the `0x2111` Enhanced and `0x2110` Legacy features) while preserving the current Free/Ratchet mode.

**Architecture:** Add one method (`set_sensitivity`) to the existing private `SmartShift` enum in `crates/openlogi-hid/src/write.rs` plus one public entry point (`set_smartshift_sensitivity`) that opens the device, writes, reads back, and returns the post-write status. The CLI command branches: when `--sensitivity` is given it runs a set-and-verify flow (no toggle); otherwise it keeps the existing toggle round-trip untouched. The proven toggle path and the `openlogi-hidpp` fork are not modified.

**Tech Stack:** Rust (edition 2024), `tokio` async, `clap` derive, the vendored `hidpp` fork. Build/test on this machine require `DEVELOPER_DIR=/Library/Developer/CommandLineTools` (no full Xcode installed).

**Reference spec:** `docs/superpowers/specs/2026-06-02-smartshift-sensitivity-design.md`

**Pre-commit gate (run before every commit):**
```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

---

## Background the engineer needs

The `SmartShift` enum (private, in `write.rs`) already wraps whichever SmartShift
feature a device exposes and exposes async `open`, `status`, and `set_mode`:

- `Enhanced(Arc<SmartShiftFeatureV0>)` — the `0x2111` feature from
  `crate::smartshift`. Write fn: `set_status(mode: SmartShiftMode, sensitivity: u8)`.
- `Legacy(Arc<SmartShiftFeature>)` — the fork's `0x2110` feature from
  `hidpp::feature::smartshift`. Write fn:
  `set_ratchet_control_mode(wheel_mode: Option<WheelMode>, auto_disengage: Option<u8>, auto_disengage_default: Option<u8>)`
  (each `None` keeps the existing value).

`SmartShiftStatus { mode: SmartShiftMode, sensitivity: u8 }` is the normalised
read result; `status()` already fills `sensitivity` from `auto_disengage` on the
Legacy arm. `SmartShiftMode { Free, Ratchet }`. `WheelMode { Freespin = 1,
Ratchet = 2 }` is `#[non_exhaustive]`.

Key facts that shape the design:

- **`auto_disengage` semantics:** `0x01..=0xfe` = quarter-turns/sec threshold to
  auto-release ratchet (smaller = more sensitive); `0xff` = permanent ratchet;
  `0x00` = "no change" (silent no-op) — so the CLI rejects `0`.
- **`WriteError` does NOT derive `PartialEq`** — any test must use `matches!`,
  never `==`.
- Map a `Hidpp20Error` into `WriteError` with
  `.map_err(|e| WriteError::Hidpp(format!("{e:?}")))`.
- The new backend logic is device I/O, which has no automated test in this repo
  (same as DPI / toggle). Verification is the compile + clippy + existing-test
  gate (Tasks 1–2) plus a manual smoke test on the MX Master 2S (Task 3). This
  matches CLAUDE.md: "DPI writes / SmartShift are smoke-tested manually."

---

## File Structure

- **Modify:** `crates/openlogi-hid/src/write.rs`
  - add `SmartShift::set_sensitivity(&self, value: u8)` method (Task 1)
  - add public `set_smartshift_sensitivity(route, value) -> Result<SmartShiftStatus, WriteError>` (Task 1)
- **Modify:** `crates/openlogi-cli/src/cmd/diag/smartshift.rs`
  - add `sensitivity: Option<u8>` arg + `conflicts_with` on `leave_flipped` (Task 2)
  - branch `run()` into the set-sensitivity flow vs the existing toggle flow (Task 2)

No changes to the `openlogi-hidpp` fork, the GUI, the config schema, or
`docs/USAGE.md` / `docs/CONFIGURATION.md` / `docs/DEVELOPMENT.md`.

---

### Task 1: Backend — `set_sensitivity` + public entry point

**Files:**
- Modify: `crates/openlogi-hid/src/write.rs`

- [ ] **Step 1: Add the `set_sensitivity` method to the `SmartShift` impl**

In `crates/openlogi-hid/src/write.rs`, inside `impl SmartShift { ... }`, directly
after the existing `set_mode` method (which currently ends around write.rs:197),
add a new method:

```rust
    /// Write a new auto-disengage `sensitivity`, preserving the current mode.
    /// Reads the current mode first, then writes it back together with the new
    /// sensitivity — both arms write the mode explicitly, so we never rely on
    /// the device treating a `None` wheel-mode as "keep current".
    async fn set_sensitivity(&self, value: u8) -> Result<(), WriteError> {
        let SmartShiftStatus { mode, .. } = self.status().await?;
        match self {
            Self::Enhanced(feature) => feature
                .set_status(mode, value)
                .await
                .map_err(|e| WriteError::Hidpp(format!("{e:?}"))),
            Self::Legacy(feature) => {
                let wheel = match mode {
                    SmartShiftMode::Free => WheelMode::Freespin,
                    SmartShiftMode::Ratchet => WheelMode::Ratchet,
                };
                feature
                    .set_ratchet_control_mode(Some(wheel), Some(value), None)
                    .await
                    .map_err(|e| WriteError::Hidpp(format!("{e:?}")))
            }
        }
    }
```

(`SmartShiftStatus`, `SmartShiftMode`, and `WheelMode` are already imported at the
top of the file — no new imports needed.)

- [ ] **Step 2: Add the public `set_smartshift_sensitivity` function**

In `crates/openlogi-hid/src/write.rs`, directly after the existing
`get_smartshift_status` function (which ends around write.rs:230), add:

```rust
/// Set the SmartShift auto-disengage sensitivity on `route`, preserving the
/// current mode. Returns the read-back status after the write so the caller can
/// display and verify it.
///
/// `value` is written verbatim: `0x01..=0xfe` is the auto-disengage threshold
/// (smaller = releases sooner / more sensitive) and `0xff` is permanent ratchet.
/// Callers should reject `0`, which the device treats as "no change".
///
/// `FeatureUnsupported` when the device exposes neither HID++ `0x2111`
/// (MX Master 3 / 3S) nor the older `0x2110` (MX Master 2S).
pub async fn set_smartshift_sensitivity(
    route: &DeviceRoute,
    value: u8,
) -> Result<SmartShiftStatus, WriteError> {
    let index = route.device_index();
    with_route(route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        let smartshift = SmartShift::open(&mut device).await?;
        smartshift.set_sensitivity(value).await?;
        smartshift.status().await
    })
    .await
}
```

- [ ] **Step 3: Re-export `set_smartshift_sensitivity` from `lib.rs`**

`crates/openlogi-hid/src/lib.rs` re-exports the `write` module's public items via
an explicit list (lib.rs:29–32), not a glob. Add `set_smartshift_sensitivity` to
that list so the CLI can call `openlogi_hid::set_smartshift_sensitivity` the same
way it calls `get_smartshift_status`:

```rust
pub use write::{
    FeatureEntry, SharedChannel, WriteError, dump_features, get_dpi, get_smartshift_status,
    set_dpi, set_dpi_on, set_smartshift_sensitivity, toggle_smartshift, toggle_smartshift_on,
};
```

- [ ] **Step 4: Run the full gate**

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected: fmt clean; clippy clean (no unused-symbol warning — `set_sensitivity`
is reached through `set_smartshift_sensitivity`, which is `pub`); all existing
tests pass (no new tests in this task — the code is device I/O).

- [ ] **Step 5: Commit**

```sh
git add crates/openlogi-hid/src/write.rs crates/openlogi-hid/src/lib.rs
git commit -m "feat(hid): write SmartShift sensitivity preserving mode

Add SmartShift::set_sensitivity (reads current mode, rewrites mode +
auto_disengage on both the 0x2111 and 0x2110 arms) and a public
set_smartshift_sensitivity entry point that reads back the post-write
status. The toggle path is untouched.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: CLI — `--sensitivity` flag + set-and-verify flow

**Files:**
- Modify: `crates/openlogi-cli/src/cmd/diag/smartshift.rs`

- [ ] **Step 1: Add the `sensitivity` arg and conflict**

In `crates/openlogi-cli/src/cmd/diag/smartshift.rs`, replace the
`SmartshiftArgs` struct (smartshift.rs:8–14) with:

```rust
#[derive(Debug, Args)]
pub struct SmartshiftArgs {
    /// Leave the wheel in the toggled mode (skip the second toggle that
    /// restores the original). Useful for visually verifying the flip.
    #[arg(long, conflicts_with = "sensitivity")]
    pub leave_flipped: bool,

    /// Set the auto-disengage sensitivity (1-255; 255 = permanent ratchet)
    /// instead of toggling. Keeps the current Free/Ratchet mode.
    #[arg(long, value_name = "N")]
    pub sensitivity: Option<u8>,
}
```

- [ ] **Step 2: Branch `run()` into the set-sensitivity flow**

In the same file, replace the body of `run()` (smartshift.rs:16–60) with the
version below. The toggle branch is the existing logic verbatim; the new
set-sensitivity branch runs first and returns early.

```rust
pub async fn run(args: SmartshiftArgs) -> Result<()> {
    let (route, name) = first_online_device().await?;
    println!("device: {name} ({route})");

    if let Some(n) = args.sensitivity {
        if n == 0 {
            anyhow::bail!("sensitivity must be 1-255 (0 means \"no change\")");
        }

        let before = openlogi_hid::get_smartshift_status(&route)
            .await
            .context("read SmartShift status")?;
        println!(
            "  current: mode={:?} sensitivity={}",
            before.mode, before.sensitivity
        );

        let after = openlogi_hid::set_smartshift_sensitivity(&route, n)
            .await
            .context("set SmartShift sensitivity")?;
        println!(
            "  read-back: mode={:?} sensitivity={}",
            after.mode, after.sensitivity
        );

        if after.sensitivity != n {
            anyhow::bail!(
                "SmartShift sensitivity write not applied: requested {n}, device reports {}",
                after.sensitivity
            );
        }
        if after.mode != before.mode {
            anyhow::bail!(
                "SmartShift mode changed unexpectedly: was {:?}, now {:?}",
                before.mode,
                after.mode
            );
        }

        println!(
            "✓ SmartShift sensitivity set to {n} (mode {:?} preserved)",
            after.mode
        );
        return Ok(());
    }

    let before = openlogi_hid::get_smartshift_status(&route)
        .await
        .context("read SmartShift status")?;
    println!(
        "  current: mode={:?} sensitivity={}",
        before.mode, before.sensitivity
    );

    let new_mode = openlogi_hid::toggle_smartshift(&route)
        .await
        .context("toggle SmartShift")?;
    println!("  toggled to: {new_mode:?}");

    let after = openlogi_hid::get_smartshift_status(&route)
        .await
        .context("read SmartShift after toggle")?;
    println!(
        "  read-back: mode={:?} sensitivity={}",
        after.mode, after.sensitivity
    );

    if after.mode == before.mode {
        anyhow::bail!(
            "SmartShift toggle had no effect: still {:?} after write",
            before.mode
        );
    }

    if args.leave_flipped {
        println!("✓ SmartShift toggle OK (wheel left in {new_mode:?})");
        return Ok(());
    }

    println!("  restoring mode: {:?}", before.mode);
    openlogi_hid::toggle_smartshift(&route)
        .await
        .context("restore SmartShift")?;

    println!("✓ SmartShift round-trip OK");
    Ok(())
}
```

- [ ] **Step 3: Run the full gate**

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
Expected: fmt clean; clippy clean; all tests pass.

- [ ] **Step 4: Verify the flag is wired (help + conflict), no device needed**

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo run -p openlogi --release -- diag smartshift --help 2>&1 | tail -20
```
Expected: help lists `--sensitivity <N>` with the "1-255; 255 = permanent
ratchet" text and `--leave-flipped`.

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo run -p openlogi --release -- diag smartshift --sensitivity 5 --leave-flipped 2>&1 | tail -5
```
Expected: clap exits with an error that `--leave-flipped` cannot be used with
`--sensitivity` (the `conflicts_with` is enforced before any device access).

- [ ] **Step 5: Commit**

```sh
git add crates/openlogi-cli/src/cmd/diag/smartshift.rs
git commit -m "feat(cli): diag smartshift --sensitivity N

Set the SmartShift auto-disengage sensitivity (1-255; 255 = permanent
ratchet) without toggling, preserving the current mode. Rejects 0 (a
device no-op), conflicts with --leave-flipped, and verifies the
read-back value and mode.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Manual smoke test on the MX Master 2S

**Files:** none (verification only)

Requires the physical MX Master 2S connected over Bluetooth, with Logi Options+
quit and Input Monitoring granted to the terminal (a probe failure otherwise
shows `Hid("Failed to open device")`).

- [ ] **Step 1: Read the current value (baseline)**

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo run -p openlogi --release -- diag smartshift --sensitivity 1
```
This writes `1` (likely the current value, so a safe no-op write). Note the
`current:` line's sensitivity so you can restore it at the end.
Expected output shape:
```
device: MX Master 2S (direct 046d:b019)
  current: mode=Ratchet sensitivity=1
  read-back: mode=Ratchet sensitivity=1
✓ SmartShift sensitivity set to 1 (mode Ratchet preserved)
```

- [ ] **Step 2: Set a different value and confirm the read-back + preserved mode**

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo run -p openlogi --release -- diag smartshift --sensitivity 50
```
Expected: `read-back: ... sensitivity=50`, the `mode=` unchanged from the
`current:` line, and `✓ SmartShift sensitivity set to 50 (mode ... preserved)`.

- [ ] **Step 3: Confirm the `0` rejection**

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo run -p openlogi --release -- diag smartshift --sensitivity 0 2>&1 | tail -3
```
Expected: exits non-zero with `sensitivity must be 1-255 (0 means "no change")`
and never touches the device.

- [ ] **Step 4: Feel the change on the hardware (optional but recommended)**

Set a low value (e.g. `--sensitivity 5`) and a high value (e.g.
`--sensitivity 200`), each time spinning the wheel by hand to feel whether it
auto-releases into free-spin sooner or later. Then restore the original value
noted in Step 1:
```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo run -p openlogi --release -- diag smartshift --sensitivity <original>
```

- [ ] **Step 5: Confirm no regression on the toggle path**

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
cargo run -p openlogi --release -- diag smartshift
```
Expected: the unchanged round-trip — `✓ SmartShift round-trip OK`.

---

## Notes for the executor

- **Branch:** work continues on `feat/smartshift-0x2110` (the sensitivity spec
  commit `b000c28` is the latest parent). Do not push without explicit request.
- **Why `DEVELOPER_DIR`:** this machine has only Command Line Tools, not full
  Xcode; the repo's `.cargo/config.toml` defaults `DEVELOPER_DIR` to a
  non-existent `Xcode.app` path with `force = false`, so exporting the CLT path
  overrides it. The CLI builds fine under CLT; the GUI crate cannot build here.
- **Do not** modify `crates/openlogi-hidpp/**` (vendored third-party fork) — its
  `0x2110` wrapper is consumed as-is.
- Tasks 1–2 are integration (compile + clippy + existing tests + a no-device help
  check). Task 3 is manual hardware verification and cannot run in CI.
