# CLI Quickstart

A task-oriented guide to building and using the `openlogi` command-line binary.
This covers the CLI only — the `openlogi-gui` desktop app is out of scope here.

For the full command reference see [`USAGE.md`](USAGE.md); for the config file
format see [`CONFIGURATION.md`](CONFIGURATION.md).

## 1. Prerequisites

- macOS (the supported platform today; Linux/Windows are compile-only stubs).
- A Rust toolchain matching the workspace MSRV (effectively `1.87` — the
  vendored `openlogi-hidpp` fork raises it above the workspace `1.85`).
- A Logitech HID++ mouse reachable over a Logi Bolt receiver, a direct
  Bluetooth link, or a USB cable.

### macOS without full Xcode

Building the **CLI** only needs the Command Line Tools — full Xcode is required
only for the GUI. If you have just the Command Line Tools installed, point
`DEVELOPER_DIR` at them before any `cargo` command, otherwise linking fails with
`xcrun: error: unable to find utility "metal"`:

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
```

Add it to your shell profile to make it permanent. Note this also means
`cargo build/clippy/test --workspace` cannot complete on a CLT-only machine
(the GUI crate won't compile); scope to the CLI crates instead, e.g.
`cargo clippy -p openlogi-cli --all-targets`.

## 2. Build

```sh
# Build the CLI binary (does not pull in the GUI):
cargo build -p openlogi --release

# Or build and run in one step:
cargo run -p openlogi --release -- <command>
```

The binary lands at `target/release/openlogi`. Running it through
`cargo run` is convenient because the repo's `scripts/cargo-run-macos.sh`
wrapper sets `DEVELOPER_DIR` for the launched process automatically.

## 3. Granting device access (macOS)

Any command that talks to hardware (`list`, `diag`) requires two things:

1. **Quit Logi Options+** — it holds an exclusive HID++ connection. Check with
   `pgrep -fl -i logi`; if it is running, quit it and disable its login item.
2. **Grant Input Monitoring** to the terminal you run from
   (System Settings → Privacy & Security → Input Monitoring), then **relaunch
   the terminal** so the new permission takes effect.

Without these you get `Hid("Failed to open device")` or
`No Logitech HID++ devices found`.

## 4. Commands

### `list` — enumerate connected devices

```sh
cargo run -p openlogi --release -- list
```

### `diag features` — dump every HID++ feature a device reports

```sh
cargo run -p openlogi --release -- diag features
```

Use this to discover which features a mouse exposes (e.g. the MX Master 2S
reports `0x2110`, `0x2201`, `0x1b04`, …).

### `diag dpi` — DPI round-trip (read → write → read back → restore)

```sh
cargo run -p openlogi --release -- diag dpi                # default: current + 200
cargo run -p openlogi --release -- diag dpi --target 1600  # write a specific value
```

`--target` is clamped to the 200–6400 window the GUI slider uses.

### `diag smartshift` — SmartShift mode (free ↔ ratchet) and sensitivity

```sh
# Toggle round-trip (read → flip → read back → flip back):
cargo run -p openlogi --release -- diag smartshift

# Flip and LEAVE the wheel in the new mode (to feel the change by hand):
cargo run -p openlogi --release -- diag smartshift --leave-flipped

# Set the auto-disengage SENSITIVITY, keeping the current mode (no toggle).
# N = 1-255: lower = more sensitive (the wheel free-spins at a lower speed);
# typical 10-40; 255 = permanent ratchet.
cargo run -p openlogi --release -- diag smartshift --sensitivity 25
```

`--sensitivity` and `--leave-flipped` are mutually exclusive. `--sensitivity 0`
is rejected because the device treats `0` as "no change". Works on both the
`0x2111` Enhanced (MX Master 3 / 3S) and the original `0x2110` (MX Master 2S)
SmartShift features.

### `assets sync` — fetch device render assets

```sh
cargo run -p openlogi --release -- assets sync
```

Downloads each device's bundle-required files from `assets.openlogi.org`
(cached and sha256-verified).

## 5. Logging

Set the `OPENLOGI_LOG` env filter to raise verbosity (default `info`):

```sh
OPENLOGI_LOG=debug cargo run -p openlogi --release -- list
```

Accepts the standard `tracing` levels: `error`, `warn`, `info`, `debug`,
`trace`.
