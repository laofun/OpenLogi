//! HID++ `BatteryStatus` feature (`0x1000`) — legacy battery read for devices
//! such as MX Master 2S that do not expose `0x1004 UnifiedBattery`.
//!
//! Only `getStatus` (function 0) is implemented; notification decoding is not
//! needed for the inventory probe. Payload format cross-checked against the
//! Linux kernel `hid-logitech-hidpp.c` (`hidpp20_battery_set_battery_state`).

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
    /// Battery charge percentage (0–100). May be a coarse approximation.
    pub percentage: u8,
    /// Next battery level estimate (firmware field; often same as current).
    pub next_percentage: u8,
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
        // p[0] = discharge level (0–7 levels) documented as percentage proxy
        // p[1] = next level
        // p[2] = status byte (charging state)
        let percentage = decode_discharge_level(p[0]);
        let next_percentage = decode_discharge_level(p[1]);
        let status = decode_status_byte(p[2]);
        Ok(BatteryStatusInfo {
            percentage,
            next_percentage,
            status,
        })
    }

    /// Convenience: return a [`BatteryInfo`] suitable for the inventory layer.
    pub async fn get_battery_info(&self) -> Result<BatteryInfo, Hidpp20Error> {
        let raw = self.get_status().await?;
        let level = percentage_to_level(raw.percentage);
        Ok(BatteryInfo {
            percentage: raw.percentage,
            level,
            status: raw.status,
        })
    }
}

/// The `0x1000` `dischargeLevel` field encodes approximate % in 7 steps.
/// Mapping cross-checked against Linux kernel `hid-logitech-hidpp.c`
/// (`hidpp20_battery_set_battery_state`, cases 0–7).
fn decode_discharge_level(level: u8) -> u8 {
    match level {
        1 => 10,
        2 => 30,
        3 => 50,
        4 => 70,
        5 => 90,
        6 | 7 => 100,
        _ => 0,
    }
}

/// `statusBit` byte from `0x1000 getStatus` response.
/// bit 0 = discharging, bit 1 = recharging, bit 2 = charge complete, bit 3 = error.
/// Cross-checked against Linux kernel `hidpp20_battery_set_battery_state`.
fn decode_status_byte(byte: u8) -> BatteryStatus {
    if byte & 0x08 != 0 {
        return BatteryStatus::Error;
    }
    if byte & 0x04 != 0 {
        return BatteryStatus::Full;
    }
    if byte & 0x02 != 0 {
        return BatteryStatus::Charging;
    }
    BatteryStatus::Discharging
}

fn percentage_to_level(pct: u8) -> BatteryLevel {
    if pct == 0 {
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
    use super::{decode_discharge_level, decode_status_byte, percentage_to_level};
    use openlogi_core::device::{BatteryLevel, BatteryStatus};

    #[test]
    fn discharge_level_maps_to_percentage() {
        assert_eq!(decode_discharge_level(0), 0);
        assert_eq!(decode_discharge_level(1), 10);
        assert_eq!(decode_discharge_level(4), 70);
        assert_eq!(decode_discharge_level(6), 100);
        assert_eq!(decode_discharge_level(7), 100);
    }

    #[test]
    fn status_byte_discharging() {
        assert_eq!(decode_status_byte(0x00), BatteryStatus::Discharging);
        assert_eq!(decode_status_byte(0x01), BatteryStatus::Discharging);
    }

    #[test]
    fn status_byte_charging() {
        assert_eq!(decode_status_byte(0x02), BatteryStatus::Charging);
    }

    #[test]
    fn status_byte_full() {
        assert_eq!(decode_status_byte(0x04), BatteryStatus::Full);
    }

    #[test]
    fn status_byte_error() {
        assert_eq!(decode_status_byte(0x08), BatteryStatus::Error);
    }

    #[test]
    fn percentage_to_level_boundaries() {
        assert_eq!(percentage_to_level(0), BatteryLevel::Critical);
        assert_eq!(percentage_to_level(10), BatteryLevel::Low);
        assert_eq!(percentage_to_level(50), BatteryLevel::Good);
        assert_eq!(percentage_to_level(100), BatteryLevel::Full);
    }
}
