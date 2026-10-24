//! Timeline scrubber model (port of the logic in `ui/timeline_widget.py`).
//!
//! The Slint `Timeline` component only draws and reports pointer ratios; the
//! position state, clamping and labels live here.

/// Duration and current position of the scrubber, in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timeline {
    duration_ms: u64,
    position_ms: u64,
}

impl Timeline {
    /// Set the total duration and reset the position to 0.
    pub fn set_duration(&mut self, ms: u64) {
        self.duration_ms = ms;
        self.position_ms = 0;
    }

    /// Move the handle to `ms`, clamped to `[0, duration]`.
    pub fn set_position(&mut self, ms: u64) {
        self.position_ms = ms.min(self.duration_ms);
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    pub fn position_ms(&self) -> u64 {
        self.position_ms
    }

    /// Position for a pointer at `ratio` (0..=1) of the track width; `None`
    /// when the timeline is empty.
    pub fn position_for_ratio(&self, ratio: f64) -> Option<u64> {
        if self.duration_ms == 0 || !ratio.is_finite() {
            return None;
        }
        Some((ratio.clamp(0.0, 1.0) * self.duration_ms as f64) as u64)
    }

    /// Fraction of the track that is filled.
    pub fn fill_ratio(&self) -> f32 {
        if self.duration_ms == 0 {
            0.0
        } else {
            (self.position_ms as f64 / self.duration_ms as f64) as f32
        }
    }

    /// `"M:SS / M:SS"` label under the track.
    pub fn label(&self) -> String {
        format!(
            "{} / {}",
            format_mss(self.position_ms),
            format_mss(self.duration_ms)
        )
    }
}

/// `M:SS` (minutes unbounded).
pub fn format_mss(ms: u64) -> String {
    let total = ms / 1000;
    format!("{}:{:02}", total / 60, total % 60)
}

/// `M:SS.mmm`, as shown in the frame panel title.
pub fn format_position(ms: u64) -> String {
    let (total, millis) = (ms / 1000, ms % 1000);
    format!("{}:{:02}.{millis:03}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state() {
        let t = Timeline::default();
        assert_eq!(t.duration_ms(), 0);
        assert_eq!(t.position_ms(), 0);
        assert_eq!(t.label(), "0:00 / 0:00");
    }

    #[test]
    fn set_duration_resets_position() {
        let mut t = Timeline::default();
        t.set_duration(10_000);
        t.set_position(5_000);
        t.set_duration(20_000);
        assert_eq!(t.position_ms(), 0);
        assert_eq!(t.duration_ms(), 20_000);
    }

    #[test]
    fn set_position_clamps_to_duration() {
        let mut t = Timeline::default();
        t.set_duration(10_000);
        t.set_position(99_999);
        assert_eq!(t.position_ms(), 10_000);
        t.set_position(5_000);
        assert_eq!(t.position_ms(), 5_000);
        assert!((t.fill_ratio() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn pointer_ratio_maps_to_position_and_clamps() {
        let mut t = Timeline::default();
        t.set_duration(60_000);
        assert_eq!(t.position_for_ratio(0.25), Some(15_000));
        assert_eq!(t.position_for_ratio(-1.0), Some(0));
        assert_eq!(t.position_for_ratio(2.0), Some(60_000));
    }

    #[test]
    fn no_position_when_duration_zero() {
        assert_eq!(Timeline::default().position_for_ratio(0.5), None);
    }

    #[test]
    fn labels() {
        let mut t = Timeline::default();
        t.set_duration(63_500);
        t.set_position(17_000);
        assert_eq!(t.label(), "0:17 / 1:03");
        assert_eq!(format_position(17_000), "0:17.000");
        assert_eq!(format_position(3_723_045), "62:03.045");
    }
}
