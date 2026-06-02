//! HID++ writes back to the device — DPI and SmartShift.
//!
//! Each entry point takes a [`DeviceRoute`] and resolves it to an open channel
//! through [`open_route_channel`], so the same call works whether the device is
//! behind a Bolt receiver or attached directly (USB cable / Bluetooth). Each
//! call re-enumerates and re-opens — fine at the frequency this is invoked
//! (once per slider release) — unless a [`SharedChannel`] from the capture
//! session is reused.

use std::sync::Arc;

use hidpp::feature::smartshift::{SmartShiftFeature, WheelMode};
use hidpp::{channel::HidppChannel, device::Device, feature::CreatableFeature};
use thiserror::Error;
use tracing::debug;

use crate::adjustable_dpi::AdjustableDpiFeatureV0;
use crate::route::{DeviceRoute, open_route_channel};
use crate::smartshift::{SmartShiftFeatureV0, SmartShiftMode, SmartShiftStatus};

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("HID transport error")]
    Hid(#[from] async_hid::HidError),
    #[error("no connected device matched the route")]
    DeviceNotFound,
    #[error("device at index {index:#04x} did not respond to HID++")]
    DeviceUnreachable { index: u8 },
    #[error("device does not expose HID++ feature {feature_hex:#06x}")]
    FeatureUnsupported { feature_hex: u16 },
    #[error("HID++ protocol error: {0}")]
    Hidpp(String),
}

/// Snapshot of one HID++ feature exposed by a device: protocol ID +
/// version. Returned by [`dump_features`] for diagnostics.
#[derive(Debug, Clone, Copy)]
pub struct FeatureEntry {
    pub id: u16,
    pub version: u8,
}

/// Enumerate every HID++ feature the device on `route` reports — used by
/// `openlogi diag features` to confirm which DPI / SmartShift / etc.
/// feature IDs a given peripheral actually exposes (e.g. some mice use
/// `0x2202 ExtendedAdjustableDpi` instead of `0x2201 AdjustableDpi`).
pub async fn dump_features(route: &DeviceRoute) -> Result<Vec<FeatureEntry>, WriteError> {
    use hidpp::feature::feature_set::FeatureSetFeature;
    let index = route.device_index();
    with_route(route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        // The root feature exposes the FeatureSet (0x0001) at a fixed
        // address; we look it up directly rather than going through
        // `enumerate_features` so the iteration is observable.
        let feature_set_info = device
            .root()
            .get_feature(FeatureSetFeature::ID)
            .await
            .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?
            .ok_or(WriteError::FeatureUnsupported {
                feature_hex: FeatureSetFeature::ID,
            })?;
        let feature_set = device.add_feature::<FeatureSetFeature>(feature_set_info.index);
        let count = feature_set
            .count()
            .await
            .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?;
        let mut entries = Vec::with_capacity(usize::from(count));
        for i in 0..=count {
            let info = feature_set
                .get_feature(i)
                .await
                .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?;
            entries.push(FeatureEntry {
                id: info.id,
                version: info.version,
            });
        }
        Ok(entries)
    })
    .await
}

/// Look up `F` on a device by HID++ feature ID, register it with
/// [`Device::add_feature`], and return the typed wrapper.
///
/// We bypass [`Device::enumerate_features`] because hidpp 0.2's central
/// registry has `versions: &[]` for the features OpenLogi cares about
/// (`0x2201 AdjustableDpi`, `0x2202 ExtendedAdjustableDpi`). Calling
/// `enumerate_features` ends up _not_ registering them, so a subsequent
/// `device.get_feature::<F>()` looking up our own TypeId returns `None`
/// even when the device announces the feature ID. The direct lookup via
/// `root().get_feature(id)` returns the assigned index unconditionally;
/// `add_feature` then attaches our wrapper to that index.
async fn open_feature<F: CreatableFeature + 'static>(
    device: &mut Device,
) -> Result<Arc<F>, WriteError> {
    let info = device
        .root()
        .get_feature(F::ID)
        .await
        .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?
        .ok_or(WriteError::FeatureUnsupported { feature_hex: F::ID })?;
    Ok(device.add_feature::<F>(info.index))
}

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

/// Whichever SmartShift feature a device exposes, normalised onto
/// [`SmartShiftMode`]. Devices ship one or the other: MX Master 3 / 3S use the
/// `0x2111` Enhanced variant, the MX Master 2S uses the original `0x2110`.
enum SmartShift {
    /// `0x2111 SmartShiftWheelEnhanced`.
    Enhanced(Arc<SmartShiftFeatureV0>),
    /// `0x2110 SmartShiftWheel`.
    Legacy(Arc<SmartShiftFeature>),
}

impl SmartShift {
    /// Open whichever SmartShift feature the device exposes. Tries `0x2111`
    /// first; on a missing-`0x2111` error (and only that), retries with
    /// `0x2110`. Any other error from the first attempt propagates unchanged.
    async fn open(device: &mut Device) -> Result<Self, WriteError> {
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
    async fn status(&self) -> Result<SmartShiftStatus, WriteError> {
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
    async fn set_mode(&self, mode: SmartShiftMode, sensitivity: u8) -> Result<(), WriteError> {
        match self {
            Self::Enhanced(feature) => feature
                .set_status(mode, sensitivity)
                .await
                .map_err(|e| WriteError::Hidpp(format!("{e:?}"))),
            Self::Legacy(feature) => {
                let wheel = match mode {
                    SmartShiftMode::Free => WheelMode::Freespin,
                    SmartShiftMode::Ratchet => WheelMode::Ratchet,
                };
                feature
                    .set_ratchet_control_mode(Some(wheel), None, None)
                    .await
                    .map_err(|e| WriteError::Hidpp(format!("{e:?}")))
            }
        }
    }
}

/// Read the device's current DPI on sensor 0 — companion to [`set_dpi`].
/// Used by `openlogi diag dpi` and any future Settings → Diagnostics
/// surface that wants to display the current value without writing.
pub async fn get_dpi(route: &DeviceRoute) -> Result<u16, WriteError> {
    let index = route.device_index();
    with_route(route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        let feature = open_feature::<AdjustableDpiFeatureV0>(&mut device).await?;
        feature
            .get_sensor_dpi(0)
            .await
            .map_err(|e| WriteError::Hidpp(format!("{e:?}")))
    })
    .await
}

/// Read the device's current SmartShift mode + sensitivity — companion to
/// [`toggle_smartshift`].
pub async fn get_smartshift_status(route: &DeviceRoute) -> Result<SmartShiftStatus, WriteError> {
    let index = route.device_index();
    with_route(route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        let smartshift = SmartShift::open(&mut device).await?;
        smartshift.status().await
    })
    .await
}

pub async fn set_dpi(route: &DeviceRoute, dpi: u16) -> Result<(), WriteError> {
    let index = route.device_index();
    with_route(route, move |channel| async move {
        set_dpi_on_channel(&channel, index, dpi).await
    })
    .await
}

/// The DPI write itself, on an already-open channel at HID++ `index`. Shared by
/// [`set_dpi`] (which opens a fresh channel) and [`set_dpi_on`] (which reuses
/// one).
async fn set_dpi_on_channel(
    channel: &Arc<HidppChannel>,
    index: u8,
    dpi: u16,
) -> Result<(), WriteError> {
    let mut device = Device::new(Arc::clone(channel), index)
        .await
        .map_err(|_| WriteError::DeviceUnreachable { index })?;
    let feature = open_feature::<AdjustableDpiFeatureV0>(&mut device).await?;
    feature
        .set_sensor_dpi(0, dpi)
        .await
        .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?;
    // Read back to confirm the firmware accepted the value. A mismatch is a
    // silent failure mode that's otherwise invisible — devices in low-power
    // states or with unsupported DPI ranges can ACK the write yet keep the old
    // value. We log a warning but still return Ok because the request reached
    // the device.
    if let Ok(actual) = feature.get_sensor_dpi(0).await {
        if actual == dpi {
            debug!(index, dpi, "wrote DPI (verified)");
        } else {
            tracing::warn!(
                index,
                requested = dpi,
                actual,
                "DPI write accepted but device reports a different value — \
                 likely out of the device's supported range"
            );
        }
    } else {
        debug!(index, dpi, "wrote DPI (read-back skipped)");
    }
    Ok(())
}

/// Toggle SmartShift mode (free ↔ ratchet) on `route`. Reads the current
/// mode first, then writes the opposite — keeps current sensitivity.
/// Returns the new mode written.
///
/// `FeatureUnsupported` when the device exposes neither HID++ `0x2111`
/// (MX Master 3 / 3S) nor the older `0x2110` (MX Master 2S) — i.e. it has no
/// SmartShift wheel.
pub async fn toggle_smartshift(route: &DeviceRoute) -> Result<SmartShiftMode, WriteError> {
    let index = route.device_index();
    with_route(route, move |channel| async move {
        toggle_smartshift_on_channel(&channel, index).await
    })
    .await
}

/// The SmartShift toggle itself, on an already-open channel at HID++ `index`.
/// Shared by [`toggle_smartshift`] and [`toggle_smartshift_on`].
async fn toggle_smartshift_on_channel(
    channel: &Arc<HidppChannel>,
    index: u8,
) -> Result<SmartShiftMode, WriteError> {
    let mut device = Device::new(Arc::clone(channel), index)
        .await
        .map_err(|_| WriteError::DeviceUnreachable { index })?;
    let smartshift = SmartShift::open(&mut device).await?;
    let SmartShiftStatus { mode, sensitivity } = smartshift.status().await?;
    let next = mode.flipped();
    smartshift.set_mode(next, sensitivity).await?;
    debug!(index, ?next, "wrote SmartShift mode");
    Ok(next)
}

/// An open HID++ channel to a device, shared so DPI / SmartShift writes can
/// reuse the capture session's connection instead of re-enumerating and
/// opening a fresh channel each time (which costs ~100ms+).
///
/// Cheap to clone (an `Arc` plus the [`DeviceRoute`] it points at). Built by
/// the capture session via [`SharedChannel::new`] and stashed in a slot the
/// GUI's write path consults.
#[derive(Clone)]
pub struct SharedChannel {
    channel: Arc<HidppChannel>,
    route: DeviceRoute,
}

impl SharedChannel {
    /// Wrap an open channel that reaches `route`.
    #[must_use]
    pub(crate) fn new(channel: Arc<HidppChannel>, route: DeviceRoute) -> Self {
        Self { channel, route }
    }

    /// Whether this channel reaches `route` — so the write path only reuses it
    /// for the device it actually points at.
    #[must_use]
    pub fn matches(&self, route: &DeviceRoute) -> bool {
        self.route == *route
    }
}

/// Write DPI on an already-open [`SharedChannel`] — the fast path that skips
/// enumeration and channel setup.
pub async fn set_dpi_on(shared: &SharedChannel, dpi: u16) -> Result<(), WriteError> {
    set_dpi_on_channel(&shared.channel, shared.route.device_index(), dpi).await
}

/// Toggle SmartShift on an already-open [`SharedChannel`].
pub async fn toggle_smartshift_on(shared: &SharedChannel) -> Result<SmartShiftMode, WriteError> {
    toggle_smartshift_on_channel(&shared.channel, shared.route.device_index()).await
}

/// Boilerplate-eater: open the channel that reaches `route`, then run `f` once
/// with it. The caller addresses features at [`DeviceRoute::device_index`].
async fn with_route<F, Fut, T>(route: &DeviceRoute, f: F) -> Result<T, WriteError>
where
    F: FnOnce(Arc<HidppChannel>) -> Fut,
    Fut: std::future::Future<Output = Result<T, WriteError>>,
{
    match open_route_channel(route).await? {
        Some(channel) => f(channel).await,
        None => Err(WriteError::DeviceNotFound),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smartshift::SmartShiftMode;

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
