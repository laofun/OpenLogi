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
/// Per-axis smoothing distance tuning. Horizontal corresponds to macOS scroll
/// axis 2 (thumb wheel); vertical corresponds to axis 1 (main wheel).
#[derive(Debug, Clone, Copy)]
pub struct AxisTuning {
    pub horizontal_speed: f64,
    pub vertical_speed: f64,
    pub horizontal_step: f64,
    pub vertical_step: f64,
}

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
    /// raw event deltas; they are normalized and scaled per axis.
    pub fn add(&mut self, dx: f64, dy: f64, tuning: AxisTuning) {
        self.buffer.0 += normalize(dx, tuning.horizontal_step) * tuning.horizontal_speed;
        self.buffer.1 += normalize(dy, tuning.vertical_step) * tuning.vertical_speed;
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
/// thread calls [`Self::frame_or_stop`]. All state lives under a single mutex so
/// a push that races a park cannot lose its wakeup.
#[derive(Debug, Default)]
pub struct SharedSmooth {
    state: Mutex<EngineState>,
}

impl SharedSmooth {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an input tick (already inverted) and remember its target PID. If this
    /// push arms a previously idle engine, `start` is invoked **while the lock is
    /// still held**, so the display-link start is ordered against a concurrent
    /// stop decision in [`Self::frame_or_stop`] and the two FFI calls can never
    /// reorder into a "stopped but running" wedge.
    pub fn push(&self, dx: f64, dy: f64, tuning: AxisTuning, pid: i32, start: impl FnOnce()) {
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        st.target_pid = pid;
        st.engine.add(dx, dy, tuning);
        if !st.running {
            st.running = true;
            start();
        }
    }

    /// Advance one frame. Returns `Some((dx, dy, pid))` to post this frame.
    /// When the run has settled, this clears the run-flag and invokes `stop`
    /// **while the lock is held**, then returns `None`. Holding the lock across
    /// the stop means a concurrent arming [`Self::push`] cannot start the link
    /// after this stop has been decided: the push either runs first (its work is
    /// seen by `advance`, so we don't settle) or runs strictly after the stop
    /// completes (and then restarts the link cleanly). This replaces the old
    /// `frame` + `rearm_if_pending` two-step, whose separate unlocked FFI could
    /// reorder into a permanently-stopped-but-running wedge.
    pub fn frame_or_stop(
        &self,
        transition: f64,
        dead_zone: f64,
        stop: impl FnOnce(),
    ) -> Option<(f64, f64, i32)> {
        let mut st = self.state.lock().ok()?;
        if let Some((dx, dy)) = st.engine.advance(transition, dead_zone) {
            Some((dx, dy, st.target_pid))
        } else {
            st.running = false;
            stop();
            None
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
    use std::cell::Cell;

    fn default_tuning() -> AxisTuning {
        AxisTuning {
            horizontal_speed: 1.0,
            vertical_speed: 1.0,
            horizontal_step: 0.0,
            vertical_step: 0.0,
        }
    }

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
        e.add(0.0, 10.0, default_tuning()); // buffer.1 = 10
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
    fn engine_scales_each_axis_with_its_own_speed() {
        let mut e = SmoothEngine::new();
        e.add(
            1.0,
            1.0,
            AxisTuning {
                horizontal_speed: 2.0,
                vertical_speed: 4.0,
                horizontal_step: 1.0,
                vertical_step: 1.0,
            },
        );
        let (dx, dy) = e.advance(1.0, 0.1).expect("frame");
        assert!((dx - 2.0).abs() < f64::EPSILON);
        assert!((dy - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn engine_normalizes_each_axis_with_its_own_step() {
        let mut e = SmoothEngine::new();
        e.add(
            0.5,
            0.5,
            AxisTuning {
                horizontal_speed: 1.0,
                vertical_speed: 1.0,
                horizontal_step: 3.0,
                vertical_step: 7.0,
            },
        );
        let (dx, dy) = e.advance(1.0, 0.1).expect("frame");
        assert!((dx - 3.0).abs() < f64::EPSILON);
        assert!((dy - 7.0).abs() < f64::EPSILON);
    }

    #[test]
    fn shared_smooth_drains_to_none() {
        let s = SharedSmooth::new();
        let started = Cell::new(false);
        s.push(0.0, 10.0, default_tuning(), 1234, || started.set(true));
        assert!(started.get(), "idle push must start the link");
        let mut got = 0.0;
        while let Some((_, dy, pid)) = s.frame_or_stop(0.5, 0.5, || {}) {
            assert_eq!(pid, 1234);
            got += dy;
        }
        assert!(got > 8.0);
    }

    #[test]
    fn push_starts_only_when_idle() {
        let s = SharedSmooth::new();
        let starts = Cell::new(0u32);
        s.push(0.0, 10.0, default_tuning(), 1, || starts.set(starts.get() + 1)); // idle → start
        s.push(0.0, 10.0, default_tuning(), 1, || starts.set(starts.get() + 1)); // running → no start
        assert_eq!(starts.get(), 1);
    }

    #[test]
    fn frame_or_stop_invokes_stop_when_settled() {
        let s = SharedSmooth::new();
        s.push(0.0, 10.0, default_tuning(), 1, || {});
        let stopped = Cell::new(false);
        while s.frame_or_stop(0.5, 0.5, || stopped.set(true)).is_some() {}
        assert!(stopped.get(), "settling must stop the link");
    }

    #[test]
    fn push_after_settle_restarts() {
        let s = SharedSmooth::new();
        s.push(0.0, 10.0, default_tuning(), 1, || {});
        while s.frame_or_stop(0.5, 0.5, || {}).is_some() {}
        let restarted = Cell::new(false);
        s.push(0.0, 10.0, default_tuning(), 1, || restarted.set(true)); // idle again → restart
        assert!(restarted.get(), "a push after settle must restart the link");
    }
}
