//! `openlogi diag smartshift` — SmartShift toggle round-trip.

use anyhow::{Context, Result};
use clap::Args;

use crate::cmd::diag::first_online_device;
use openlogi_core::config::Config;

#[derive(Debug, Args)]
pub struct SmartshiftArgs {
    /// Leave the wheel in the toggled mode (skip the second toggle that
    /// restores the original). Useful for visually verifying the flip.
    #[arg(long, conflicts_with = "sensitivity")]
    pub leave_flipped: bool,

    /// Set the auto-disengage sensitivity instead of toggling, keeping the
    /// current Free/Ratchet mode. N is 1-255 (the wheel's speed threshold to
    /// free-spin): lower = more sensitive; typical 10-40; 255 = permanent
    /// ratchet.
    #[arg(long, value_name = "N")]
    pub sensitivity: Option<u8>,

    /// Persist the sensitivity to config.toml under this device's key so the
    /// GUI re-applies it on every connect. Only valid together with
    /// --sensitivity.
    #[arg(long, requires = "sensitivity")]
    pub save: bool,
}

pub async fn run(args: SmartshiftArgs) -> Result<()> {
    let (route, name, config_key) = first_online_device().await?;
    println!("device: {name} ({route})");

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
                before.mode,
                after.mode
            );
        }

        println!(
            "✓ SmartShift sensitivity set to {n} (mode {:?} preserved)",
            after.mode
        );

        if args.save {
            if config_key.is_empty() {
                anyhow::bail!(
                    "cannot --save: device did not report a model id (HID++ 0x0003); \
                     the GUI keys config by model id"
                );
            }
            let mut config = Config::load_or_default().context("load config for --save")?;
            config.set_smartshift_sensitivity(&config_key, Some(n));
            config.save_atomic().context("save config")?;
            println!("✓ saved sensitivity {n} to config for device {config_key}");
        }
        return Ok(());
    }

    let before = openlogi_hid::get_smartshift_status(&route)
        .await
        .context("read SmartShift status")?;
    println!(
        "  current: mode={:?} sensitivity={}",
        before.mode, before.sensitivity
    );

    let new_mode = openlogi_hid::toggle_smartshift(&route)
        .await
        .context("toggle SmartShift")?;
    println!("  toggled to: {new_mode:?}");

    let after = openlogi_hid::get_smartshift_status(&route)
        .await
        .context("read SmartShift after toggle")?;
    println!(
        "  read-back: mode={:?} sensitivity={}",
        after.mode, after.sensitivity
    );

    if after.mode == before.mode {
        anyhow::bail!(
            "SmartShift toggle had no effect: still {:?} after write",
            before.mode
        );
    }

    if args.leave_flipped {
        println!("✓ SmartShift toggle OK (wheel left in {new_mode:?})");
        return Ok(());
    }

    println!("  restoring mode: {:?}", before.mode);
    openlogi_hid::toggle_smartshift(&route)
        .await
        .context("restore SmartShift")?;

    println!("✓ SmartShift round-trip OK");
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "expect is idiomatic in tests")]
mod tests {
    use super::*;
    use clap::Parser;

    /// Wrap the `Args` group in a throwaway `Parser` so we can exercise
    /// clap's `requires` / value parsing without a real device.
    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: SmartshiftArgs,
    }

    #[test]
    fn save_requires_sensitivity() {
        // `--save` alone must be rejected at parse time by `requires`.
        let parsed = TestCli::try_parse_from(["t", "--save"]);
        assert!(parsed.is_err(), "save without sensitivity should fail");
    }

    #[test]
    fn save_with_sensitivity_parses() {
        let cli =
            TestCli::try_parse_from(["t", "--sensitivity", "20", "--save"]).expect("should parse");
        assert_eq!(cli.args.sensitivity, Some(20));
        assert!(cli.args.save);
    }

    #[test]
    fn sensitivity_without_save_parses() {
        let cli = TestCli::try_parse_from(["t", "--sensitivity", "20"]).expect("should parse");
        assert_eq!(cli.args.sensitivity, Some(20));
        assert!(!cli.args.save);
    }
}
