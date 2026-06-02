//! `openlogi diag controls` — dump reprogrammable controls with OpenLogi button labels.

use anyhow::{Context, Result};
use clap::Args;

use crate::cmd::diag::first_online_device;

#[derive(Debug, Args)]
pub struct ControlsArgs {}

/// Human-readable label for a well-known control ID.
pub fn cid_label(cid: u16) -> &'static str {
    match cid {
        0x0050 => "left-click",
        0x0051 => "right-click",
        0x0052 => "middle-click",
        0x0053 => "back",
        0x0056 => "forward",
        // GestureButton (OpenLogi ButtonId::GestureButton)
        0x00c3 => "gesture-button (GestureButton)",
        // SmartShift / wheel-mode-shift family (OpenLogi ButtonId::DpiToggle)
        0x00c4 => "wheel-mode-shift / SmartShift (DpiToggle)",
        0x00ed => "DPI mode shift (DpiToggle)",
        0x00fd => "DPI mode shift alt (DpiToggle)",
        _ => "unknown",
    }
}

pub async fn run(_args: ControlsArgs) -> Result<()> {
    let (route, name, _) = first_online_device().await?;
    println!("device: {name} ({route})");

    let controls = openlogi_hid::dump_reprog_controls(&route)
        .await
        .context("dump reprog controls")?;

    if controls.is_empty() {
        println!("  0x1b04 ReprogControlsV4: not found or no controls");
        return Ok(());
    }

    println!("  0x1b04 ReprogControlsV4: {} control(s)", controls.len());
    println!("  {:>3}  {:>6}  label", "idx", "cid");
    println!("  {}  {}  {}", "-".repeat(3), "-".repeat(6), "-".repeat(40));
    for entry in &controls {
        println!(
            "  {:>3}  {:#06x}  {}",
            entry.index,
            entry.info.cid,
            cid_label(entry.info.cid)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::cid_label;

    #[test]
    fn gesture_button_cid_recognized() {
        let label = cid_label(0x00c3);
        assert!(label.contains("GestureButton"), "label={label}");
    }

    #[test]
    fn wheel_mode_shift_cid_recognized() {
        let label = cid_label(0x00c4);
        assert!(label.contains("DpiToggle"), "label={label}");
        assert!(
            label.contains("SmartShift") || label.contains("wheel-mode"),
            "label={label}"
        );
    }

    #[test]
    fn dpi_mode_shift_variants_recognized() {
        for cid in [0x00ed_u16, 0x00fd] {
            let label = cid_label(cid);
            assert!(label.contains("DpiToggle"), "cid={cid:#06x} label={label}");
        }
    }

    #[test]
    fn standard_buttons_recognized() {
        assert_eq!(cid_label(0x0052), "middle-click");
        assert_eq!(cid_label(0x0053), "back");
        assert_eq!(cid_label(0x0056), "forward");
    }

    #[test]
    fn unknown_cid_does_not_panic() {
        let label = cid_label(0xffff);
        assert_eq!(label, "unknown");
    }
}
