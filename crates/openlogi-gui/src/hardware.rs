//! Hardware-side actions invoked from both the GPUI thread (slider release)
//! and the OS-event hook thread (bound button press).
//!
//! Each call spawns a one-shot tokio runtime on a dedicated OS thread —
//! cheap at the cadence these fire at (≤ once per slider release / button
//! press) and avoids holding a long-lived async runtime alongside GPUI's
//! executor.
//!
//! When the HID++ capture session already has the target device open, these
//! reuse that channel ([`openlogi_hid::CaptureChannel`]) instead of
//! re-enumerating and opening a fresh one — the dominant cost of a write. The
//! transient open is kept as a fallback for callers (e.g. the CGEventTap hook)
//! firing while no session is connected.

use std::time::Duration;

use openlogi_hid::{
    CaptureChannel, DeviceRoute, DpiInfo, SharedChannel, SmartShiftMode, SmartShiftStatus,
    WriteError,
};
use tracing::{debug, warn};

/// Upper bound on a single HID++ write. `hidpp` has no request timeout of its
/// own, so without this an asleep / unresponsive device would hang (and leak)
/// this background thread forever; a write to a live device completes in well
/// under a second.
const WRITE_BUDGET: Duration = Duration::from_secs(5);

#[must_use]
pub fn smartshift_percent_to_raw(percent: u8) -> u8 {
    let clamped = u16::from(percent.min(100));
    let raw = 1 + ((clamped * 254 + 50) / 100);
    u8::try_from(raw).unwrap_or(255)
}

#[must_use]
pub fn smartshift_raw_to_percent(raw: u8) -> u8 {
    let raw = u16::from(raw.max(1));
    let percent = ((raw - 1) * 100 + 127) / 254;
    u8::try_from(percent).unwrap_or(100)
}

/// Read the current DPI and supported DPI values on a background worker.
///
/// This helper is intentionally blocking so GPUI callers can run it via
/// `cx.background_spawn` without making the UI thread own a Tokio runtime.
pub fn read_dpi_info_blocking(target: &DeviceRoute) -> Result<DpiInfo, WriteError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| WriteError::Hidpp(format!("tokio runtime init failed: {e}")))?;

    rt.block_on(async {
        tokio::time::timeout(WRITE_BUDGET, openlogi_hid::get_dpi_info(target))
            .await
            .map_err(|_| WriteError::Hidpp("DPI info read timed out".into()))?
    })
}

/// Apply any persisted SmartShift settings to the device, then read back its
/// live mode + auto-disengage sensitivity, on a background worker. Companion to
/// [`read_dpi_info_blocking`]: intentionally blocking so GPUI callers can run it
/// on a dedicated OS thread without the UI thread owning a Tokio runtime.
///
/// `persist_mode` / `persist_sensitivity` are the values stored in
/// `config.toml` for this device (`None` when never persisted). When `Some`,
/// each is written best-effort — a failed write is logged and skipped, never
/// aborting the read-back — so the panel always settles on the device's true
/// live state even if the apply didn't land. `(None, None)` reduces to a plain
/// status read (the read-only path). The whole apply-then-read sequence shares
/// one [`WRITE_BUDGET`] timeout.
pub fn sync_smartshift_status_blocking(
    target: &DeviceRoute,
    persist_mode: Option<bool>,
    persist_sensitivity: Option<u8>,
) -> Result<SmartShiftStatus, WriteError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| WriteError::Hidpp(format!("tokio runtime init failed: {e}")))?;

    rt.block_on(async {
        tokio::time::timeout(WRITE_BUDGET, async {
            let applied_mode = match persist_mode {
                Some(ratchet) => {
                    let mode = if ratchet {
                        SmartShiftMode::Ratchet
                    } else {
                        SmartShiftMode::Free
                    };
                    match openlogi_hid::set_smartshift_mode(target, mode).await {
                        Ok(_) => Some(mode),
                        Err(e) => {
                            warn!(error = ?e, "auto-apply SmartShift mode failed; reading live state");
                            None
                        }
                    }
                }
                None => None,
            };
            let applied_sensitivity = match persist_sensitivity {
                Some(raw) => match openlogi_hid::set_smartshift_sensitivity(target, raw).await {
                    Ok(_) => Some(raw),
                    Err(e) => {
                        warn!(error = ?e, "auto-apply SmartShift sensitivity failed; reading live state");
                        None
                    }
                },
                None => None,
            };
            let read_back = openlogi_hid::get_smartshift_status(target).await?;
            Ok(seat_smartshift_status(read_back, applied_mode, applied_sensitivity))
        })
        .await
        .map_err(|_| WriteError::Hidpp("SmartShift sync timed out".into()))?
    })
}

/// Seat the panel's view of SmartShift state, preferring the values we *just
/// applied* over the device read-back. A successful HID++ write is the
/// authoritative source for the field it set — the read-back exists only to
/// recover fields we did not touch. This matters on the MX Master 2S (`0x2110`),
/// whose immediate post-write read can return a stale mode/sensitivity (see
/// `openlogi-hid`'s `smartshift_backend`); trusting the applied value sidesteps
/// that race. `None` for either argument (nothing persisted, or the write
/// failed) falls back to the read-back for that field.
fn seat_smartshift_status(
    read_back: SmartShiftStatus,
    applied_mode: Option<SmartShiftMode>,
    applied_sensitivity: Option<u8>,
) -> SmartShiftStatus {
    SmartShiftStatus {
        mode: applied_mode.unwrap_or(read_back.mode),
        sensitivity: applied_sensitivity.unwrap_or(read_back.sensitivity),
    }
}

/// Clone out the capture session's channel when it reaches `route`. `None` when
/// no capture session is connected or the open channel points at a different
/// device.
fn reusable_channel(
    capture: Option<&CaptureChannel>,
    route: &DeviceRoute,
) -> Option<SharedChannel> {
    capture?
        .read()
        .ok()
        .and_then(|slot| (*slot).clone())
        .filter(|chan| chan.matches(route))
}

/// Spawn an OS thread that toggles SmartShift (free ↔ ratchet) on the
/// device at `target` via `openlogi_hid::toggle_smartshift`. Returns
/// immediately; failures (incl. devices that expose neither `0x2111` nor
/// the older `0x2110` SmartShift feature) are logged.
pub fn toggle_smartshift_in_background(
    capture: Option<&CaptureChannel>,
    target: Option<DeviceRoute>,
) {
    let Some(target) = target else {
        debug!("no target device — SmartShift toggle skipped");
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
                warn!(error = %e, "tokio runtime init failed; SmartShift toggle skipped");
                return;
            }
        };
        let result = rt.block_on(async {
            tokio::time::timeout(WRITE_BUDGET, async {
                match &shared {
                    Some(shared) => openlogi_hid::toggle_smartshift_on(shared).await,
                    None => openlogi_hid::toggle_smartshift(&target).await,
                }
            })
            .await
        });
        let index = target.device_index();
        match result {
            Ok(Ok(mode)) => debug!(index, ?mode, reused, "SmartShift toggled"),
            Ok(Err(e)) => warn!(error = ?e, "SmartShift toggle failed"),
            Err(_) => warn!(
                index,
                "SmartShift toggle timed out (device asleep/unresponsive)"
            ),
        }
    });
}

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
            Ok(Ok(status)) => {
                debug!(index, ?mode, applied = ?status.mode, reused, "SmartShift mode set");
            }
            Ok(Err(e)) => warn!(error = ?e, "SmartShift mode set failed"),
            Err(_) => warn!(
                index,
                "SmartShift mode set timed out (device asleep/unresponsive)"
            ),
        }
    });
}

/// Spawn an OS thread that writes a SmartShift auto-disengage `value` (1–255)
/// to the device at `target` via `openlogi_hid::set_smartshift_sensitivity`,
/// preserving the current mode. Invoked when the user releases the SmartShift
/// sensitivity slider. Returns immediately; failures (incl. devices exposing
/// neither `0x2111` nor the older `0x2110` SmartShift feature) are logged,
/// never retried within the call.
///
/// Unlike the toggle/DPI writers this always opens a fresh channel — there is
/// no channel-reusing `set_smartshift_sensitivity_on` variant.
///
/// `target == None` is a no-op (dev environment without a real device).
pub fn apply_smartshift_sensitivity_in_background(target: Option<DeviceRoute>, value: u8) {
    let Some(target) = target else {
        debug!(
            value,
            "no target device — SmartShift sensitivity apply skipped"
        );
        return;
    };
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

/// Spawn an OS thread that writes `dpi` to the device at `target` via
/// `openlogi_hid::set_dpi`. Returns immediately; failures are logged.
///
/// `target == None` is a no-op (dev environment without a real device).
pub fn write_dpi_in_background(
    capture: Option<&CaptureChannel>,
    target: Option<DeviceRoute>,
    dpi: u32,
) {
    let Some(target) = target else {
        debug!(dpi, "no target device — DPI write skipped");
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
                warn!(error = %e, "tokio runtime init failed; DPI write skipped");
                return;
            }
        };
        // All device-supported DPI values fit in HID++'s u16 wire field. The
        // saturating fallback exists only for type-system exhaustiveness.
        let dpi_u16 = u16::try_from(dpi).unwrap_or(u16::MAX);
        let result = rt.block_on(async {
            tokio::time::timeout(WRITE_BUDGET, async {
                match &shared {
                    Some(shared) => openlogi_hid::set_dpi_on(shared, dpi_u16).await,
                    None => openlogi_hid::set_dpi(&target, dpi_u16).await,
                }
            })
            .await
        });
        match result {
            Ok(Ok(())) => debug!(
                index = target.device_index(),
                dpi = dpi_u16,
                reused,
                "DPI written to device"
            ),
            Ok(Err(e)) => warn!(error = ?e, "DPI write failed"),
            Err(_) => warn!(
                dpi = dpi_u16,
                "DPI write timed out (device asleep/unresponsive)"
            ),
        }
    });
}

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

    #[test]
    fn seat_prefers_applied_mode_over_stale_readback() {
        // The MX Master 2S can answer a read issued right after a write with a
        // stale value. A successful mode write is authoritative, so the applied
        // mode must win over the read-back.
        let read_back = SmartShiftStatus {
            mode: SmartShiftMode::Ratchet,
            sensitivity: 30,
        };
        let seated = seat_smartshift_status(read_back, Some(SmartShiftMode::Free), None);
        assert_eq!(seated.mode, SmartShiftMode::Free);
        assert_eq!(seated.sensitivity, 30);
    }

    #[test]
    fn seat_prefers_applied_sensitivity_over_stale_readback() {
        let read_back = SmartShiftStatus {
            mode: SmartShiftMode::Free,
            sensitivity: 30,
        };
        let seated = seat_smartshift_status(read_back, None, Some(200));
        assert_eq!(seated.mode, SmartShiftMode::Free);
        assert_eq!(seated.sensitivity, 200);
    }

    #[test]
    fn seat_falls_back_to_readback_when_nothing_applied() {
        // Nothing persisted (or both writes failed) → trust the device entirely.
        let read_back = SmartShiftStatus {
            mode: SmartShiftMode::Ratchet,
            sensitivity: 42,
        };
        let seated = seat_smartshift_status(read_back, None, None);
        assert_eq!(seated.mode, SmartShiftMode::Ratchet);
        assert_eq!(seated.sensitivity, 42);
    }
}
