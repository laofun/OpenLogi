//! `openlogi diag dpi` — DPI write round-trip.

use anyhow::{Context, Result};
use clap::Args;

use crate::cmd::diag::first_online_device;

async fn print_dpi_reference(route: &openlogi_hid::DeviceRoute) {
    if let Ok(summary) = openlogi_hid::device_identity_summary(route).await {
        if let Some(r) = summary.dpi_reference {
            println!(
                "  reference range: {}..={} step {} ({})",
                r.min, r.max, r.step, r.source
            );
        }
    }
}

#[derive(Debug, Args)]
pub struct DpiArgs {
    /// DPI to set during the test. Default = current + 200, clamped to the
    /// 200–6400 window the GUI slider uses.
    #[arg(long)]
    pub target: Option<u16>,
}

pub async fn run(args: DpiArgs) -> Result<()> {
    let (route, name, _) = first_online_device().await?;
    println!("device: {name} ({route})");

    let before = openlogi_hid::get_dpi(&route)
        .await
        .context("read current DPI")?;
    println!("  current DPI: {before}");
    print_dpi_reference(&route).await;

    let target = args.target.unwrap_or_else(|| {
        if before < 3200 {
            before.saturating_add(200).clamp(200, 6400)
        } else {
            before.saturating_sub(200).clamp(200, 6400)
        }
    });
    if target == before {
        println!(
            "  target {target} equals current — pick a different --target to exercise the write"
        );
        return Ok(());
    }

    println!("  writing DPI: {target}");
    openlogi_hid::set_dpi(&route, target)
        .await
        .context("write DPI")?;

    let after = openlogi_hid::get_dpi(&route)
        .await
        .context("read DPI after write")?;
    println!("  read-back DPI: {after}");

    if after != target {
        anyhow::bail!(
            "DPI write failed: requested {target}, device reports {after} \
             (likely out of the device's supported range)"
        );
    }

    println!("  restoring DPI: {before}");
    openlogi_hid::set_dpi(&route, before)
        .await
        .context("restore DPI")?;

    println!("✓ DPI round-trip OK");
    Ok(())
}
