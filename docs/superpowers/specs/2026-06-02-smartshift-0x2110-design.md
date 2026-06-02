# SmartShift `0x2110` Support (toggle free↔ratchet) — Design

**Date:** 2026-06-02
**Status:** Approved, pending spec review

## Goal

Let OpenLogi toggle SmartShift on Logitech mice that expose the original
`0x2110 SmartShiftWheel` feature (e.g. the MX Master 2S) in addition to the
newer `0x2111 SmartShiftWheelEnhanced` it already drives. The MX Master 2S
exposes `0x2110 v0` and **not** `0x2111`, so `openlogi diag smartshift`
currently fails with `device does not expose HID++ feature 0x2111`.

## Scope & Decisions

- **Functional scope:** toggle only — read the current wheel mode, write the
  opposite (free-spin ↔ ratchet), keep the existing auto-disengage threshold
  unchanged. No sensitivity read/write UI (YAGNI — nothing consumes it).
- **Selection strategy:** try `0x2111` first, fall back to `0x2110`. This keeps
  current behaviour for MX Master 3 / 3S (Enhanced-only) and transparently adds
  2S support, with no configuration and no per-device gating.
- **Crate boundaries:** changes are confined to
  `crates/openlogi-hid/src/write.rs` (plus inline tests). No new files. The
  vendored `openlogi-hidpp` fork is **not** modified (it is third-party code and
  already ships a `0x2110` wrapper). The GUI, the config schema, and
  `SharedChannel` are untouched.
- **No code change required** for device detection, DPI, or reprog controls on
  the 2S — those are already dynamic feature-probing and verified working
  (`diag dpi` passes, `0x1b04 v3` present).

## Source of Truth

Grounded in the current tree (read during brainstorming):

- `crates/openlogi-hid/src/write.rs` — `open_feature`, `get_smartshift_status`,
  `toggle_smartshift`, `toggle_smartshift_on_channel`, `WriteError`.
- `crates/openlogi-hid/src/smartshift.rs` — `SmartShiftFeatureV0` (`0x2111`),
  `SmartShiftMode { Free, Ratchet }`, `SmartShiftStatus { mode, sensitivity }`,
  `as_byte`/`from_byte`/`flipped`.
- `crates/openlogi-hidpp/src/feature/smartshift/mod.rs` — the fork's existing
  `0x2110` wrapper `SmartShiftFeature`: `get_ratchet_control_mode()`,
  `set_ratchet_control_mode(wheel_mode, auto_disengage, default)`,
  `WheelMode { Freespin = 1, Ratchet = 2 }`.
- `crates/openlogi-cli/src/cmd/diag/smartshift.rs` — the smoke test that
  exercises the path.
- `crates/openlogi-gui/src/hardware.rs` — calls `toggle_smartshift_on` /
  `toggle_smartshift`; unchanged because it routes through the same internal
  function the fallback lives in.

Where source and this spec disagree at write time, **source wins**.

## Key Reuse Insight

The fork's `0x2110` `SmartShiftFeature` is already a `CreatableFeature`, and its
`WheelMode { Freespin = 1, Ratchet = 2 }` encoding matches
`SmartShiftMode::as_byte()` (`Free = 1`, `Ratchet = 2`) exactly. So no new
protocol wrapper is needed — only an internal dispatch in `write.rs` that opens
whichever feature the device exposes and maps both onto the existing
`SmartShiftMode`.

## Design

### Internal backend enum (private to `write.rs`)

```rust
// Not exported. Wraps whichever SmartShift feature the device exposes.
enum SmartShift {
    Enhanced(Arc<SmartShiftFeatureV0>),                          // 0x2111
    Legacy(Arc<hidpp::feature::smartshift::SmartShiftFeature>),  // 0x2110
}
```

Three methods map both feature flavours onto the existing `SmartShiftMode`:

| Method | Enhanced (`0x2111`) | Legacy (`0x2110`) |
|---|---|---|
| `open(device)` | `open_feature::<SmartShiftFeatureV0>` → `Enhanced` | on `FeatureUnsupported{0x2111}`, try `open_feature::<SmartShiftFeature>` → `Legacy` |
| `status()` → `SmartShiftStatus` | `get_status()` | `get_ratchet_control_mode()`; `mode` from `wheel_mode`, `sensitivity` from `auto_disengage` |
| `set_mode(m)` | `set_status(m, sensitivity_unchanged)` | `set_ratchet_control_mode(Some(wheel), None, None)` (`None` keeps auto-disengage) |

`WheelMode` ↔ `SmartShiftMode` mapping: `Freespin → Free`, `Ratchet → Ratchet`.
A reserved/unknown wheel-mode byte falls back to `SmartShiftMode::Ratchet`,
preserving the existing "safe / clicky" convention.

### Fallback discipline

Fallback to `0x2110` fires **only** when opening `0x2111` returns
`WriteError::FeatureUnsupported { feature_hex: 0x2111 }`. Any other error
(transport `Hid`, `DeviceUnreachable`, `Hidpp` protocol error) is returned
unchanged — the fallback must not swallow real failures. If `0x2110` is also
absent, the resulting `FeatureUnsupported { feature_hex: 0x2110 }` is returned.

### Rewritten call sites

1. `get_smartshift_status` (write.rs:128) — opens via `SmartShift::open`, returns
   `status()`. For Legacy devices `sensitivity` carries `auto_disengage` (enough
   for `diag` to print).
2. `toggle_smartshift_on_channel` (write.rs:206) — opens via `SmartShift::open`,
   reads `status().mode`, writes `set_mode(mode.flipped())`. Toggle logic
   unchanged.

`toggle_smartshift`, `toggle_smartshift_on`, `set_dpi_on`, `SharedChannel`, and
all GUI code keep their signatures. The GUI inherits 2S support for free because
it calls `toggle_smartshift_on` → `toggle_smartshift_on_channel`.

### Docstring fix

`toggle_smartshift` (write.rs:194) currently says it returns
`FeatureUnsupported` "when the device doesn't expose HID++ `0x2111`". Update to
"`0x2111` or the older `0x2110`".

## Error Handling

| Situation | Behaviour |
|---|---|
| Device exposes `0x2111` | Enhanced path, identical to today |
| Device exposes only `0x2110` (MX Master 2S) | fallback opens `0x2110`, toggle works |
| Device exposes neither | returns `FeatureUnsupported { 0x2110 }` |
| Transport / unreachable error opening `0x2111` | returned unchanged — no fallback |
| Reserved `WheelMode` byte | maps to `SmartShiftMode::Ratchet` |

## Testing

**Layer 1 — pure unit tests (run in CI, no hardware):**
- Encoding-parity test: assert `SmartShiftMode::Free.as_byte() == WheelMode::Freespin as u8`
  and `SmartShiftMode::Ratchet.as_byte() == WheelMode::Ratchet as u8`. This
  guards the central assumption that lets both features share `SmartShiftMode`.
- `WheelMode → SmartShiftMode` mapping helper round-trips for both variants.
- If not already covered, a `SmartShiftMode::flipped()` test.

**Layer 2 — fallback-decision test (if extractable):**
If the "should this error trigger fallback?" check is a small pure helper, test
that `FeatureUnsupported{0x2111}` → fallback; `FeatureUnsupported{0x2110}`,
`DeviceUnreachable`, `Hid` → no fallback.

**Layer 3 — manual smoke test (on the user's MX Master 2S):**
```sh
DEVELOPER_DIR=/Library/Developer/CommandLineTools \
  cargo run -p openlogi --release -- diag smartshift
```
Expected after the change: a successful `current → toggled → read-back →
✓ SmartShift round-trip OK`, and the wheel physically switching between ratchet
and free-spin. This matches the project's existing convention (CLAUDE.md notes
DPI/SmartShift writes are smoke-tested manually).

**Pre-commit gate (must pass):** `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## Out of Scope (YAGNI)

- Sensitivity / auto-disengage adjustment UI or API surface.
- Any per-device-model gating or PID lists.
- Asset-registry entry for the 2S render (separate, data-only, not a code task).
- Changes to the `openlogi-hidpp` fork.

## Success Criteria

- `openlogi diag smartshift` succeeds on the MX Master 2S (`0x2110`) and the
  wheel toggles between ratchet and free-spin.
- MX Master 3 / 3S behaviour (`0x2111`) is unchanged.
- The pre-commit gate passes; new pure-logic tests cover the encoding-parity
  assumption.
