//! Highlight-aware scene timing: snap scene boundaries to musical events.
//!
//! The highlight video used to cut on a metronome — every segment exactly
//! `total/n` seconds — so transitions landed mid-phrase as often as not.
//! The python side (`visualizer/highlights.py`) scores the track and emits
//! candidate marks; this module turns those marks into segment durations.
//!
//! Pure planning: parsing and boundary selection are offline-testable here,
//! the analysis subprocess lives in `commands.rs`.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::video::XFADE_SECONDS;

/// A detected musical moment worth cutting on.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Mark {
    /// Seconds from the start of the track.
    pub time: f64,
    /// Relative prominence in 0..1 (1 = strongest moment in the track).
    pub strength: f64,
}

/// The document emitted by `python3 -m visualizer.highlights`.
#[derive(Debug, Clone, Deserialize)]
pub struct Highlights {
    /// Track length in seconds.
    pub duration: f64,
    /// Candidate cut points, ascending by time.
    #[serde(default)]
    pub marks: Vec<Mark>,
    /// Mean musical energy over the track, 0..1.
    #[serde(default)]
    pub mean_energy: f64,
    /// Per-scene energy (one bucket per planned scene), each 0..1.
    #[serde(default)]
    pub scene_energy: Vec<f64>,
}

/// A segment must keep enough body to read as a scene rather than a flash.
///
/// Two xfades (in and out) plus a second of settled image is the floor; any
/// shorter and the crossfades overlap, which ffmpeg's `xfade` cannot express.
pub fn min_segment_seconds() -> f64 {
    2.0 * XFADE_SECONDS + 1.0
}

impl Highlights {
    /// Energy for scene `index`, falling back to the track mean and then to
    /// a calm default when the analyzer supplied no profile.
    pub fn energy_for(&self, index: usize) -> f64 {
        self.scene_energy
            .get(index)
            .copied()
            .filter(|v| v.is_finite())
            .or(Some(self.mean_energy).filter(|v| v.is_finite() && *v > 0.0))
            .unwrap_or(0.35)
            .clamp(0.0, 1.0)
    }

    /// Strength of the cut that opens scene `index` (0 for the first).
    pub fn cut_strength(&self, index: usize) -> f64 {
        if index == 0 {
            return 0.0;
        }
        self.marks
            .get(index - 1)
            .map(|m| m.strength)
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
    }
}

    /// Parse the python highlight document.
    pub fn parse(text: &str) -> Result<Self> {
        let mut doc: Self =
            serde_json::from_str(text).context("highlight analysis is not valid JSON")?;
        if !doc.duration.is_finite() || doc.duration <= 0.0 {
            anyhow::bail!("highlight analysis reported a non-positive duration");
        }
        doc.marks
            .retain(|m| m.time.is_finite() && m.time > 0.0 && m.time < doc.duration);
        doc.marks
            .sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
        Ok(doc)
    }

    /// Choose `scene_count - 1` cut points and return per-segment durations.
    ///
    /// Strategy: walk the uniform layout the planner would have used and, for
    /// each ideal boundary, snap to the strongest mark within `tolerance`
    /// seconds that still leaves every segment at least `min_segment_seconds`
    /// long. Boundaries with no usable mark keep their uniform position, so a
    /// track with only two detectable events still produces an even video
    /// with two musically-placed cuts.
    ///
    /// Durations always sum to exactly `total_seconds` plus the xfade overlap
    /// the chain consumes, so the final video still lands on the music.
    pub fn segment_durations(
        &self,
        scene_count: usize,
        total_seconds: f64,
        tolerance: f64,
    ) -> Vec<f64> {
        let boundaries = self.cut_points(scene_count, total_seconds, tolerance);
        boundaries_to_durations(&boundaries, total_seconds)
    }

    /// Absolute timeline positions of the `scene_count - 1` scene cuts.
    pub fn cut_points(
        &self,
        scene_count: usize,
        total_seconds: f64,
        tolerance: f64,
    ) -> Vec<f64> {
        if scene_count <= 1 {
            return Vec::new();
        }
        let n = scene_count as f64;
        let floor = min_segment_seconds();
        // Ideal spacing on the *chain* timeline: n segments overlapping by X
        // cover `total`, so consecutive cuts sit (total - X)/n apart. Using
        // total/n here would drift, leaving the last segment short.
        let step = (total_seconds - XFADE_SECONDS) / n;
        let mut chosen: Vec<f64> = Vec::with_capacity(scene_count - 1);
        for k in 1..scene_count {
            let ideal = k as f64 * step;
            // Each segment consumes `floor - X` of timeline beyond the cut
            // before it, and the tail after the last cut needs a full floor.
            let lower = chosen.last().copied().unwrap_or(0.0) + (floor - XFADE_SECONDS);
            let upper =
                total_seconds - floor - (scene_count - 1 - k) as f64 * (floor - XFADE_SECONDS);
            if lower > upper {
                chosen.push(ideal.clamp(0.0, total_seconds));
                continue;
            }
            let best = self
                .marks
                .iter()
                .filter(|m| (m.time - ideal).abs() <= tolerance)
                .filter(|m| m.time >= lower && m.time <= upper)
                .max_by(|a, b| {
                    // Prefer the strongest mark; break ties toward the ideal
                    // position so the pacing stays even.
                    let key = |m: &Mark| (m.strength, -(m.time - ideal).abs());
                    key(a)
                        .partial_cmp(&key(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|m| m.time)
                .unwrap_or_else(|| ideal.clamp(lower, upper));
            chosen.push(best);
        }
        chosen
    }
}

/// Convert absolute cut points into per-segment render durations.
///
/// Each segment spans from its cut point to the next, plus the crossfade it
/// must overlap with the following segment.
pub fn boundaries_to_durations(cuts: &[f64], total_seconds: f64) -> Vec<f64> {
    let mut durations = Vec::with_capacity(cuts.len() + 1);
    let mut prev = 0.0;
    for &cut in cuts {
        durations.push(cut - prev + XFADE_SECONDS);
        prev = cut;
    }
    durations.push(total_seconds - prev);
    durations
}

/// xfade offsets for segments of individually-planned lengths.
///
/// With uniform segments the offset is `k * (L - X)`; with variable lengths
/// each offset is the running sum of the preceding segments minus the
/// crossfades already consumed.
pub fn variable_xfade_offsets(durations: &[f64]) -> Vec<f64> {
    let mut offsets = Vec::with_capacity(durations.len().saturating_sub(1));
    let mut acc = 0.0;
    for d in durations.iter().take(durations.len().saturating_sub(1)) {
        acc += d - XFADE_SECONDS;
        offsets.push(acc);
    }
    offsets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(duration: f64, marks: &[(f64, f64)]) -> Highlights {
        Highlights {
            duration,
            marks: marks
                .iter()
                .map(|&(time, strength)| Mark { time, strength })
                .collect(),
            mean_energy: 0.0,
            scene_energy: Vec::new(),
        }
    }

    #[test]
    fn parse_sorts_and_drops_out_of_range_marks() {
        let h = Highlights::parse(
            r#"{"duration": 60, "marks": [{"time": 40, "strength": 1.0},
                {"time": -3, "strength": 0.5}, {"time": 90, "strength": 0.9},
                {"time": 20, "strength": 0.4}]}"#,
        )
        .unwrap();
        assert_eq!(h.marks.len(), 2);
        assert_eq!(h.marks[0].time, 20.0);
        assert_eq!(h.marks[1].time, 40.0);
    }

    #[test]
    fn energy_falls_back_through_profile_then_mean_then_default() {
        let mut h = doc(60.0, &[]);
        assert!((h.energy_for(0) - 0.35).abs() < 1e-9, "default");
        h.mean_energy = 0.6;
        assert!((h.energy_for(3) - 0.6).abs() < 1e-9, "track mean");
        h.scene_energy = vec![0.1, 0.9];
        assert!((h.energy_for(1) - 0.9).abs() < 1e-9, "per-scene");
        assert!((h.energy_for(5) - 0.6).abs() < 1e-9, "past the profile");
    }

    #[test]
    fn cut_strength_is_zero_for_the_opening_scene() {
        let h = doc(60.0, &[(20.0, 0.4), (40.0, 1.0)]);
        assert_eq!(h.cut_strength(0), 0.0);
        assert!((h.cut_strength(1) - 0.4).abs() < 1e-9);
        assert_eq!(h.cut_strength(99), 0.0);
    }

    #[test]
    fn parse_rejects_garbage_and_nonpositive_duration() {
        assert!(Highlights::parse("not json").is_err());
        assert!(Highlights::parse(r#"{"duration": 0, "marks": []}"#).is_err());
    }

    #[test]
    fn missing_marks_reproduce_the_uniform_layout() {
        let h = doc(600.0, &[]);
        let d = h.segment_durations(10, 600.0, 8.0);
        assert_eq!(d.len(), 10);
        // Uniform 10-scene/600 s layout is the classic 61.8 s segment.
        for x in &d {
            assert!((x - 61.8).abs() < 1e-6, "got {x}");
        }
    }

    #[test]
    fn boundaries_snap_to_nearby_marks() {
        // Ideal cuts for 4 scenes over 120 s: 29.5, 59, 88.5.
        let h = doc(120.0, &[(33.0, 0.9), (58.0, 0.8), (95.0, 1.0)]);
        let cuts = h.cut_points(4, 120.0, 8.0);
        assert_eq!(cuts, vec![33.0, 58.0, 95.0]);
    }

    #[test]
    fn far_away_marks_are_ignored() {
        // The only mark sits far from every ideal boundary.
        let h = doc(120.0, &[(5.0, 1.0)]);
        let cuts = h.cut_points(4, 120.0, 8.0);
        assert_eq!(cuts, vec![29.5, 59.0, 88.5]);
    }

    #[test]
    fn strongest_mark_wins_within_tolerance() {
        // Single ideal cut for 2 scenes over 120 s is 59 s.
        let h = doc(120.0, &[(57.0, 0.3), (62.0, 0.95)]);
        let cuts = h.cut_points(2, 120.0, 10.0);
        assert_eq!(cuts, vec![62.0]);
    }

    #[test]
    fn durations_always_sum_back_to_the_track_length() {
        let h = doc(600.0, &[(55.0, 0.9), (130.0, 1.0), (300.0, 0.7)]);
        for scenes in [1usize, 2, 5, 10] {
            let d = h.segment_durations(scenes, 600.0, 12.0);
            assert_eq!(d.len(), scenes);
            // n segments overlapping by X must cover exactly the total.
            let covered: f64 =
                d.iter().sum::<f64>() - (scenes.saturating_sub(1)) as f64 * XFADE_SECONDS;
            assert!(
                (covered - 600.0).abs() < 1e-6,
                "{scenes} scenes covered {covered}"
            );
        }
    }

    #[test]
    fn every_segment_stays_long_enough_to_crossfade() {
        // Marks clustered at the start must not starve the later segments.
        let h = doc(60.0, &[(1.0, 1.0), (2.0, 0.9), (3.0, 0.95), (4.0, 0.8)]);
        let d = h.segment_durations(6, 60.0, 30.0);
        for x in &d {
            assert!(*x >= min_segment_seconds() - 1e-9, "segment too short: {x}");
        }
    }

    #[test]
    fn variable_offsets_match_uniform_math_for_equal_segments() {
        let durations = vec![61.8; 10];
        let offsets = variable_xfade_offsets(&durations);
        assert_eq!(offsets.len(), 9);
        assert!((offsets[0] - 59.8).abs() < 1e-9);
        assert!((offsets[8] - 538.2).abs() < 1e-9);
    }

    #[test]
    fn variable_offsets_track_uneven_segments() {
        let durations = vec![30.0, 50.0, 20.0];
        let offsets = variable_xfade_offsets(&durations);
        assert_eq!(offsets.len(), 2);
        assert!((offsets[0] - 28.0).abs() < 1e-9);
        assert!((offsets[1] - 76.0).abs() < 1e-9);
    }

    #[test]
    fn single_scene_has_no_cuts() {
        let h = doc(60.0, &[(30.0, 1.0)]);
        assert!(h.cut_points(1, 60.0, 10.0).is_empty());
        assert_eq!(h.segment_durations(1, 60.0, 10.0), vec![60.0]);
    }
}
