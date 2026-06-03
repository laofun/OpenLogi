//! SmartShift dual-backend — the fork-specific abstraction that lets one code
//! path drive both the modern `0x2111 SmartShiftWheelEnhanced` (MX Master 3 /
//! 3S) and the original `0x2110 SmartShiftWheel` (MX Master 2S).
//!
//! **Why this module exists separately from [`crate::write`].** Upstream
//! supports only `0x2111`; the fork adds the `0x2110` legacy path the older
//! MX Master 2S needs. Keeping that divergence in its own file shrinks the
//! merge surface against upstream: [`crate::write`]'s SmartShift entry points
//! call [`SmartShift::open`] in one line each, and all the fork-only logic —
//! the feature-probe fallback, the two wire encodings, and the
//! function-ID-shift gotcha — lives here.
//!
//! **The `0x2110` vs `0x2111` gotcha.** The two features are *not* a versioned
//! pair: their function IDs are shifted (e.g. `0x2111` getStatus is function 1,
//! `0x2110` getRatchetControlMode is function 0). They also keep the current
//! mode differently on a sensitivity write — see [`SmartShift::set_sensitivity`].

use std::sync::Arc;

use hidpp::device::Device;
use hidpp::feature::smartshift::{SmartShiftFeature, WheelMode};

use crate::smartshift::{SmartShiftFeatureV0, SmartShiftMode, SmartShiftStatus};
use crate::write::{WriteError, open_feature};

/// Whether a failure to open the `0x2111` Enhanced SmartShift feature should
/// trigger the `0x2110` legacy fallback. Only a missing-`0x2111` feature
/// qualifies; transport and protocol errors propagate unchanged so a real
/// failure is never masked by a second open attempt.
fn is_missing_enhanced(err: &WriteError) -> bool {
    matches!(
        err,
        WriteError::FeatureUnsupported { feature_hex } if *feature_hex == 0x2111
    )
}

/// Map the fork's `0x2110` [`WheelMode`] onto OpenLogi's [`SmartShiftMode`].
/// A future `#[non_exhaustive]` variant maps to [`SmartShiftMode::Ratchet`],
/// the "safe" clicky default OpenLogi uses elsewhere. (Reserved wire bytes
/// never reach here — the fork's `get_ratchet_control_mode` rejects them.)
fn wheel_mode_to_smartshift(wheel: WheelMode) -> SmartShiftMode {
    if matches!(wheel, WheelMode::Freespin) {
        SmartShiftMode::Free
    } else {
        SmartShiftMode::Ratchet
    }
}

/// Map OpenLogi's [`SmartShiftMode`] onto the fork's `0x2110` [`WheelMode`] —
/// the inverse of [`wheel_mode_to_smartshift`], used when writing the legacy
/// ratchet-control mode.
fn smartshift_to_wheel(mode: SmartShiftMode) -> WheelMode {
    match mode {
        SmartShiftMode::Free => WheelMode::Freespin,
        SmartShiftMode::Ratchet => WheelMode::Ratchet,
    }
}

/// Whichever SmartShift feature a device exposes, normalised onto
/// [`SmartShiftMode`]. Devices ship one or the other: MX Master 3 / 3S use the
/// `0x2111` Enhanced variant, the MX Master 2S uses the original `0x2110`.
pub(crate) enum SmartShift {
    /// `0x2111 SmartShiftWheelEnhanced`.
    Enhanced(Arc<SmartShiftFeatureV0>),
    /// `0x2110 SmartShiftWheel`.
    Legacy(Arc<SmartShiftFeature>),
}

impl SmartShift {
    /// Open whichever SmartShift feature the device exposes. Tries `0x2111`
    /// first; on a missing-`0x2111` error (and only that), retries with
    /// `0x2110`. Any other error from the first attempt propagates unchanged.
    pub(crate) async fn open(device: &mut Device) -> Result<Self, WriteError> {
        match open_feature::<SmartShiftFeatureV0>(device).await {
            Ok(feature) => Ok(Self::Enhanced(feature)),
            Err(err) if is_missing_enhanced(&err) => {
                let feature = open_feature::<SmartShiftFeature>(device).await?;
                Ok(Self::Legacy(feature))
            }
            Err(err) => Err(err),
        }
    }

    /// Read the current mode (and, for Enhanced, the sensitivity; for Legacy,
    /// the auto-disengage threshold reported as `sensitivity`).
    pub(crate) async fn status(&self) -> Result<SmartShiftStatus, WriteError> {
        match self {
            Self::Enhanced(feature) => feature
                .get_status()
                .await
                .map_err(|e| WriteError::Hidpp(format!("{e:?}"))),
            Self::Legacy(feature) => {
                let rcm = feature
                    .get_ratchet_control_mode()
                    .await
                    .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?;
                Ok(SmartShiftStatus {
                    mode: wheel_mode_to_smartshift(rcm.wheel_mode),
                    sensitivity: rcm.auto_disengage,
                })
            }
        }
    }

    /// Write a new mode. `sensitivity` is preserved on Enhanced; on Legacy the
    /// auto-disengage threshold is left unchanged (`None`).
    pub(crate) async fn set_mode(
        &self,
        mode: SmartShiftMode,
        sensitivity: u8,
    ) -> Result<(), WriteError> {
        match self {
            Self::Enhanced(feature) => feature
                .set_status(mode, sensitivity)
                .await
                .map_err(|e| WriteError::Hidpp(format!("{e:?}"))),
            Self::Legacy(feature) => feature
                .set_ratchet_control_mode(Some(smartshift_to_wheel(mode)), None, None)
                .await
                .map_err(|e| WriteError::Hidpp(format!("{e:?}"))),
        }
    }

    /// Write a new auto-disengage `sensitivity`, preserving the current mode.
    ///
    /// The two features keep the mode differently:
    /// - `0x2111` (Enhanced) `set_status` has no "keep current" sentinel — it
    ///   always takes a mode — so we read the current mode and write it back
    ///   alongside the new threshold.
    /// - `0x2110` (Legacy) `set_ratchet_control_mode` treats `wheel_mode = None`
    ///   as "leave unchanged" (the fork's documented contract), so we pass
    ///   `None` and touch only the threshold. Re-writing a just-read mode there
    ///   risks persisting a stale / misread value back to the device — on the
    ///   MX Master 2S that flipped Ratchet → Free.
    pub(crate) async fn set_sensitivity(&self, value: u8) -> Result<(), WriteError> {
        match self {
            Self::Enhanced(feature) => {
                let SmartShiftStatus { mode, .. } = self.status().await?;
                feature
                    .set_status(mode, value)
                    .await
                    .map_err(|e| WriteError::Hidpp(format!("{e:?}")))
            }
            Self::Legacy(feature) => feature
                .set_ratchet_control_mode(None, Some(value), None)
                .await
                .map_err(|e| WriteError::Hidpp(format!("{e:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_missing_enhanced, smartshift_to_wheel, wheel_mode_to_smartshift};
    use crate::smartshift::SmartShiftMode;
    use crate::write::WriteError;
    use hidpp::feature::smartshift::WheelMode;

    #[test]
    fn smartshift_and_wheel_mode_byte_encodings_match() {
        // The whole design relies on 0x2110 WheelMode and 0x2111
        // SmartShiftMode sharing one wire encoding (Free/Freespin = 1,
        // Ratchet = 2). If the fork ever renumbers WheelMode this fails loudly.
        assert_eq!(SmartShiftMode::Free.as_byte(), WheelMode::Freespin as u8);
        assert_eq!(SmartShiftMode::Ratchet.as_byte(), WheelMode::Ratchet as u8);
    }

    #[test]
    fn wheel_mode_maps_to_smartshift_mode() {
        assert_eq!(
            wheel_mode_to_smartshift(WheelMode::Freespin),
            SmartShiftMode::Free
        );
        assert_eq!(
            wheel_mode_to_smartshift(WheelMode::Ratchet),
            SmartShiftMode::Ratchet
        );
    }

    #[test]
    fn smartshift_to_wheel_round_trips() {
        // smartshift_to_wheel is the inverse of wheel_mode_to_smartshift.
        for mode in [SmartShiftMode::Free, SmartShiftMode::Ratchet] {
            assert_eq!(wheel_mode_to_smartshift(smartshift_to_wheel(mode)), mode);
        }
    }

    #[test]
    fn missing_enhanced_triggers_fallback() {
        assert!(is_missing_enhanced(&WriteError::FeatureUnsupported {
            feature_hex: 0x2111,
        }));
    }

    #[test]
    fn missing_legacy_does_not_trigger_fallback() {
        // A device missing 0x2110 must NOT loop back — it genuinely has no
        // SmartShift.
        assert!(!is_missing_enhanced(&WriteError::FeatureUnsupported {
            feature_hex: 0x2110,
        }));
    }

    #[test]
    fn transport_errors_do_not_trigger_fallback() {
        // Real failures must propagate, not be masked by a fallback attempt.
        assert!(!is_missing_enhanced(&WriteError::DeviceUnreachable {
            index: 0xff,
        }));
        assert!(!is_missing_enhanced(&WriteError::Hidpp("boom".into())));
    }
}
