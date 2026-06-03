//! Pure scroll math and interpolation state for the macOS smooth-scroll
//! engine. No FFI lives here — the CGEvent/CVDisplayLink glue is in `macos.rs`.
//! This module is unit-tested in isolation.

/// Mos's upper limit for the duration knob (`5.0 + 0.2`).
const DURATION_UPPER_LIMIT: f64 = 5.2;

/// Map the user-facing `duration` (Mos range 1..=5) to a lerp coefficient.
/// Larger duration → smaller coefficient → slower convergence → longer tail.
/// Mirrors Mos `generateDurationTransition`.
#[must_use]
pub fn duration_to_transition(duration: f64) -> f64 {
    let val = 1.0 - (duration / DURATION_UPPER_LIMIT).sqrt();
    (val * 1000.0).round() / 1000.0
}

/// One interpolation step: the fraction of the remaining distance to travel
/// this frame. Mirrors Mos `Interpolator.lerp`.
#[must_use]
pub fn lerp(src: f64, dest: f64, trans: f64) -> f64 {
    (dest - src) * trans
}

/// Raise sub-`step` input magnitudes up to the minimum quantum, preserving
/// sign. A zero input stays zero. Mirrors Mos `normalize`.
#[must_use]
pub fn normalize(value: f64, step: f64) -> f64 {
    if value == 0.0 {
        return 0.0;
    }
    if value.abs() < step {
        return step.copysign(value);
    }
    value
}

/// Classify an event as trackpad-origin from its phase/count fields. Any
/// nonzero phase or scroll-count signals a trackpad (or already-continuous
/// source) that the engine must leave untouched. Mirrors Mos `isTrackpad`.
#[must_use]
pub fn is_trackpad(scroll_phase: f64, momentum_phase: f64, scroll_count: i64) -> bool {
    scroll_phase != 0.0 || momentum_phase != 0.0 || scroll_count != 0
}

/// Accumulates scroll deltas and emits per-frame interpolated output until it
/// converges. The tap thread calls [`Self::add`]; the display-link thread
/// calls [`Self::advance`].
#[derive(Debug, Default)]
pub struct SmoothEngine {
    /// Target accumulated distance.
    buffer: (f64, f64),
    /// Distance already emitted.
    current: (f64, f64),
}

impl SmoothEngine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an input tick (already inverted if applicable). `dx`/`dy` are the
    /// raw event deltas; they are normalized to `step` then scaled by `speed`.
    pub fn add(&mut self, dx: f64, dy: f64, speed: f64, step: f64) {
        self.buffer.0 += normalize(dx, step) * speed;
        self.buffer.1 += normalize(dy, step) * speed;
    }

    /// Advance one frame. Returns the `(dx, dy)` to post this frame, or `None`
    /// once the run has settled (output below `dead_zone` on both axes), at
    /// which point the engine resets so the next burst starts clean.
    pub fn advance(&mut self, transition: f64, dead_zone: f64) -> Option<(f64, f64)> {
        let step = (
            lerp(self.current.0, self.buffer.0, transition),
            lerp(self.current.1, self.buffer.1, transition),
        );
        if step.0.abs() <= dead_zone && step.1.abs() <= dead_zone {
            // Settled: drop the residue and park.
            self.buffer = (0.0, 0.0);
            self.current = (0.0, 0.0);
            return None;
        }
        self.current.0 += step.0;
        self.current.1 += step.1;
        Some(step)
    }

    /// True when there is no in-flight scroll (nothing buffered or emitted).
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.buffer == (0.0, 0.0) && self.current == (0.0, 0.0)
    }
}

use std::sync::Mutex;

/// Minimum dead-zone used when consuming frames. Flooring `dead_zone` here
/// guarantees `SmoothEngine::advance` always converges to its cutoff: with a
/// non-positive dead_zone the geometric lerp tail never falls below it and the
/// display link would emit tiny frames forever. The GUI also clamps its slider
/// to this minimum.
pub const MIN_DEAD_ZONE: f64 = 0.1;

/// Engine plus the run-flag and target PID, all guarded by one lock so the
/// buffer state and "is the display link running" flag flip atomically.
#[derive(Debug, Default)]
struct EngineState {
    engine: SmoothEngine,
    /// Whether the display link is currently driving this engine.
    running: bool,
    /// PID of the app the original scroll targeted, captured on the last input.
    target_pid: i32,
}

/// Shared smoothing state. The tap thread calls [`Self::push`]; the display-link
/// thread calls [`Self::frame`] / [`Self::rearm_if_pending`]. All state lives
/// under a single mutex so a push that races a park cannot lose its wakeup.
#[derive(Debug, Default)]
pub struct SharedSmooth {
    state: Mutex<EngineState>,
}

impl SharedSmooth {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an input tick (already inverted) and remember its target PID.
    /// Returns `true` when the caller must start the display link — i.e. this
    /// push armed a previously idle engine. The run-flag flips under the same
    /// lock as the buffer, so a concurrent [`Self::frame`] parking the link
    /// cannot lose this wakeup.
    pub fn push(&self, dx: f64, dy: f64, speed: f64, step: f64, pid: i32) -> bool {
        let Ok(mut st) = self.state.lock() else {
            return false;
        };
        st.target_pid = pid;
        st.engine.add(dx, dy, speed, step);
        if st.running {
            false
        } else {
            st.running = true;
            true
        }
    }

    /// Advance one frame. `Some((dx, dy, pid))` to post, or `None` when the run
    /// has settled. On `None`, clears the run-flag under the lock; the caller
    /// must then call [`Self::rearm_if_pending`] before stopping the link.
    pub fn frame(&self, transition: f64, dead_zone: f64) -> Option<(f64, f64, i32)> {
        let mut st = self.state.lock().ok()?;
        if let Some((dx, dy)) = st.engine.advance(transition, dead_zone) {
            Some((dx, dy, st.target_pid))
        } else {
            st.running = false;
            None
        }
    }

    /// After [`Self::frame`] returned `None`, re-check whether a concurrent
    /// [`Self::push`] re-armed the engine during parking. Returns `true` (and
    /// re-sets the run-flag) when there is fresh work, so the caller keeps the
    /// link running instead of stopping it; `false` when genuinely idle.
    pub fn rearm_if_pending(&self) -> bool {
        let Ok(mut st) = self.state.lock() else {
            return false;
        };
        if st.engine.is_idle() {
            false
        } else {
            st.running = true;
            true
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "exact-default comparisons are intentional"
)]
mod tests {
    use super::*;

    #[test]
    fn duration_transition_matches_mos_formula() {
        assert_eq!(duration_to_transition(5.2), 0.0);
        assert_eq!(duration_to_transition(0.0), 1.0);
        // Mos default 4.35 → 1 - sqrt(4.35/5.2) rounded to 3dp.
        assert_eq!(duration_to_transition(4.35), 0.085);
    }

    #[test]
    fn lerp_moves_a_fraction_toward_dest() {
        assert_eq!(lerp(0.0, 100.0, 0.1), 10.0);
        assert_eq!(lerp(50.0, 100.0, 0.5), 25.0);
        assert_eq!(lerp(100.0, 100.0, 0.5), 0.0);
    }

    #[test]
    fn normalize_raises_small_inputs_keeps_sign() {
        assert_eq!(normalize(0.0, 10.0), 0.0);
        assert_eq!(normalize(3.0, 10.0), 10.0);
        assert_eq!(normalize(-3.0, 10.0), -10.0);
        assert_eq!(normalize(50.0, 10.0), 50.0);
    }

    #[test]
    fn trackpad_detected_from_any_phase_field() {
        assert!(!is_trackpad(0.0, 0.0, 0));
        assert!(is_trackpad(1.0, 0.0, 0));
        assert!(is_trackpad(0.0, 2.0, 0));
        assert!(is_trackpad(0.0, 0.0, 1));
    }

    #[test]
    fn engine_converges_and_then_idles() {
        let mut e = SmoothEngine::new();
        e.add(0.0, 10.0, 1.0, 0.0); // buffer.1 = 10
        let mut emitted = 0.0;
        let mut frames = 0;
        while let Some((_, dy)) = e.advance(0.5, 0.5) {
            emitted += dy;
            frames += 1;
            assert!(frames < 100, "must converge");
        }
        assert!(e.is_idle());
        assert!(emitted > 8.0 && emitted <= 10.0, "emitted = {emitted}");
    }

    #[test]
    fn idle_engine_emits_nothing() {
        let mut e = SmoothEngine::new();
        assert!(e.advance(0.1, 1.0).is_none());
        assert!(e.is_idle());
    }

    #[test]
    fn shared_smooth_drains_to_none() {
        let s = SharedSmooth::new();
        assert!(s.push(0.0, 10.0, 1.0, 0.0, 1234)); // idle → caller starts link
        let mut got = 0.0;
        while let Some((_, dy, pid)) = s.frame(0.5, 0.5) {
            assert_eq!(pid, 1234);
            got += dy;
        }
        assert!(got > 8.0);
    }

    #[test]
    fn push_signals_start_only_when_idle() {
        let s = SharedSmooth::new();
        assert!(s.push(0.0, 10.0, 1.0, 0.0, 1)); // was idle → start
        assert!(!s.push(0.0, 10.0, 1.0, 0.0, 1)); // already running → no start
    }

    #[test]
    fn rearm_detects_a_raced_push() {
        let s = SharedSmooth::new();
        assert!(s.push(0.0, 10.0, 1.0, 0.0, 1));
        while s.frame(0.5, 0.5).is_some() {} // drain → frame cleared running
        assert!(!s.rearm_if_pending()); // genuinely idle
        assert!(s.push(0.0, 10.0, 1.0, 0.0, 1)); // fresh work arrives
        assert!(s.rearm_if_pending()); // re-armed
    }
}
