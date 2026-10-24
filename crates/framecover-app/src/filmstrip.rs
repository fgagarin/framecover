//! Lazy, width-adaptive filmstrip model (port of `ui/filmstrip_widget.py`).
//!
//! Pure scheduling logic: it decides which frames to sample for the current
//! width, hands out at most [`MAX_CONCURRENT_EXTRACTIONS`] jobs at a time and
//! drops results from a previous video via a generation counter. The tile
//! image type is generic so tests can run without Slint.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use framecover_core::VideoFile;

pub const TARGET_TILE_WIDTH: f32 = 128.0;
pub const TILE_HEIGHT: f32 = 72.0;
pub const MAX_CONCURRENT_EXTRACTIONS: usize = 3;

/// One frame extraction to run off the UI thread.
#[derive(Debug, Clone)]
pub struct FilmJob {
    pub generation: u64,
    pub frame_index: u64,
    pub position_ms: u64,
    pub video: Arc<VideoFile>,
}

/// Geometry and content of one visible tile.
#[derive(Debug, Clone, PartialEq)]
pub struct TileView<'a, T> {
    pub x: f32,
    pub width: f32,
    pub position_ms: u64,
    pub selected: bool,
    pub image: Option<&'a T>,
}

#[derive(Debug)]
pub struct Filmstrip<T> {
    video: Option<Arc<VideoFile>>,
    generation: u64,
    frame_step_ms: u64,
    frame_count: u64,
    width: f32,
    tile_width: f32,
    /// `(frame index, position ms)` of each visible tile.
    positions: Vec<(u64, u64)>,
    selected_position_ms: u64,
    frames: HashMap<u64, T>,
    in_flight: HashSet<(u64, u64)>,
    queue: VecDeque<(u64, u64, u64)>,
}

impl<T> Default for Filmstrip<T> {
    fn default() -> Self {
        Self {
            video: None,
            generation: 0,
            frame_step_ms: 40,
            frame_count: 0,
            width: 0.0,
            tile_width: TARGET_TILE_WIDTH,
            positions: Vec::new(),
            selected_position_ms: 0,
            frames: HashMap::new(),
            in_flight: HashSet::new(),
            queue: VecDeque::new(),
        }
    }
}

impl<T> Filmstrip<T> {
    /// Show `video` (or nothing), discarding all previous samples.
    pub fn set_video(&mut self, video: Option<Arc<VideoFile>>) -> Vec<FilmJob> {
        self.generation += 1;
        self.frames.clear();
        self.queue.clear();
        self.positions.clear();
        self.selected_position_ms = 0;
        self.video = video;
        let Some(video) = &self.video else {
            return Vec::new();
        };
        self.frame_step_ms = video.frame_step_ms.max(1);
        self.frame_count = (video.duration_ms.div_ceil(self.frame_step_ms) + 1).max(1);
        self.update_samples()
    }

    /// The strip was resized to `width` logical pixels.
    pub fn set_width(&mut self, width: f32) -> Vec<FilmJob> {
        if (width - self.width).abs() < 0.5 {
            return Vec::new();
        }
        self.width = width.max(0.0);
        self.update_samples()
    }

    pub fn set_selected_position(&mut self, position_ms: u64) {
        self.selected_position_ms = position_ms;
    }

    /// A worker finished. Returns the follow-up jobs that may now start.
    /// Returns `true` in the second slot when the visible content changed.
    pub fn on_result(
        &mut self,
        generation: u64,
        frame_index: u64,
        image: Option<T>,
    ) -> (Vec<FilmJob>, bool) {
        self.in_flight.remove(&(generation, frame_index));
        let mut changed = false;
        if generation == self.generation
            && let Some(image) = image
        {
            self.frames.insert(frame_index, image);
            changed = true;
        }
        (self.dispatch_queue(), changed)
    }

    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }

    #[cfg(test)]
    pub fn positions(&self) -> &[(u64, u64)] {
        &self.positions
    }

    #[cfg(test)]
    pub fn loaded_frame_indices(&self) -> HashSet<u64> {
        self.frames.keys().copied().collect()
    }

    /// Timeline position of the tile under `x`, if any.
    pub fn position_at(&self, x: f32) -> Option<u64> {
        if self.positions.is_empty() {
            return None;
        }
        let index = ((x.max(0.0) / self.tile_width) as usize).min(self.positions.len() - 1);
        Some(self.positions[index].1)
    }

    /// `(x, width)` of tile `index`, with integer edges so tiles abut exactly.
    pub fn tile_rect(&self, index: usize) -> (f32, f32) {
        let left = (index as f32 * self.tile_width).round();
        let right = ((index + 1) as f32 * self.tile_width).round();
        (left, (right - left).max(1.0))
    }

    pub fn selected_tile_index(&self) -> Option<usize> {
        (0..self.positions.len())
            .min_by_key(|&i| self.positions[i].1.abs_diff(self.selected_position_ms))
    }

    pub fn tiles(&self) -> Vec<TileView<'_, T>> {
        let selected = self.selected_tile_index();
        self.positions
            .iter()
            .enumerate()
            .map(|(index, &(frame_index, position_ms))| {
                let (x, width) = self.tile_rect(index);
                TileView {
                    x,
                    width,
                    position_ms,
                    selected: Some(index) == selected,
                    image: self.frames.get(&frame_index),
                }
            })
            .collect()
    }

    fn update_samples(&mut self) -> Vec<FilmJob> {
        let Some(video) = &self.video else {
            return Vec::new();
        };
        if self.width < 1.0 {
            return Vec::new();
        }
        let tile_count = ((self.width / TARGET_TILE_WIDTH).round() as u64)
            .max(1)
            .min(self.frame_count);
        self.tile_width = (self.width / tile_count as f32).max(1.0);
        let frame_indices: Vec<u64> = if tile_count == 1 {
            vec![self.frame_count / 2]
        } else {
            let last = (self.frame_count - 1) as f64;
            (0..tile_count)
                .map(|i| (i as f64 * last / (tile_count - 1) as f64).round() as u64)
                .collect()
        };
        let duration = video.duration_ms;
        self.positions = frame_indices
            .into_iter()
            .map(|index| (index, duration.min(index * self.frame_step_ms)))
            .collect();
        let generation = self.generation;
        self.queue = self
            .positions
            .iter()
            .filter(|(index, _)| {
                !self.frames.contains_key(index) && !self.in_flight.contains(&(generation, *index))
            })
            .map(|&(index, ms)| (generation, index, ms))
            .collect();
        self.dispatch_queue()
    }

    fn dispatch_queue(&mut self) -> Vec<FilmJob> {
        let mut jobs = Vec::new();
        while self.in_flight.len() < MAX_CONCURRENT_EXTRACTIONS {
            let Some((generation, frame_index, position_ms)) = self.queue.pop_front() else {
                break;
            };
            let key = (generation, frame_index);
            if generation != self.generation || self.in_flight.contains(&key) {
                continue;
            }
            let Some(video) = &self.video else {
                break;
            };
            self.in_flight.insert(key);
            jobs.push(FilmJob {
                generation,
                frame_index,
                position_ms,
                video: Arc::clone(video),
            });
        }
        jobs
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use framecover_core::VideoFormat;
    use std::path::PathBuf;

    pub(crate) fn video(duration_ms: u64) -> VideoFile {
        VideoFile {
            path: PathBuf::from("/fake/video.mp4"),
            format: VideoFormat::Mp4,
            duration_ms,
            width: 1920,
            height: 1080,
            existing_thumbnail: None,
            is_writable: true,
            frame_step_ms: 40,
            thumbnail_frame_number: None,
            thumbnail_position_ms: None,
        }
    }

    fn strip(width: f32) -> (Filmstrip<u32>, Vec<FilmJob>) {
        let mut f = Filmstrip::default();
        f.set_width(width);
        let jobs = f.set_video(Some(Arc::new(video(60_000))));
        (f, jobs)
    }

    /// Run all jobs to completion, like the worker pool would.
    fn drain(f: &mut Filmstrip<u32>, mut jobs: Vec<FilmJob>) -> usize {
        let mut runs = 0;
        while let Some(job) = jobs.pop() {
            runs += 1;
            assert!(f.in_flight_count() <= MAX_CONCURRENT_EXTRACTIONS);
            let (more, _) = f.on_result(job.generation, job.frame_index, Some(1));
            jobs.extend(more);
        }
        runs
    }

    #[test]
    fn samples_cover_video_duration_evenly() {
        let (f, _) = strip(700.0);
        let samples: Vec<u64> = f.positions().iter().map(|p| p.1).collect();
        assert_eq!(samples.len(), (700 + 4) / 132);
        assert_eq!(samples[0], 0);
        assert_eq!(*samples.last().unwrap(), 60_000);
        assert!(samples.windows(2).all(|w| w[0] <= w[1]));

        let rects: Vec<_> = (0..samples.len()).map(|i| f.tile_rect(i)).collect();
        assert_eq!(rects[0].0, 0.0);
        let last = rects.last().unwrap();
        assert_eq!(last.0 + last.1, 700.0);
        assert!(rects.windows(2).all(|w| w[1].0 == w[0].0 + w[0].1));
    }

    #[test]
    fn resize_changes_visible_sample_count() {
        let (mut f, _) = strip(700.0);
        let wide = f.positions().len();
        f.set_width(400.0);
        assert!(f.positions().len() < wide);
    }

    #[test]
    fn click_returns_the_tile_timestamp() {
        let (f, _) = strip(700.0);
        assert_eq!(f.position_at(140.0 + 10.0), Some(f.positions()[1].1));
        assert_eq!(f.position_at(10_000.0), Some(60_000));
    }

    #[test]
    fn requests_are_bounded_and_fill_as_ready() {
        let (mut f, jobs) = strip(700.0);
        assert_eq!(jobs.len(), MAX_CONCURRENT_EXTRACTIONS);
        let runs = drain(&mut f, jobs);
        assert_eq!(runs, f.positions().len());
        assert_eq!(f.loaded_frame_indices().len(), f.positions().len());
    }

    #[test]
    fn resize_requests_new_samples_lazily() {
        let (mut f, jobs) = strip(700.0);
        drain(&mut f, jobs);
        let initial = f.loaded_frame_indices();
        let jobs = f.set_width(1200.0);
        assert!(f.positions().len() > initial.len());
        assert!(jobs.iter().all(|j| !initial.contains(&j.frame_index)));
        drain(&mut f, jobs);
        let loaded = f.loaded_frame_indices();
        assert!(f.positions().iter().all(|(i, _)| loaded.contains(i)));
        assert!(loaded.is_superset(&initial) && loaded.len() > initial.len());
    }

    #[test]
    fn stale_generation_results_are_dropped() {
        let (mut f, jobs) = strip(700.0);
        let old = jobs[0].clone();
        let new_jobs = f.set_video(Some(Arc::new(video(30_000))));
        // Old jobs still count against the concurrency limit.
        assert!(new_jobs.is_empty());
        let (next, changed) = f.on_result(old.generation, old.frame_index, Some(7));
        assert!(!changed);
        assert!(f.loaded_frame_indices().is_empty());
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].generation, f.generation());
    }

    #[test]
    fn selected_tile_is_nearest_to_position() {
        let (mut f, _) = strip(700.0);
        f.set_selected_position(31_000);
        let tiles = f.tiles();
        let selected: Vec<_> = tiles.iter().filter(|t| t.selected).collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].position_ms, 30_000);
    }

    #[test]
    fn no_video_has_no_tiles() {
        let mut f: Filmstrip<u32> = Filmstrip::default();
        assert!(f.set_width(500.0).is_empty());
        assert!(f.set_video(None).is_empty());
        assert_eq!(f.position_at(5.0), None);
    }
}
