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
}
