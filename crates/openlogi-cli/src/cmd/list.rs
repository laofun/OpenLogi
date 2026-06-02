use anyhow::{Context, Result};
use clap::Args;
use openlogi_core::device::{BatteryInfo, DeviceInventory, DeviceModelInfo, PairedDevice};
use openlogi_hid::DIRECT_DEVICE_INDEX;

#[derive(Debug, Args)]
pub struct ListArgs {}

pub async fn run(_args: ListArgs) -> Result<()> {
    let inventories = openlogi_hid::enumerate()
        .await
        .context("failed to enumerate HID++ devices")?;

    if inventories.is_empty() {
        println!("No Logitech HID++ devices found.");
        println!();
        println!("Notes:");
        println!("  - On macOS, quit Logi Options+ first — both apps fight over HID++ access.");
        println!(
            "  - A Bluetooth-direct mouse (e.g. Lift, Signature) needs Input Monitoring \
             permission: System Settings → Privacy & Security → Input Monitoring."
        );
        println!(
            "  - hidpp 0.2 only recognises Logi Bolt receivers (PID 0xC548); other \
             receivers (Unifying) aren't surfaced yet."
        );
        std::process::exit(2);
    }

    for (i, inv) in inventories.iter().enumerate() {
        if i != 0 {
            println!();
        }
        print_inventory(inv);
    }

    Ok(())
}

fn print_inventory(inv: &DeviceInventory) {
    let uid = inv.receiver.unique_id.as_deref().unwrap_or("—");
    println!(
        "{} ({}, vid={:04x} pid={:04x})",
        inv.receiver.name, uid, inv.receiver.vendor_id, inv.receiver.product_id
    );

    if inv.paired.is_empty() {
        println!("  └─ no paired devices");
        return;
    }

    let last = inv.paired.len() - 1;
    for (i, d) in inv.paired.iter().enumerate() {
        let prefix = if i == last { "  └─" } else { "  ├─" };
        println!("{prefix} {}", format_device(d));
        if let Some(model) = d.model_info.as_ref() {
            let cont = if i == last { "     " } else { "  │  " };
            println!("{cont}{}", format_model(model));
        }
    }
}

fn format_device(d: &PairedDevice) -> String {
    let dot = if d.online { "●" } else { "○" };
    let codename = d.codename.as_deref().unwrap_or("Unknown device");
    let route_hint = d.wpid.map_or_else(
        || {
            if d.slot == DIRECT_DEVICE_INDEX {
                "direct".to_string()
            } else {
                "wpid=?".to_string()
            }
        },
        |w| format!("wpid={w:04x}"),
    );
    let battery = d
        .battery
        .as_ref()
        .map_or_else(|| "battery=—".to_string(), format_battery);
    let kind = format!("{:?}", d.kind).to_lowercase();
    format!(
        "slot {} {dot} {codename} ({kind}, {route_hint}, {battery})",
        d.slot
    )
}

fn format_battery(b: &BatteryInfo) -> String {
    let status = format!("{:?}", b.status).to_lowercase();
    // `0%` is the legacy `0x1000` "percentage unknown" sentinel (e.g. while
    // charging): show the status only, not a misleading `0% critical`.
    if b.percentage == 0 {
        return format!("battery={} ({status})", b.percentage_display());
    }
    let level = format!("{:?}", b.level).to_lowercase();
    format!("battery={} {level} ({status})", b.percentage_display())
}

fn format_model(m: &DeviceModelInfo) -> String {
    let transports = {
        let mut t = Vec::new();
        if m.transports.usb {
            t.push("usb");
        }
        if m.transports.equad {
            t.push("equad");
        }
        if m.transports.btle {
            t.push("btle");
        }
        if m.transports.bluetooth {
            t.push("bt");
        }
        if t.is_empty() {
            "—".to_string()
        } else {
            t.join("+")
        }
    };
    let ids = m
        .model_ids
        .iter()
        .map(|id| format!("{id:04x}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut unit = String::with_capacity(8);
    for b in m.unit_id {
        use std::fmt::Write as _;
        let _ = write!(unit, "{b:02x}");
    }
    let serial = m.serial_number.as_deref().unwrap_or("—");
    let config_key = m.config_key();
    format!(
        "     model_ids=[{ids}] ext={:02x} serial={serial} unit_id={unit} transports={transports} config_key={config_key}",
        m.extended_model_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlogi_core::device::{BatteryLevel, BatteryStatus, DeviceKind, DeviceTransports};

    #[test]
    fn format_battery_shows_percentage_and_level() {
        let b = BatteryInfo {
            percentage: 90,
            level: BatteryLevel::Full,
            status: BatteryStatus::Discharging,
        };
        assert_eq!(format_battery(&b), "battery=90% full (discharging)");
    }

    #[test]
    fn format_battery_zero_percent_charging_hides_bogus_percent() {
        // 0% is the "unknown" sentinel; while charging show status only.
        let b = BatteryInfo {
            percentage: 0,
            level: BatteryLevel::Unknown,
            status: BatteryStatus::Charging,
        };
        assert_eq!(format_battery(&b), "battery=?% (charging)");
    }

    #[test]
    fn format_device_marks_direct_device_without_wpid() {
        let device = PairedDevice {
            slot: DIRECT_DEVICE_INDEX,
            codename: Some("MX Master 2S".to_string()),
            wpid: None,
            kind: DeviceKind::Mouse,
            online: true,
            battery: None,
            model_info: None,
        };

        let formatted = format_device(&device);
        assert!(formatted.contains("(mouse, direct, battery=—)"));
        assert!(!formatted.contains("wpid=?"));
    }

    #[test]
    fn format_model_includes_config_key() {
        let model = DeviceModelInfo {
            entity_count: 0,
            serial_number: None,
            unit_id: [0; 4],
            transports: DeviceTransports::default(),
            model_ids: [0xb019, 0, 0],
            extended_model_id: 0,
        };

        let formatted = format_model(&model);
        assert!(formatted.contains("model_ids=[b019,0000,0000]"));
        assert!(formatted.contains("config_key=0b019"));
    }
}
