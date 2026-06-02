//! `openlogi diag battery` — probe which HID++ battery features the device exposes.

use anyhow::{Context, Result};
use clap::Args;

use crate::cmd::diag::first_online_device;

#[derive(Debug, Args)]
pub struct BatteryArgs {}

pub async fn run(_args: BatteryArgs) -> Result<()> {
    let (route, name, _) = first_online_device().await?;
    println!("device: {name} ({route})");

    let summary = openlogi_hid::battery_feature_summary(&route)
        .await
        .context("battery feature summary")?;

    println!(
        "  0x1000 BatteryStatus:  {}",
        if summary.battery_status_present {
            "present"
        } else {
            "not found"
        }
    );
    if summary.battery_status_present {
        match summary.legacy_battery {
            Ok(Some(b)) => println!(
                "    battery: {}% {:?} ({:?})",
                b.percentage, b.level, b.status
            ),
            Ok(None) => println!("    battery: 0% reported — treated as unknown"),
            Err(ref e) => println!("    battery: read failed — {e}"),
        }
    }
    println!(
        "  0x1001 BatteryVoltage: {}",
        if summary.battery_voltage_present {
            "present (decoder not implemented)"
        } else {
            "not found"
        }
    );
    println!(
        "  0x1004 UnifiedBattery: {}",
        if summary.unified_battery_present {
            "present"
        } else {
            "not found"
        }
    );

    if summary.unified_battery_present {
        match summary.unified_battery {
            Ok(Some(b)) => println!(
                "    battery: {}% {:?} ({:?})",
                b.percentage, b.level, b.status
            ),
            Ok(None) => println!("    battery: unavailable via 0x1004"),
            Err(e) => println!("    battery: read failed — {e}"),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use openlogi_core::device::{BatteryInfo, BatteryLevel, BatteryStatus};
    use openlogi_hid::{BatteryFeatureSummary, WriteError};

    fn format_battery_line(summary: &BatteryFeatureSummary) -> String {
        match &summary.unified_battery {
            Ok(Some(b)) => format!("{}% {:?} ({:?})", b.percentage, b.level, b.status),
            Ok(None) => "unavailable via 0x1004".to_string(),
            Err(e) => format!("read failed — {e}"),
        }
    }

    fn format_legacy_line(summary: &BatteryFeatureSummary) -> String {
        match &summary.legacy_battery {
            Ok(Some(b)) => format!("{}% {:?} ({:?})", b.percentage, b.level, b.status),
            Ok(None) => "0% reported — treated as unknown".to_string(),
            Err(e) => format!("read failed — {e}"),
        }
    }

    #[test]
    fn unified_battery_decoded() {
        let summary = BatteryFeatureSummary {
            battery_status_present: false,
            battery_voltage_present: false,
            unified_battery_present: true,
            unified_battery: Ok(Some(BatteryInfo {
                percentage: 80,
                level: BatteryLevel::Good,
                status: BatteryStatus::Discharging,
            })),
            legacy_battery: Ok(None),
        };
        let line = format_battery_line(&summary);
        assert!(line.contains("80%"), "line={line}");
        assert!(line.contains("Good"), "line={line}");
        assert!(line.contains("Discharging"), "line={line}");
    }

    #[test]
    fn legacy_battery_decoded() {
        let summary = BatteryFeatureSummary {
            battery_status_present: true,
            battery_voltage_present: false,
            unified_battery_present: false,
            unified_battery: Ok(None),
            legacy_battery: Ok(Some(BatteryInfo {
                percentage: 50,
                level: BatteryLevel::Good,
                status: BatteryStatus::Discharging,
            })),
        };
        let line = format_legacy_line(&summary);
        assert!(line.contains("50%"), "line={line}");
        assert!(line.contains("Good"), "line={line}");
    }

    #[test]
    fn legacy_battery_zero_is_unknown() {
        let summary = BatteryFeatureSummary {
            battery_status_present: true,
            battery_voltage_present: false,
            unified_battery_present: false,
            unified_battery: Ok(None),
            legacy_battery: Ok(None),
        };
        let line = format_legacy_line(&summary);
        assert_eq!(line, "0% reported — treated as unknown");
    }

    #[test]
    fn only_legacy_features_present() {
        let summary = BatteryFeatureSummary {
            battery_status_present: true,
            battery_voltage_present: true,
            unified_battery_present: false,
            unified_battery: Ok(None),
            legacy_battery: Ok(None),
        };
        assert!(summary.battery_status_present);
        assert!(summary.battery_voltage_present);
        assert!(!summary.unified_battery_present);
        let line = format_battery_line(&summary);
        assert_eq!(line, "unavailable via 0x1004");
    }

    #[test]
    fn no_battery_features() {
        let summary = BatteryFeatureSummary {
            battery_status_present: false,
            battery_voltage_present: false,
            unified_battery_present: false,
            unified_battery: Ok(None),
            legacy_battery: Ok(None),
        };
        assert!(!summary.battery_status_present);
        assert!(!summary.battery_voltage_present);
        assert!(!summary.unified_battery_present);
        let line = format_battery_line(&summary);
        assert_eq!(line, "unavailable via 0x1004");
    }

    #[test]
    fn unified_battery_read_error() {
        let summary = BatteryFeatureSummary {
            battery_status_present: false,
            battery_voltage_present: false,
            unified_battery_present: true,
            unified_battery: Err(WriteError::Hidpp("timeout".to_string())),
            legacy_battery: Ok(None),
        };
        let line = format_battery_line(&summary);
        assert!(line.starts_with("read failed"), "line={line}");
    }
}
