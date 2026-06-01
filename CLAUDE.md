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
`direnv reload`. Full workflow and packaging:
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
| `openlogi-gui` | GPUI + gpui-component desktop app (the `openlogi-gui` binary). |

## 4. Conventions

Rust edition 2024, MSRV `rust-version = 1.85`. The workspace runs Clippy
`pedantic` (warn) with `unwrap_used` and `expect_used` warned; the pre-commit
gate runs `cargo clippy --workspace --all-targets -- -D warnings`, so any of
those lints fails the build.

`unsafe` is denied workspace-wide via `unsafe_code = "deny"`
(`workspace.lints.rust`). Because the level is `deny` (not `forbid`), the
three modules that genuinely need FFI opt back in locally — via an
`#[expect]`/`#[allow(unsafe_code, reason = "…")]` attribute or a crate-wide
`unsafe_code = "allow"`:

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

## 5. Critical gotchas

The non-obvious traps. Details and code references in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

- **`hidpp 0.2`'s feature registry is empty for our features.** Resolve a
  feature via `device.root().get_feature(ID)` + `device.add_feature::<F>()`,
  **not** `enumerate_features()` (its `versions: &[]` means a typed
  `get_feature::<F>()` returns `None`). Applies to `0x2201`, `0x2111`,
  `0x1b04`, `0x2150`. See `write::open_feature`.
- **SmartShift `0x2111` function IDs are shifted** vs the older `0x2110`:
  getStatus = function `1`, setStatus = function `2`. Using `0x2110`'s IDs
  silently no-ops the device.
- **Directly-attached devices are addressed at index `0xff`**
  (`DIRECT_DEVICE_INDEX`). `probe_direct`'s phantom-device guard (requires a
  battery *or* a control feature) keeps a Bolt receiver's secondary interface
  from being mistaken for a mouse — **don't remove it.**
- **`Action::execute` has no automated test** (it would have to intercept the
  OS event queue). Smoke-test it manually.
- **DPI writes read back and verify**; a mismatch logs a warning but still
  returns `Ok` (the request reached the device).
- **A few actions are stubs** — some media keys log their intended NX key
  instead of posting it.
- **The hook callback runs on a background thread**, not the GPUI thread, and
  must return quickly — blocking it stalls system-wide input.

## 6. Stability contracts

The variant *names* of `Action`, `ButtonId`, and `GestureDirection` are the
on-disk `config.toml` schema (serde external tagging). Appending new variants
is safe; renaming or removing one is a migration event that requires bumping
the `SCHEMA_VERSION` in `config.rs`.

## 7. Pointers

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — detailed architecture and the HID++ flow.
- [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) — full dev workflow + packaging.
- [`docs/USAGE.md`](docs/USAGE.md) — the CLI reference.
- [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) — the config file format.
