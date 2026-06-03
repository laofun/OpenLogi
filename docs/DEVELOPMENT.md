# Developing OpenLogi

This document covers the local development workflow for OpenLogi. For end-user
build instructions, see the [README](../README.md).

## Toolchain

- Stable Rust (Edition 2024, MSRV 1.85)
- macOS: Xcode 16+ with the optional **Metal Toolchain** component (required by
  GPUI's `gpui_macos` build script to compile shaders)
- `create-dmg` for packaging (`brew install create-dmg`); `cargo-bundle` is
  installed automatically by `cargo run -p xtask -- bundle-macos`

## Building from source

CLI:

```sh
git clone https://github.com/AprilNEA/OpenLogi
cd OpenLogi
cargo run -p openlogi --release -- list
```

Desktop app:

```sh
cargo run -p openlogi-gui --release
```

On macOS the desktop binary is launched from inside a throwaway
`target/dev/OpenLogi.app` — a Cargo `runner` wired in `.cargo/config.toml`
(`scripts/cargo-run-macos.sh`). This makes the dev build show the real
**OpenLogi** name in the menu bar and the app icon in the Dock; a bare
`cargo run` binary has no bundle, so macOS would otherwise fall back to the
`openlogi-gui` executable name and a generic icon. The binary is hardlinked in
(no copy) and the icon is generated on demand by
`cargo run -p xtask -- macos-icns`. The runner is a transparent passthrough for
everything else (the CLI, tests); set
`OPENLOGI_DEV_BUNDLE=0` to launch the raw `openlogi-gui` binary instead.

To install the CLI binary on `PATH`:

```sh
cargo install --path .
```

## Make targets

A top-level `Makefile` wraps the common cargo / xtask / scripts incantations —
handy shortcuts, not a required build step (`cargo` works directly). Run
`make help` for the full list. The ones worth knowing:

```sh
make check        # fmt + clippy + tests (the pre-commit gate)
make gui          # build + run the desktop app (release, signed dev bundle)
make sign-setup   # one-time: create the dev signing cert (persists TCC grants)
make diag-mx2s    # MX Master 2S diag sweep: battery 0x1000, SmartShift 0x2110, DPI
make install      # bundle + sign + install OpenLogi.app into /Applications
```

`make install` is the everyday path for using the app with a real device: it
builds the release `OpenLogi.app`, signs it with the `OpenLogi Dev` cert (so the
TCC grants survive reinstalls — see below), and copies it into `/Applications`.

## Using devenv (macOS)

The repo's `devenv.nix` provisions a Nix-based dev shell with sccache, the
stable Rust toolchain, and the env overrides GPUI needs. It exposes tasks that
mirror CI and packaging:

```sh
devenv tasks run openlogi:gui      # run the desktop app
devenv tasks run openlogi:check    # fmt + clippy + tests (run before committing)
devenv tasks run openlogi:dmg      # build the macOS DMG
```

The first time you `cd` into the repo after pulling a change to `devenv.nix`,
**reload direnv** so the new env vars (`DEVELOPER_DIR`, `SDKROOT`, the PATH
filter that strips Nix's `xcbuild` xcrun stub) take effect:

```sh
direnv reload    # or: exit your shell and `cd` back in
```

Without that, GPUI's `gpui_macos` build script can't find Apple's `metal`
shader compiler, and link errors about missing `_write` / `_sysconf` /
`_waitpid` symbols show up because the Nix `apple-sdk-14.4` stub doesn't
expose `libSystem` the way Apple's real linker wants.

## Keeping macOS permissions across rebuilds

OpenLogi needs Accessibility, Input Monitoring, and (for BLE-direct mice)
Bluetooth grants. macOS (TCC) ties those grants to the app's **code
signature**. A bare `cargo build` produces an *ad-hoc* signature, which TCC
keys on the binary's **cdhash** — and the cdhash changes on every rebuild, so
each `cargo run -p openlogi-gui` looks like a brand-new app and the grants are
dropped. That's why permissions reset every time you rebuild.

The fix is to sign the dev bundle with a **stable self-signed certificate**.
When the bundle carries a real certificate, TCC keys the grant on the
*designated requirement* — `bundle id + certificate leaf` — instead of the
cdhash. Both are stable across rebuilds, so you grant permissions once.

Create the certificate once (idempotent — safe to re-run):

```sh
scripts/setup-dev-signing.sh
```

This generates a self-signed `"OpenLogi Dev"` code-signing cert (10-year
validity) and imports it into your login keychain. The dev runner
(`scripts/cargo-run-macos.sh`, wired as the cargo `runner` in
`.cargo/config.toml`) then re-signs the bundle with it on every
`cargo run -p openlogi-gui`. To use a different identity (e.g. a real Apple
Development cert), set `OPENLOGI_DEV_SIGN_IDENTITY` and it takes precedence.

Notes:

- The cert shows as untrusted (`CSSMERR_TP_NOT_TRUSTED`) in
  `security find-identity` — that's expected and harmless. `codesign` signs
  with it regardless, and TCC keys the grant on it. We deliberately don't add
  it to the trust store (that needs `sudo`).
- The **first** run after creating the cert still needs a one-time grant — the
  bundle moves from ad-hoc to the new identity, so TCC sees a new signature
  once. After that, rebuilds keep the grants.
- This is dev-only. Production bundles are signed by `xtask` with a real
  Developer ID (`OPENLOGI_SIGN_IDENTITY`) — see *Packaging the macOS DMG*.
- Contributors and CI without the cert keep working: the runner leaves the
  ad-hoc signature in place when no signing identity is found.

## Project layout

```
src/                the `openlogi` binary (workspace root package) — a thin wrapper over openlogi-cli
crates/
  openlogi-core/    types, config (TOML), paths, button + action catalog — no HID, no async
  openlogi-hid/     hidpp + async-hid: enumerate(), DPI (0x2201) and SmartShift (0x2111) writes
  openlogi-assets/  device-render registry schema + cached HTTP fetch from assets.openlogi.org
  openlogi-cli/     CLI implementation: command tree + `run()`, called by the `openlogi` binary
  openlogi-hook/    macOS CGEventTap mouse hook + Accessibility + frontmost-app detection
  openlogi-gui/     the `openlogi-gui` binary — GPUI + gpui-component
```

## Pre-commit checklist

Before committing, the following must pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Equivalent to `devenv tasks run openlogi:check`.

## Packaging the macOS DMG

```sh
cargo run -p xtask -- package-macos    # → target/release/OpenLogi.dmg
```

Environment overrides:

- `OPENLOGI_BUNDLE_ASSETS=1` — bundle every device render into the `.app` for a
  fully offline build (default: fetched on demand at first launch).
- `OPENLOGI_SIGN_IDENTITY=<identity>` — codesign the `.app` and `.dmg` with the
  given Developer ID.
- `OPENLOGI_DMG_BACKGROUND_URL=<url>` — override the branded DMG background
  TIFF URL (default: `https://assets.openlogi.org/dmg/dmg-background.tiff`).

The local packaging command and release workflow both use the same branded DMG
layout: a 760×480 background image in a 760×512 Finder window, with 128px icons
positioned at `(212, 250)` for `OpenLogi.app` and `(548, 250)` for
`Applications`.

## Release updater publishing

Tagged releases still attach DMGs and `SHA256SUMS` to GitHub Releases for manual
downloads and the Homebrew cask. The release workflow also publishes the same
DMGs to Cloudflare R2 and writes a static updater manifest at:

```text
${OPENLOGI_UPDATE_BASE_URL}/channels/stable/latest.json
```

The app embeds that manifest URL at build time via
`OPENLOGI_UPDATE_MANIFEST_URL`, derived from `OPENLOGI_UPDATE_BASE_URL` in the
release workflow.

Configure the R2/update settings in one 1Password item referenced by the GitHub
secret `OP_R2_SECRET_ITEM`. The item must contain:

- `OPENLOGI_UPDATE_BASE_URL` — public HTTPS base URL, for example
  `https://updates.openlogi.org`.
- `CLOUDFLARE_R2_ACCOUNT_ID` — Cloudflare account ID used for the S3 endpoint.
- `CLOUDFLARE_R2_BUCKET` — bucket name.
- `CLOUDFLARE_R2_ACCESS_KEY_ID` — R2 S3 access key.
- `CLOUDFLARE_R2_SECRET_ACCESS_KEY` — R2 S3 secret key.

The workflow uploads immutable artifacts under `/releases/<tag>/` and only the
channel manifest under `/channels/stable/latest.json` is mutable.

The manifest is generated by the workspace `xtask` helper:

```sh
cargo run -p xtask -- generate-updater-manifest \
  --dist dist \
  --tag v0.2.0 \
  --base-url https://updates.openlogi.org \
  --output dist/latest.json
```
