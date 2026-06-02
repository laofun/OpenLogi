# SmartShift Sensitivity Adjustment — Design

**Date:** 2026-06-02
**Branch:** `feat/smartshift-0x2110`
**Status:** Approved (design)

## Goal

Let `openlogi diag smartshift` write the SmartShift auto-disengage sensitivity
to a device via a new `--sensitivity N` flag, on top of the existing toggle
round-trip. This works on both the `0x2111` Enhanced and the `0x2110` Legacy
(MX Master 2S) SmartShift features.

This reverses the explicit "sensitivity adjustment is out of scope (YAGNI)"
note in the earlier `2026-06-02-smartshift-0x2110-design.md`: the user has since
asked to be able to adjust sensitivity from the CLI.

## Background

On the `0x2110 SmartShiftWheel` feature, the `auto_disengage` byte is the number
of quarter-turns-per-second the wheel must spin before the ratchet auto-releases
into free-spin. Smaller value = releases sooner = "more sensitive". The wire
semantics:

- `0x01..=0xfe` — auto-disengage threshold.
- `0xff` — permanent ratchet (never auto-disengage).
- `0x00` — "no change" (same effect as passing `None`); writing it is a silent
  no-op, which is confusing, so the CLI rejects it.

On `0x2111 SmartShiftWheelEnhanced`, `set_status(mode, sensitivity)` carries the
same sensitivity byte directly.

Today `SmartShift::set_mode` (in `crates/openlogi-hid/src/write.rs`) ignores its
`sensitivity` argument on the Legacy arm — it passes
`set_ratchet_control_mode(Some(wheel), None, None)`, so only the mode is written.
There is currently no code path that writes sensitivity on the Legacy feature.

## Behavior (decided)

1. **`--sensitivity N` sets the sensitivity only and keeps the current mode.**
   It does NOT toggle Free/Ratchet. Each invocation does one clear thing.
2. **`N` accepts `1..=255`; `0` is rejected** with a clear error (on `0x2110`
   it is a silent no-op). `255` = permanent ratchet, documented in the flag help.
3. **`--sensitivity` and `--leave-flipped` are mutually exclusive** (they belong
   to two different modes of the command).
4. When `--sensitivity` is absent, the command keeps its existing toggle
   round-trip behavior unchanged.

## Design

### 1. `crates/openlogi-hid/src/write.rs`

**New method on the `SmartShift` enum** — write sensitivity while preserving the
current mode, explicitly (approach A: read the current mode, then write
mode + sensitivity on both arms — no reliance on firmware treating
`wheel_mode = None` as "keep current"):

```rust
/// Write a new auto-disengage `sensitivity`, preserving the current mode.
/// Reads the current mode first, then writes it back together with the new
/// sensitivity (both arms write the mode explicitly — no reliance on the
/// device treating a `None` wheel-mode as "keep current").
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

**New public function** — open the device, write sensitivity, read back, return
the post-write status so the caller can display and verify it:

```rust
/// Set the SmartShift auto-disengage sensitivity on `route`, preserving the
/// current mode. Returns the read-back status after the write.
///
/// `FeatureUnsupported` when the device exposes neither `0x2111` nor `0x2110`.
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

The existing `set_mode`, `toggle_smartshift`, `toggle_smartshift_on_channel`,
and `get_smartshift_status` are **unchanged** — the proven toggle path is not
touched. No shared-channel (`_on`) variant is added (the GUI does not need it).

### 2. `crates/openlogi-cli/src/cmd/diag/smartshift.rs`

Add the flag to `SmartshiftArgs`:

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

`run()` branches up front: when `sensitivity` is `Some(n)`, run the
set-sensitivity flow and return; otherwise fall through to the existing toggle
round-trip (unchanged).

Set-sensitivity flow:

```rust
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
            before.mode, after.mode
        );
    }

    println!("✓ SmartShift sensitivity set to {n} (mode {:?} preserved)", after.mode);
    return Ok(());
}
```

### Error handling

- `n == 0` → `anyhow::bail!` before touching the device.
- Read-back sensitivity mismatch → `bail!` (stricter than the DPI path, which
  only warns; a diagnostic command should fail loudly).
- Mode changed unexpectedly → `bail!` (catches a firmware that doesn't honor
  "keep mode").
- Device exposes no SmartShift feature → `WriteError::FeatureUnsupported`
  propagates unchanged.

## Testing

- **CI / unit:** the new logic is mostly device I/O; the testable piece is the
  `n == 0` rejection. The existing `write::tests` and `smartshift::tests` stay
  green. (No new pure-logic helper is introduced that warrants a dedicated test
  beyond what already exists.)
- **Manual smoke test on the MX Master 2S** (the project's standard for device
  writes): run `diag smartshift --sensitivity N` for a couple of values, confirm
  the read-back matches and the mode is preserved, and confirm the wheel feel
  changes on the hardware. Restore the original value afterwards.

## Out of scope

- GUI surface for sensitivity (slider/setting). The flag is a diagnostic/CLI
  tool only.
- Persisting sensitivity in `config.toml`.
- A shared-channel (`_on`) fast path.
- Changing the toggle round-trip behavior.

## Files touched

- Modify: `crates/openlogi-hid/src/write.rs`
- Modify: `crates/openlogi-cli/src/cmd/diag/smartshift.rs`

Not touched: `openlogi-hidpp` fork, GUI, config schema, `docs/USAGE.md`,
`docs/CONFIGURATION.md`, `docs/DEVELOPMENT.md`.
