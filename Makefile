# OpenLogi — dev convenience targets.
#
# Thin wrappers over the cargo / xtask / scripts workflow already in the repo.
# Nothing here is required to build — `cargo` works directly — but these
# shortcuts capture the exact incantations, especially for running the GUI
# locally against a real device (the MX Master 2S this fork targets).
#
# macOS notes:
#   - GUI builds need full Xcode (Metal toolchain). DEVELOPER_DIR is handled by
#     .cargo/config.toml (points at /Applications/Xcode.app, force = false), so
#     these targets work both inside and outside the devenv shell. Do NOT unset
#     it for GUI builds.
#   - The GUI is launched from a throwaway OpenLogi.app via the cargo runner
#     (scripts/cargo-run-macos.sh) so macOS shows the real name + Dock icon and
#     keeps TCC permission grants across rebuilds (see `make sign-setup`).

CARGO       ?= cargo
APP_RELEASE := target/release/bundle/osx/OpenLogi.app
INSTALL_DIR := /Applications
SIGN_ID     ?= OpenLogi Dev

.DEFAULT_GOAL := help

# ---------------------------------------------------------------------------
# Help
# ---------------------------------------------------------------------------

.PHONY: help
help: ## Show this help
	@echo "OpenLogi dev targets:"
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
	  | sort \
	  | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

# ---------------------------------------------------------------------------
# Quality gate (matches the pre-commit gate in CLAUDE.md / devenv check)
# ---------------------------------------------------------------------------

.PHONY: check
check: fmt-check clippy test ## Full pre-commit gate: fmt + clippy + test

.PHONY: fmt
fmt: ## Format all crates in place
	$(CARGO) fmt --all

.PHONY: fmt-check
fmt-check: ## Check formatting (fails if unformatted)
	$(CARGO) fmt --all -- --check

.PHONY: clippy
clippy: ## Lint with warnings denied
	$(CARGO) clippy --workspace --all-targets -- -D warnings

.PHONY: test
test: ## Run the workspace test suite
	$(CARGO) test --workspace

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

.PHONY: list
list: ## List connected Logitech HID++ devices (CLI)
	$(CARGO) run -p openlogi --release -- list

.PHONY: assets
assets: ## Sync device render assets
	$(CARGO) run -p openlogi --release -- assets sync

# ---------------------------------------------------------------------------
# GUI — local dev against the MX Master 2S
# ---------------------------------------------------------------------------

.PHONY: gui
gui: ## Build + run the desktop app (release; signed dev bundle, keeps TCC grants)
	$(CARGO) run -p openlogi-gui --release

.PHONY: gui-debug
gui-debug: ## Build + run the desktop app (debug; faster build, slower runtime)
	$(CARGO) run -p openlogi-gui

.PHONY: build-gui
build-gui: ## Build the desktop binary only (release, no launch)
	$(CARGO) build -p openlogi-gui --release

.PHONY: sign-setup
sign-setup: ## One-time: create the "OpenLogi Dev" signing cert (persists TCC grants)
	scripts/setup-dev-signing.sh

# ---------------------------------------------------------------------------
# MX Master 2S diagnostics — verify the fork-only feature paths on real hardware
# ---------------------------------------------------------------------------

.PHONY: diag-mx2s
diag-mx2s: ## Run the MX2S diag sweep (battery 0x1000, SmartShift 0x2110, DPI)
	@echo "==> diag battery (0x1000 legacy + 0x1004 unified)"
	$(CARGO) run -p openlogi --release -- diag battery
	@echo "==> diag smartshift (0x2110 legacy path)"
	$(CARGO) run -p openlogi --release -- diag smartshift
	@echo "==> diag dpi (200..4000 step ~50, round-trip)"
	$(CARGO) run -p openlogi --release -- diag dpi

# ---------------------------------------------------------------------------
# Packaging / install for everyday MX2S use
# ---------------------------------------------------------------------------

.PHONY: bundle
bundle: ## Build OpenLogi.app (release) at target/release/bundle/osx
	$(CARGO) run -p xtask -- bundle-macos

.PHONY: install
install: bundle ## Bundle, sign with the dev cert, install into /Applications
	@if [ ! -d "$(APP_RELEASE)" ]; then \
	  echo "error: $(APP_RELEASE) not found — bundle step failed" >&2; exit 1; fi
	@if security find-identity -p codesigning 2>/dev/null | grep -q '"$(SIGN_ID)"'; then \
	  echo "==> signing with '$(SIGN_ID)' (keeps TCC grants across reinstalls)"; \
	  codesign --force --deep --sign "$(SIGN_ID)" "$(APP_RELEASE)"; \
	else \
	  echo "warning: '$(SIGN_ID)' cert not found — run 'make sign-setup' first."; \
	  echo "         Installing the ad-hoc-signed bundle; you'll re-grant"; \
	  echo "         permissions after each reinstall."; \
	fi
	@echo "==> installing to $(INSTALL_DIR)/OpenLogi.app"
	rm -rf "$(INSTALL_DIR)/OpenLogi.app"
	cp -R "$(APP_RELEASE)" "$(INSTALL_DIR)/OpenLogi.app"
	@echo "==> done. Launch from Spotlight/Applications, grant Accessibility +"
	@echo "    Input Monitoring + Bluetooth once, then use it with your MX2S."

.PHONY: dmg
dmg: ## Build a distributable macOS DMG
	$(CARGO) run -p xtask -- package-macos

# ---------------------------------------------------------------------------
# Housekeeping
# ---------------------------------------------------------------------------

.PHONY: clean
clean: ## Remove the cargo build directory
	$(CARGO) clean
