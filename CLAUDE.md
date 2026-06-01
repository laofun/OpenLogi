# CLAUDE.md

Orientation for AI agents working in this repository. Read this first, then
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the detailed design.

## 1. What this is

OpenLogi is a six-crate Rust workspace — a native, local-first alternative to
Logitech Options+ that controls Logitech mice over HID++ (via a Logi Bolt
receiver, a direct Bluetooth link, or a USB cable). macOS is the supported
platform today; Linux and Windows are stubs that compile but do nothing. The
workspace builds two binaries: `openlogi` (CLI) and `openlogi-gui` (a GPUI
desktop app).

## 2. Build / run / test

```sh
cargo run -p openlogi --release -- list     # CLI
cargo run -p openlogi-gui --release         # desktop app
```

**Pre-commit gate — all of these must pass before committing:**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

(Equivalent: `devenv tasks run openlogi:check`.) Notes: GPUI needs Xcode 16+
with the Metal Toolchain component; after editing `devenv.nix`, run
`direnv reload`. Logging is controlled by the `OPENLOGI_LOG` env filter
(default `info`). Full workflow and packaging:
[`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md).

## 3. Crate map

Dependency direction: `openlogi-core` and `openlogi-assets` are foundation
crates with no internal deps; `openlogi-hid` and `openlogi-hook` depend on
core; `openlogi-cli` and `openlogi-gui` sit on top.

| Crate | Role |
|---|---|
| `openlogi-core` | Types, TOML config, paths, button/action catalog. I/O-free except its own config file. No `hidpp`/`async-hid`/platform APIs. |
| `openlogi-hid` | HID++ over `hidpp` + `async-hid`: enumerate, DPI (`0x2201`), SmartShift (`0x2111`), control capture (`0x1b04`/`0x2150`). |
| `openlogi-hook` | macOS `CGEventTap` mouse hook + Accessibility + frontmost-app detection. Stub elsewhere. |
| `openlogi-assets` | Device-render registry schema + cached, sha256-verified HTTP fetch from `assets.openlogi.org`. |
| `openlogi-cli` | CLI command tree (`list` / `diag` / `assets`) + `run()`. |
| `openlogi-gui` | The `openlogi-gui` binary — GPUI + gpui-component desktop app. |

## 4. Conventions

Rust edition 2024, MSRV `rust-version = 1.85`. The workspace runs Clippy
`pedantic` (warn) with `unwrap_used` and `expect_used` warned; the pre-commit
gate runs `cargo clippy --workspace --all-targets -- -D warnings`, so any of
those lints fails the build.

`unsafe` is denied workspace-wide via `unsafe_code = "deny"`
(`workspace.lints.rust`). Because the level is `deny` (not `forbid`), the
three modules that genuinely need FFI opt in locally with
`#[expect(unsafe_code, reason = "…")]`:

- `openlogi-hook` (`src/macos.rs`) — `CGEventTap` / `CFRunLoop` /
  Accessibility FFI; opts in crate-wide via `unsafe_code = "allow"` in its
  `Cargo.toml`.
- `openlogi-core/src/binding.rs` — the Dock SPI
  (`CoreDockSendNotification`) resolved through `dlopen`/`dlsym`.
- `openlogi-gui/src/platform/status_item.rs` — Cocoa `NSStatusItem` /
  `NSMenu` FFI for the menu-bar item (GPUI has no menu-bar API).

Every opt-in carries a `reason`, and each `unsafe` block carries a
`// SAFETY:` comment justifying it. Logging goes through the `tracing` crate;
the level is set by the `OPENLOGI_LOG` env filter (default `info`).
