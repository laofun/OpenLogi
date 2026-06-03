//! Battery-feature diagnostics — the fork-specific read path that probes both
//! `0x1004 UnifiedBattery` (modern devices) and `0x1000 BatteryStatus` (legacy
//! devices such as the MX Master 2S, which upstream does not support).
//!
//! Kept separate from [`crate::write`] so the fork's legacy-battery additions
//! don't widen the merge surface on upstream's shared write paths. The `0x1000`
//! decode itself lives in [`crate::battery_status`]; this module is the
//! route-level summary that the CLI `diag battery` command consumes.

use std::sync::Arc;

use hidpp::feature::CreatableFeature;
use hidpp::feature::unified_battery::{
    BatteryLevel as HidppBatteryLevel, BatteryStatus as HidppBatteryStatus, UnifiedBatteryFeature,
};
use hidpp::{channel::HidppChannel, device::Device};
use openlogi_core::device::{BatteryInfo, BatteryLevel, BatteryStatus};

use crate::route::DeviceRoute;
use crate::write::{WriteError, feature_present, open_feature, with_route};

#[derive(Debug)]
pub struct BatteryFeatureSummary {
    pub battery_status_present: bool,
    pub battery_voltage_present: bool,
    pub unified_battery_present: bool,
    /// Decoded `0x1004 UnifiedBattery` reading, if that feature is present.
    pub unified_battery: Result<Option<BatteryInfo>, WriteError>,
    /// Decoded `0x1000 BatteryStatus` reading, if that feature is present.
    /// `Ok(None)` when the feature reports a `0%` "unknown" sentinel.
    pub legacy_battery: Result<Option<BatteryInfo>, WriteError>,
}

fn map_battery_level(level: HidppBatteryLevel) -> BatteryLevel {
    match level {
        HidppBatteryLevel::Critical => BatteryLevel::Critical,
        HidppBatteryLevel::Low => BatteryLevel::Low,
        HidppBatteryLevel::Good => BatteryLevel::Good,
        HidppBatteryLevel::Full => BatteryLevel::Full,
        _ => BatteryLevel::Unknown,
    }
}

fn map_battery_status(status: HidppBatteryStatus) -> BatteryStatus {
    match status {
        HidppBatteryStatus::Discharging => BatteryStatus::Discharging,
        HidppBatteryStatus::Charging => BatteryStatus::Charging,
        HidppBatteryStatus::ChargingSlow => BatteryStatus::ChargingSlow,
        HidppBatteryStatus::Full => BatteryStatus::Full,
        HidppBatteryStatus::Error => BatteryStatus::Error,
        _ => BatteryStatus::Unknown,
    }
}

pub async fn battery_feature_summary(
    route: &DeviceRoute,
) -> Result<BatteryFeatureSummary, WriteError> {
    let index = route.device_index();
    with_route(route, move |channel| async move {
        let mut device = Device::new(Arc::clone(&channel), index)
            .await
            .map_err(|_| WriteError::DeviceUnreachable { index })?;
        let battery_status_present = feature_present(&device, 0x1000).await?;
        let battery_voltage_present = feature_present(&device, 0x1001).await?;
        let unified_battery_present = feature_present(&device, UnifiedBatteryFeature::ID).await?;
        let unified_battery = if unified_battery_present {
            match open_feature::<UnifiedBatteryFeature>(&mut device).await {
                Ok(feature) => feature
                    .get_battery_info()
                    .await
                    .map(|info| {
                        Some(BatteryInfo {
                            percentage: info.charging_percentage,
                            level: map_battery_level(info.level),
                            status: map_battery_status(info.status),
                        })
                    })
                    .map_err(|e| WriteError::Hidpp(format!("{e:?}"))),
                Err(err) => Err(err),
            }
        } else {
            Ok(None)
        };
        let legacy_battery = if battery_status_present {
            read_legacy_battery(&device, &channel, index).await
        } else {
            Ok(None)
        };
        Ok(BatteryFeatureSummary {
            battery_status_present,
            battery_voltage_present,
            unified_battery_present,
            unified_battery,
            legacy_battery,
        })
    })
    .await
}

/// Read and decode `0x1000 BatteryStatus`. Returns `Ok(None)` when the device
/// reports the `0%` "unknown" sentinel (firmware not ready / degraded read).
async fn read_legacy_battery(
    device: &Device,
    channel: &Arc<HidppChannel>,
    index: u8,
) -> Result<Option<BatteryInfo>, WriteError> {
    use crate::battery_status::{
        BatteryStatusFeature, FEATURE_ID as BATTERY_STATUS_FEATURE_ID, is_informative,
    };
    let info = device
        .root()
        .get_feature(BATTERY_STATUS_FEATURE_ID)
        .await
        .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?
        .ok_or(WriteError::FeatureUnsupported {
            feature_hex: BATTERY_STATUS_FEATURE_ID,
        })?;
    let feature = BatteryStatusFeature::new(Arc::clone(channel), index, info.index);
    let battery = feature
        .get_battery_info()
        .await
        .map_err(|e| WriteError::Hidpp(format!("{e:?}")))?;
    // Keep a 0% charging reading (percentage is the unknown sentinel, status is
    // real); drop only a 0% discharging/unknown read.
    Ok(is_informative(&battery).then_some(battery))
}
