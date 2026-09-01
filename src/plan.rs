use serde::{Deserialize, Serialize};

use crate::duration::Duration;

/// Maximum length of a single provider track, in seconds. Longer requests must
/// be split into multiple tracks and assembled locally.
pub const MAX_TRACK_SECONDS: u64 = 240;

/// Below this many minutes a single track is acceptable.
pub const SINGLE_TRACK_MAX_MINUTES: u64 = 10;

/// Phase names used for long-form section prompts, in order (wraps at the end).
pub const SECTION_PHASES: [&str; 7] = [
    "opening",
    "settling",
    "deepening",
    "flowing",
    "reflection",
    "release",
    "closing",
];

/// Structured evaluation of a user's music idea, produced by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MusicSpec {
    pub intent: String,
    pub genre: Vec<String>,
    pub mood: Vec<String>,
    pub tempo: String,
    pub instruments: Vec<String>,
    pub vocals: String,
    pub energy: String,
    pub arrangement: String,
    pub style_prompt: String,
    pub lyrics: String,
    pub score: f32,
}

/// Split a requested duration into per-track lengths.
///
/// * < 10 minutes → one track of the full requested length.
/// * ≥ 10 minutes → chunks of at most `MAX_TRACK_SECONDS`, assembled locally.
///
/// The provider never accepts a duration parameter, so long-form is always
/// multiple generated tracks plus local assembly.
pub fn plan_tracks(duration: Duration) -> Vec<u64> {
    if duration.minutes() < SINGLE_TRACK_MAX_MINUTES {
        return vec![duration.seconds()];
    }
    let mut result = Vec::new();
    let mut left = duration.seconds();
    while left > 0 {
        let next = left.min(MAX_TRACK_SECONDS);
        result.push(next);
        left -= next;
    }
    result
}

/// Build the prompt for one section of a long-form piece.
///
/// Keeps the same musical palette, names the position, and ends in a way that
/// crossfades cleanly into the next section. For long-form (e.g. 60 m = 15
/// sections) the `SECTION_PHASES` arc is distributed proportionally across
/// the sections so every phase appears, rather than wrapping abruptly and
/// collapsing the tail to `closing`.
pub fn section_prompt(spec: &MusicSpec, index: usize, total: usize) -> String {
    let phase = if total <= SECTION_PHASES.len() {
        SECTION_PHASES[index.min(SECTION_PHASES.len() - 1)]
    } else {
        let ratio = index as f64 / total as f64;
        let phase_idx = (ratio * SECTION_PHASES.len() as f64) as usize;
        SECTION_PHASES[phase_idx.min(SECTION_PHASES.len() - 1)]
    };
    format!(
        "{}; maintain the same musical palette across the long-form work; section {}/{}; {} phase; smooth ending suitable for crossfade; {}",
        spec.style_prompt,
        index + 1,
        total,
        phase,
        spec.arrangement
    )
}
