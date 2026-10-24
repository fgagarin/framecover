//! Keeping the main window on screen when it grows after a video opens.
//!
//! All values are physical pixels. [`fit`] is pure so it can be tested;
//! [`work_area`] asks the OS where windows may go on the window's monitor.

/// A screen rectangle (`right`/`bottom` exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    fn width(self) -> i32 {
        self.right - self.left
    }

    fn height(self) -> i32 {
        self.bottom - self.top
    }
}

/// Where and how large the window should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// Client-area size.
    pub inner: (u32, u32),
    /// Top-left corner of the window including its frame.
    pub outer_position: (i32, i32),
}

/// Fit a window whose client area should become `desired_inner`, whose frame
/// adds `frame` (outer minus inner size) and whose frame's top-left corner is
/// at `outer_position`, into `work`: shrink it if it is larger than the work
/// area, then move it the least distance that brings it fully inside.
pub fn fit(
    desired_inner: (u32, u32),
    frame: (u32, u32),
    outer_position: (i32, i32),
    work: Rect,
) -> Placement {
    let max_inner =
        |space: i32, frame: u32| u32::try_from(space).unwrap_or(0).saturating_sub(frame);
    let inner = (
        desired_inner.0.min(max_inner(work.width(), frame.0)).max(1),
        desired_inner
            .1
            .min(max_inner(work.height(), frame.1))
            .max(1),
    );
    let place = |pos: i32, size: u32, start: i32, end: i32| {
        let size = i32::try_from(size).unwrap_or(i32::MAX);
        // Prefer keeping the top/left edge visible if it still can't fit.
        pos.min(end.saturating_sub(size)).max(start)
    };
    Placement {
        inner,
        outer_position: (
            place(outer_position.0, inner.0 + frame.0, work.left, work.right),
            place(outer_position.1, inner.1 + frame.1, work.top, work.bottom),
        ),
    }
}

/// Work area (screen minus taskbar) of the monitor nearest to `point`.
#[cfg(windows)]
pub fn work_area(point: (i32, i32)) -> Option<Rect> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };

    // SAFETY: plain Win32 calls with a correctly sized, initialised struct.
    unsafe {
        let monitor = MonitorFromPoint(
            POINT {
                x: point.0,
                y: point.1,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        let r = info.rcWork;
        Some(Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn window_that_fits_is_left_alone() {
        let p = fit((1280, 720), (16, 39), (100, 100), SCREEN);
        assert_eq!(p.inner, (1280, 720));
        assert_eq!(p.outer_position, (100, 100));
    }

    #[test]
    fn window_past_the_right_and_bottom_edges_moves_back() {
        let p = fit((1280, 720), (16, 39), (1000, 600), SCREEN);
        assert_eq!(p.inner, (1280, 720));
        assert_eq!(p.outer_position, (1920 - 1296, 1040 - 759));
    }

    #[test]
    fn window_left_of_or_above_the_screen_moves_in() {
        let p = fit((1280, 720), (16, 39), (-50, -20), SCREEN);
        assert_eq!(p.outer_position, (0, 0));
    }

    #[test]
    fn window_larger_than_the_screen_shrinks() {
        let small = Rect {
            left: 0,
            top: 0,
            right: 1366,
            bottom: 728,
        };
        let p = fit((1280 * 3 / 2, 720 * 3 / 2), (16, 39), (300, 200), small);
        assert_eq!(p.inner, (1366 - 16, 728 - 39));
        assert_eq!(p.outer_position, (0, 0));
    }

    #[test]
    fn secondary_monitor_with_offset_origin() {
        let right = Rect {
            left: 1920,
            top: -200,
            right: 3840,
            bottom: 880,
        };
        let p = fit((1280, 720), (16, 39), (3000, 500), right);
        assert_eq!(p.outer_position, (3840 - 1296, 880 - 759));
        let p = fit((1280, 720), (16, 39), (1800, -400), right);
        assert_eq!(p.outer_position, (1920, -200));
    }
}
