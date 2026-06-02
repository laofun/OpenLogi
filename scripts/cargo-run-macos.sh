#!/usr/bin/env bash
#
# Cargo `runner` for macOS — wired in `.cargo/config.toml`.
#
# Cargo hands this script the freshly built binary as $1 for every
# `cargo run` / `cargo test` / `cargo bench` on macOS. For everything except
# the desktop binary it's a transparent passthrough (`exec "$@"`).
#
# For `openlogi-gui` it launches the build from inside a throwaway
# `OpenLogi.app` so macOS shows the real app name (the bold menu-bar title)
# and the Dock icon during development. Both are read from the bundle's
# `Info.plist` / `Resources` — a bare `target/debug/openlogi-gui` has neither,
# so macOS falls back to the executable name and a generic icon.
#
# Set OPENLOGI_DEV_BUNDLE=0 to skip the wrapper and run the raw binary.
set -euo pipefail

bin="$1"
shift

if [ "${bin##*/}" != "openlogi-gui" ] || [ "${OPENLOGI_DEV_BUNDLE:-1}" = "0" ]; then
  exec "$bin" "$@"
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$ROOT/target/dev/OpenLogi.app"
MACOS="$APP/Contents/MacOS"
RES="$APP/Contents/Resources"
ICON_SRC="$ROOT/crates/openlogi-gui/icon/AppIcon.icns"
PLIST_SRC="$ROOT/crates/openlogi-gui/dev/Info.plist"

mkdir -p "$MACOS" "$RES"

# App icon — gitignored, generated from the master SVG on demand. Mirror it
# into the bundle whenever the source is newer (or the bundle copy is missing).
if [ ! -f "$ICON_SRC" ]; then
  cargo run -p xtask --manifest-path "$ROOT/Cargo.toml" -- macos-icns
fi
if [ "$ICON_SRC" -nt "$RES/AppIcon.icns" ]; then
  cp -f "$ICON_SRC" "$RES/AppIcon.icns"
fi

# Info.plist — minimal, dev-only. A distinct `.dev` identifier keeps this
# target artifact from registering as the production app in LaunchServices.
PLIST="$APP/Contents/Info.plist"
if [ "$PLIST_SRC" -nt "$PLIST" ]; then
  cp -f "$PLIST_SRC" "$PLIST"
fi

# Hardlink the freshly built binary into the bundle — instant, no 95 MB copy.
# A hardlink (not a symlink) is required: both NSBundle.mainBundle and Rust's
# current_exe() realpath() the executable, which would resolve a symlink back
# to target/debug/ and break the bundle association. cargo rewrites the binary
# atomically on rebuild (new inode), so relink every run; `ln -f` repoints a
# stale link. Fall back to a copy if the bundle ever lands on another volume.
ln -f "$bin" "$MACOS/openlogi-gui" 2>/dev/null || cp -f "$bin" "$MACOS/openlogi-gui"

# Re-sign the bundle with a stable identity so macOS (TCC) keeps the
# Accessibility / Input Monitoring / Bluetooth grants across rebuilds. The
# default cargo/linker signature is ad-hoc, which TCC keys on the binary's
# cdhash — that changes every build, so each rebuild looks like a brand-new app
# and the grants are dropped. Signing with a fixed certificate makes TCC key on
# the bundle id + cert (both stable) instead.
#
# Identity resolution (first match wins):
#   1. $OPENLOGI_DEV_SIGN_IDENTITY, if set
#   2. a self-signed "OpenLogi Dev" code-signing cert in the keychain
# If neither is found we leave the ad-hoc signature in place (CI / contributors
# without the cert keep working — they just re-grant permissions after a build).
# Create the cert once with: scripts/setup-dev-signing.sh
SIGN_ID="${OPENLOGI_DEV_SIGN_IDENTITY:-}"
if [ -z "$SIGN_ID" ] && security find-identity -p codesigning 2>/dev/null \
     | grep -q '"OpenLogi Dev"'; then
  SIGN_ID="OpenLogi Dev"
fi
if [ -n "$SIGN_ID" ]; then
  # --force replaces the existing (ad-hoc) signature. Sign the binary, not the
  # whole bundle: the hardlinked Info.plist/Resources don't change between
  # builds, so signing the Mach-O is enough and avoids re-hashing the icon.
  codesign --force --sign "$SIGN_ID" "$MACOS/openlogi-gui" 2>/dev/null \
    || echo "warning: codesign with '$SIGN_ID' failed — permissions may reset" >&2
fi

exec "$MACOS/openlogi-gui" "$@"
