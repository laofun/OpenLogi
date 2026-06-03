# Fork Divergence — `laofun/OpenLogi` vs `AprilNEA/OpenLogi`

This fork adds support for the **Logitech MX Master 2S** (`046d:b019`), a device
upstream does not support. Upstream targets the MX Master 3 / 3S generation,
which exposes a newer set of HID++ features; the 2S uses the older equivalents.
Every divergence below exists to bridge that gap.

This document is the map for **future upstream merges**: it records *what* the
fork changed, *why*, and a **post-merge checklist** ending in a hardware test on
real 2S hardware. Read it before merging any `upstream/master` into this fork.

- **Upstream remote:** `upstream` → `https://github.com/AprilNEA/OpenLogi.git`
- **Origin (fork):** `origin` → `https://github.com/laofun/OpenLogi.git`
- **Last merged upstream point:** `700e2a6` (`ci: configure release-plz branch
  prefix`) — the v0.4.0 line. Update this when you next merge upstream.

> Regenerate the file-level divergence at any time with:
> `git diff --stat 700e2a6 HEAD -- crates/` (swap `700e2a6` for the recorded
> merge point above).

---

## 1. The feature gap

| Concern | Upstream (MX 3 / 3S) | This fork adds (MX 2S) |
|---|---|---|
| Battery | `0x1004 UnifiedBattery` only | `0x1000 BatteryStatus` (legacy) decode |
| SmartShift | `0x2111 SmartShiftWheelEnhanced` | `0x2110 SmartShiftWheel` (legacy) + per-device sensitivity |
| DPI | live read from `0x2201 AdjustableDpi` | + a known-good *reference* range for the 2S |
| Identity | receiver-reported model info | direct-attach enrichment for `046d:b019` |

The `0x2110` ↔ `0x2111` and `0x1000` ↔ `0x1004` pairs are **not** versioned
variants of one feature — they have different function-ID layouts and wire
formats. Code that assumes the modern ID silently no-ops the 2S. See the
SmartShift gotcha in [`CLAUDE.md`](../CLAUDE.md#5-critical-gotchas).

---

## 2. Isolation strategy

The fork-only logic is deliberately **quarantined into its own modules** so it
does not touch the files upstream changes most. The shared write path
(`write.rs`) keeps upstream's shape; the fork-only feature backends live beside
it. This shrinks the merge conflict surface to a handful of one-line call sites
plus additive new files (which never conflict).

### Fork-only modules (additive — upstream has no counterpart, so they never conflict)

| File | Responsibility |
|---|---|
| `crates/openlogi-hid/src/smartshift_backend.rs` | `SmartShift` enum: dual `0x2110`/`0x2111` backend, probe-and-fallback, wire encodings, the function-ID-shift gotcha. |
| `crates/openlogi-hid/src/battery_status.rs` | `0x1000 BatteryStatus` feature wrapper + decode (percentage/level/status, `0%` "unknown" sentinel handling). |
| `crates/openlogi-hid/src/battery_diag.rs` | Route-level battery summary probing both `0x1000` and `0x1004`. |
| `crates/openlogi-hid/src/device_identity.rs` | Device-info summary + the MX 2S reference DPI range (`DpiReference`). |
| `crates/openlogi-cli/src/cmd/diag/battery.rs` | `diag battery` command (consumes `battery_diag`). |
| `crates/openlogi-cli/src/cmd/diag/controls.rs` | `diag controls` command (consumes `dump_reprog_controls`). |

### Shared files the fork modifies (these *can* conflict on a merge)

| File | Fork change | Conflict risk |
|---|---|---|
| `crates/openlogi-hid/src/write.rs` | SmartShift entry points (`get_smartshift_status`, `toggle_smartshift_on_channel`, `set_smartshift_sensitivity`) call `SmartShift::open()` instead of opening `0x2111` directly. | **Medium** — 2 functions upstream also has. One line each; re-point at `SmartShift::open`. |
| `crates/openlogi-hid/src/lib.rs` | Adds `mod` + `pub use` for the fork-only modules. | **Low** — additive lines in the module/re-export lists. |
| `crates/openlogi-hid/src/inventory.rs` | `known_direct_device` / `enrich_direct_model_info` enrich the all-zero placeholder identity for `046d:b019`; `probe_legacy_battery`. | **Low–Medium** — additive functions, but touches the shared enrich path. |
| `crates/openlogi-hid/src/reprog_controls.rs` | Adds `ControlEntry` + `dump_reprog_controls` (the `0x1b04` diagnostic), co-located with the feature wrapper. | **Low** — additive. |
| `crates/openlogi-hid/src/smartshift.rs` | `SmartShiftMode::flipped` + tests. | **Low** — additive. |
| `crates/openlogi-core/src/device.rs` | `BatteryInfo::percentage_display` (`?%` for the `0` sentinel). | **Low** — additive method. |
| `crates/openlogi-core/src/config.rs` | per-device `smartshift_sensitivity` (`Option<u8>`) + accessors. | **Low** — additive config field; serde-skipped when `None`. |
| `crates/openlogi-gui/src/state.rs` | DPI state machine never settles on permanent `Unsupported` (routes every error through the retry budget to `Failed`). | **Medium** — upstream owns this state machine; re-apply the policy if it changes. |
| `crates/openlogi-gui/src/hardware.rs` | SmartShift sensitivity read/write wiring. | **Low–Medium**. |
| `crates/openlogi-cli/src/cmd/diag/{dpi,smartshift,mod,features}.rs`, `cmd/list.rs` | wire the new diagnostics + 2S reference range into existing commands. | **Low–Medium** — additive arms in existing commands. |

**Genuine semantic conflict points to expect:** only the SmartShift reroute in
`write.rs` (2 functions) and the GUI DPI-`Failed` policy in `state.rs`.
Everything else is additive and should auto-merge.

---

## 3. Regression tests that lock fork behavior

These tests turn a 2S-breaking upstream merge **red in CI** instead of letting
it slip through silently. If one of these fails after a merge, the merge changed
fork-critical behavior — do not "fix" the test without understanding why.

| Test location | Locks |
|---|---|
| `openlogi-hid/src/smartshift_backend.rs` (`mod tests`) | `0x2110`/`0x2111` byte-encoding match; missing-`0x2111` triggers fallback; missing-`0x2110` and transport errors do **not**. |
| `openlogi-hid/src/battery_status.rs` (`mod tests`) | `0x1000` decode: status enum, `is_informative`, percentage→level, the `0%` sentinel. |
| `openlogi-hid/src/inventory.rs` (`mod tests`) | `046d:b019` recognised as a known direct device; `config_key == "0b019"`; placeholder-identity enrichment. |
| `openlogi-core/src/config.rs` (`mod tests`) | per-device `smartshift_sensitivity` round-trips and is serde-skipped when `None`. |
| `openlogi-gui/src/state.rs` (`mod tests`) | DPI discovery **never** settles on `Unsupported` for a real device; recovers to `Ready` after transient failures. |

---

## 4. Post-merge checklist

Run this **in order** after merging `upstream/master`. Do not skip the hardware
test — the automated gate cannot exercise real HID++ traffic.

1. **Resolve conflicts** — expect them only in `write.rs` (SmartShift reroute)
   and `state.rs` (GUI DPI-`Failed` policy). Re-apply the fork behavior from
   §2; the rest should auto-merge.
2. **Update the recorded merge point** in §0 of this file to the new
   `upstream/master` tip.
3. **Re-scan divergence** — `git diff --stat <new-merge-point> HEAD -- crates/`
   and update §2's tables if upstream moved fork-touched code.
4. **Run the full gate** (do **not** unset `DEVELOPER_DIR` — full Xcode is
   required for GUI builds):
   ```sh
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
   All §3 regression tests must be green. A red one means the merge changed
   2S-critical behavior — investigate before proceeding.
5. **Hardware test on a real MX Master 2S** (`046d:b019`, direct BLE/USB):
   ```sh
   cargo run -p openlogi --release -- list          # 2S appears, battery shows a real % (or ?% while charging)
   cargo run -p openlogi --release -- diag battery   # 0x1000 present + decoded
   cargo run -p openlogi --release -- diag smartshift # 0x2110 status reads; toggle + sensitivity write succeed
   cargo run -p openlogi --release -- diag dpi        # 200..4000 step ≈ 50; round-trip OK
   cargo run -p openlogi-gui --release                # DPI panel shows the slider (not "did not report Adjustable DPI support"); SmartShift + sensitivity work
   ```
   Expected: DPI panel shows `200–4000 · step 50`. If a transient miss occurs it
   must show "Couldn't read DPI — click to retry" and recover on click — never a
   permanent "not supported" dead end.
6. **Do not push** without explicit request.
