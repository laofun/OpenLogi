//! HID++ `BatteryStatus` feature (`0x1000`) — legacy battery read for devices
//! such as MX Master 2S that do not expose `0x1004 UnifiedBattery`.
//!
//! Only `getStatus` (function 0) is implemented; notification decoding is not
//! needed for the inventory probe.
//!
//! Response format (function 0, `getBatteryLevelStatus`), cross-checked against
//! Solaar's `hidpp20.decipher_battery_status`:
//!
//! - `params[0]` — current battery charge as a percentage (`0..=100`).
//! - `params[1]` — next discharge-level threshold (informational; ignored).
//! - `params[2]` — battery status enum (same values as `0x1004`):
//!   `0` discharging, `1` recharging, `2` almost-full, `3` full,
//!   `4` slow-recharge, `5` invalid battery, `6` thermal error.

use std::sync::Arc;

use hidpp::{
    channel::HidppChannel,
    nibble::U4,
    protocol::v20::{self, Hidpp20Error},
};
use openlogi_core::device::{BatteryInfo, BatteryLevel, BatteryStatus};

/// `0x1000 BatteryStatus` feature ID.
pub const FEATURE_ID: u16 = 0x1000;

/// Raw `getStatus` response decoded from `0x1000`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryStatusInfo {
    /// Battery charge percentage (`0..=100`).
    pub percentage: u8,
    /// Next discharge-level threshold (firmware field; informational only).
    pub next_threshold: u8,
    /// Charging/discharging state.
    pub status: BatteryStatus,
}

/// Lightweight accessor for `0x1000 BatteryStatus`.
pub struct BatteryStatusFeature {
    chan: Arc<HidppChannel>,
    device_index: u8,
    feature_index: u8,
}

impl BatteryStatusFeature {
    /// Bind to an already-resolved feature index on `chan`.
    #[must_use]
    pub fn new(chan: Arc<HidppChannel>, device_index: u8, feature_index: u8) -> Self {
        Self {
            chan,
            device_index,
            feature_index,
        }
    }

    /// Call `getStatus` (function 0) and decode the raw response.
    pub async fn get_status(&self) -> Result<BatteryStatusInfo, Hidpp20Error> {
        let response = self
            .chan
            .send_v20(v20::Message::Short(
                v20::MessageHeader {
                    device_index: self.device_index,
                    feature_index: self.feature_index,
                    function_id: U4::from_lo(0),
                    software_id: self.chan.get_sw_id(),
                },
                [0, 0, 0],
            ))
            .await?;
        let p = response.extend_payload();
        Ok(BatteryStatusInfo {
            percentage: p[0],
            next_threshold: p[1],
            status: decode_status(p[2]),
        })
    }

    /// Convenience: return a [`BatteryInfo`] suitable for the inventory layer.
    pub async fn get_battery_info(&self) -> Result<BatteryInfo, Hidpp20Error> {
        let raw = self.get_status().await?;
        Ok(BatteryInfo {
            percentage: raw.percentage,
            // `0%` is the "percentage unknown" sentinel (e.g. while charging the
            // device stops reporting a discharge level), so the coarse level
            // bucket is meaningless — surface `Unknown` rather than `Critical`.
            level: if raw.percentage == 0 {
                BatteryLevel::Unknown
            } else {
                percentage_to_level(raw.percentage)
            },
            status: raw.status,
        })
    }
}

/// Whether a decoded `0x1000` reading carries usable information.
///
/// `0%` is the feature's "percentage unknown" sentinel: the device only
/// reports a *discharge* percentage, so while it is on the charging cable it
/// returns `0%` with a charging status byte. Such a reading is still
/// meaningful (we can show "charging"). Only a `0%` reading whose status is
/// `Discharging`/`Unknown` carries nothing — that's the genuinely-unknown or
/// degraded just-woken (all-zero) read, which the caller drops.
#[must_use]
pub fn is_informative(info: &BatteryInfo) -> bool {
    info.percentage != 0
        || !matches!(
            info.status,
            BatteryStatus::Discharging | BatteryStatus::Unknown
        )
}

/// Decode the `0x1000` status enum byte (`params[2]`) into the core
/// [`BatteryStatus`]. Values match the `0x1004 UnifiedBattery` enum.
fn decode_status(byte: u8) -> BatteryStatus {
    match byte {
        0 => BatteryStatus::Discharging,
        1 => BatteryStatus::Charging,
        // 2 = almost-full, 3 = full → both surface as "full" in core's coarser enum.
        2 | 3 => BatteryStatus::Full,
        4 => BatteryStatus::ChargingSlow,
        // 5 = invalid battery, 6 = thermal error.
        5 | 6 => BatteryStatus::Error,
        _ => BatteryStatus::Unknown,
    }
}

/// Bucket a percentage into the coarse [`BatteryLevel`] the UI shows.
fn percentage_to_level(pct: u8) -> BatteryLevel {
    if pct <= 10 {
        BatteryLevel::Critical
    } else if pct <= 20 {
        BatteryLevel::Low
    } else if pct < 90 {
        BatteryLevel::Good
    } else {
        BatteryLevel::Full
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_status, is_informative, percentage_to_level};
    use openlogi_core::device::{BatteryInfo, BatteryLevel, BatteryStatus};

    fn info(percentage: u8, status: BatteryStatus) -> BatteryInfo {
        BatteryInfo {
            percentage,
            level: percentage_to_level(percentage),
            status,
        }
    }

    #[test]
    fn nonzero_percentage_is_informative() {
        assert!(is_informative(&info(90, BatteryStatus::Discharging)));
        assert!(is_informative(&info(5, BatteryStatus::Discharging)));
    }

    #[test]
    fn zero_percent_while_charging_is_informative() {
        // Device on the charging cable: percentage unknown (0) but status
        // carries the real charging state — keep the reading.
        assert!(is_informative(&info(0, BatteryStatus::Charging)));
        assert!(is_informative(&info(0, BatteryStatus::ChargingSlow)));
        assert!(is_informative(&info(0, BatteryStatus::Full)));
    }

    #[test]
    fn zero_percent_discharging_or_unknown_is_not_informative() {
        // Genuine "unknown" / degraded just-woken all-zero read.
        assert!(!is_informative(&info(0, BatteryStatus::Discharging)));
        assert!(!is_informative(&info(0, BatteryStatus::Unknown)));
    }

    #[test]
    fn status_enum_discharging() {
        assert_eq!(decode_status(0), BatteryStatus::Discharging);
    }

    #[test]
    fn status_enum_charging() {
        assert_eq!(decode_status(1), BatteryStatus::Charging);
    }

    #[test]
    fn status_enum_almost_full_and_full_map_to_full() {
        assert_eq!(decode_status(2), BatteryStatus::Full);
        assert_eq!(decode_status(3), BatteryStatus::Full);
    }

    #[test]
    fn status_enum_slow_recharge() {
        assert_eq!(decode_status(4), BatteryStatus::ChargingSlow);
    }

    #[test]
    fn status_enum_errors() {
        assert_eq!(decode_status(5), BatteryStatus::Error);
        assert_eq!(decode_status(6), BatteryStatus::Error);
    }

    #[test]
    fn status_enum_unknown() {
        assert_eq!(decode_status(7), BatteryStatus::Unknown);
        assert_eq!(decode_status(0xff), BatteryStatus::Unknown);
    }

    #[test]
    fn percentage_to_level_boundaries() {
        assert_eq!(percentage_to_level(0), BatteryLevel::Critical);
        assert_eq!(percentage_to_level(10), BatteryLevel::Critical);
        assert_eq!(percentage_to_level(15), BatteryLevel::Low);
        assert_eq!(percentage_to_level(20), BatteryLevel::Low);
        assert_eq!(percentage_to_level(50), BatteryLevel::Good);
        assert_eq!(percentage_to_level(89), BatteryLevel::Good);
        assert_eq!(percentage_to_level(90), BatteryLevel::Full);
        assert_eq!(percentage_to_level(100), BatteryLevel::Full);
    }
}
