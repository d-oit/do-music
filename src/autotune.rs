//! Self-tuning render memory.
//!
//! After every render the pipeline measures what it actually produced —
//! how much the picture moved, how that compared to the music's own energy,
//! how varied the shot selection was — and appends a record to a small JSON
//! file. The next render reads those records back and nudges its planning.
//!
//! Deliberately *not* a database (see AGENTS.md): one append-only JSON file
//! under `do-music-output/`, capped at `MAX_RECORDS`, with every field
//! optional so an old or hand-edited file can never break a render.
//!
//! The loop it closes is narrow and measurable: motion energy. If renders
//! consistently come out more static than the music under them, the next
//! run raises its motion bias; if they come out busier, it lowers it. That
//! is a real signal computed from the output file, not a guess.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Records kept in the memory file; oldest are dropped past this.
pub const MAX_RECORDS: usize = 50;

/// The bias never leaves this range, so a pathological history cannot
/// produce a frozen or a frantic video.
pub const MIN_BIAS: f64 = 0.6;
pub const MAX_BIAS: f64 = 1.5;

/// Filename inside the output directory.
pub const MEMORY_FILE: &str = "render-memory.json";

/// One completed render, as measured from its own output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderRecord {
    /// Free-form tag for the kind of music (from the spec's genre list).
    #[serde(default)]
    pub genre: String,
    /// Mean musical energy 0..1 over the track.
    #[serde(default)]
    pub music_energy: f64,
    /// Mean visual motion 0..1 measured from the rendered video.
    #[serde(default)]
    pub visual_motion: f64,
    /// Distinct camera moves used, over total scenes.
    #[serde(default)]
    pub shot_variety: f64,
    /// Motion bias that produced this render.
    #[serde(default = "one")]
    pub bias_used: f64,
}

fn one() -> f64 {
    1.0
}

/// The persisted memory document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Memory {
    #[serde(default)]
    pub records: Vec<RenderRecord>,
}

impl Memory {
    /// Path of the memory file inside `dir`.
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(MEMORY_FILE)
    }

    /// Load memory, treating any unreadable/corrupt file as empty.
    ///
    /// A broken memory file must never block a render: the worst outcome of
    /// ignoring it is that this run uses neutral defaults.
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Append a record (trimming to `MAX_RECORDS`) and persist.
    pub fn append(&mut self, dir: &Path, record: RenderRecord) -> Result<()> {
        self.records.push(record);
        let excess = self.records.len().saturating_sub(MAX_RECORDS);
        if excess > 0 {
            self.records.drain(0..excess);
        }
        std::fs::create_dir_all(dir)?;
        std::fs::write(Self::path(dir), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Records relevant to `genre`, falling back to all of them.
    ///
    /// Matching by genre keeps an ambient history from dictating the pacing
    /// of a percussive track, but a brand-new genre still benefits from the
    /// general trend rather than starting blind.
    fn relevant(&self, genre: &str) -> Vec<&RenderRecord> {
        let g = genre.trim().to_lowercase();
        let matched: Vec<&RenderRecord> = self
            .records
            .iter()
            .filter(|r| !g.is_empty() && r.genre.trim().to_lowercase() == g)
            .collect();
        if matched.len() >= 3 {
            matched
        } else {
            self.records.iter().collect()
        }
    }

    /// Motion bias for the next render of `genre`.
    ///
    /// Compares measured visual motion against the music energy that should
    /// have driven it. A video that came out consistently flatter than its
    /// music gets a bias above 1.0; a busier one gets a bias below 1.0.
    /// Returns exactly 1.0 (neutral) until there is enough history to mean
    /// anything.
    pub fn motion_bias(&self, genre: &str) -> f64 {
        let records = self.relevant(genre);
        let usable: Vec<&&RenderRecord> = records
            .iter()
            .filter(|r| {
                r.music_energy.is_finite()
                    && r.visual_motion.is_finite()
                    && r.music_energy > 0.05
                    && r.visual_motion > 0.0
            })
            .collect();
        if usable.len() < 2 {
            return 1.0;
        }
        // Average ratio of intended (music) to achieved (visual) motion.
        let ratio: f64 = usable
            .iter()
            .map(|r| (r.music_energy / r.visual_motion).clamp(0.25, 4.0))
            .sum::<f64>()
            / usable.len() as f64;
        // Damp it: move only part-way toward the observed correction so the
        // bias converges instead of oscillating between renders.
        let damped = 1.0 + (ratio - 1.0) * 0.5;
        damped.clamp(MIN_BIAS, MAX_BIAS)
    }

    /// Summary line for the CLI.
    pub fn summary(&self, genre: &str) -> String {
        let bias = self.motion_bias(genre);
        if self.records.is_empty() {
            return "no render history yet — using neutral motion".to_string();
        }
        format!(
            "{} past renders; motion bias {bias:.2}{}",
            self.records.len(),
            if (bias - 1.0).abs() < 0.01 {
                " (neutral)"
            } else if bias > 1.0 {
                " (previous renders were flatter than the music)"
            } else {
                " (previous renders were busier than the music)"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn rec(genre: &str, music: f64, visual: f64) -> RenderRecord {
        RenderRecord {
            genre: genre.into(),
            music_energy: music,
            visual_motion: visual,
            shot_variety: 0.5,
            bias_used: 1.0,
        }
    }

    #[test]
    fn empty_memory_is_neutral() {
        let m = Memory::default();
        assert_eq!(m.motion_bias("ambient"), 1.0);
        assert!(m.summary("ambient").contains("no render history"));
    }

    #[test]
    fn a_single_record_is_not_enough_to_move_the_bias() {
        let mut m = Memory::default();
        m.records.push(rec("ambient", 0.8, 0.2));
        assert_eq!(m.motion_bias("ambient"), 1.0);
    }

    #[test]
    fn flat_renders_raise_the_bias_and_busy_renders_lower_it() {
        let mut flat = Memory::default();
        for _ in 0..4 {
            flat.records.push(rec("ambient", 0.8, 0.4)); // ratio 2.0
        }
        let b = flat.motion_bias("ambient");
        assert!(b > 1.0, "expected upward correction, got {b}");
        assert!(b <= MAX_BIAS);

        let mut busy = Memory::default();
        for _ in 0..4 {
            busy.records.push(rec("ambient", 0.3, 0.9)); // ratio 0.33
        }
        let b = busy.motion_bias("ambient");
        assert!(b < 1.0, "expected downward correction, got {b}");
        assert!(b >= MIN_BIAS);
    }

    #[test]
    fn bias_is_always_clamped_even_for_absurd_history() {
        let mut m = Memory::default();
        for _ in 0..10 {
            m.records.push(rec("x", 1.0, 0.0001));
        }
        let b = m.motion_bias("x");
        assert!((MIN_BIAS..=MAX_BIAS).contains(&b), "unclamped: {b}");
    }

    #[test]
    fn degenerate_records_are_ignored_rather_than_poisoning_the_bias() {
        let mut m = Memory::default();
        m.records.push(rec("x", f64::NAN, 0.5));
        m.records.push(rec("x", 0.5, 0.0));
        m.records.push(rec("x", 0.0, 0.5));
        assert_eq!(m.motion_bias("x"), 1.0);
    }

    #[test]
    fn genre_history_wins_when_there_is_enough_of_it() {
        let mut m = Memory::default();
        // Lots of busy techno history...
        for _ in 0..5 {
            m.records.push(rec("techno", 0.3, 0.9));
        }
        // ...must not drag ambient down when ambient has its own record.
        for _ in 0..3 {
            m.records.push(rec("ambient", 0.9, 0.45));
        }
        assert!(m.motion_bias("ambient") > 1.0);
        assert!(m.motion_bias("techno") < 1.0);
    }

    #[test]
    fn roundtrip_persists_and_caps_records() {
        let dir = tempdir().unwrap();
        let mut m = Memory::default();
        for i in 0..(MAX_RECORDS + 10) {
            m.append(dir.path(), rec("ambient", 0.5, 0.5 + i as f64 * 1e-6))
                .unwrap();
        }
        let loaded = Memory::load(dir.path());
        assert_eq!(loaded.records.len(), MAX_RECORDS);
        // The oldest were dropped, not the newest.
        assert!(loaded.records.last().unwrap().visual_motion > loaded.records[0].visual_motion);
    }

    #[test]
    fn corrupt_or_missing_memory_loads_as_empty() {
        let dir = tempdir().unwrap();
        assert!(Memory::load(dir.path()).records.is_empty());
        std::fs::write(Memory::path(dir.path()), "{not json").unwrap();
        assert!(Memory::load(dir.path()).records.is_empty());
        std::fs::write(Memory::path(dir.path()), r#"{"records":[{}]}"#).unwrap();
        let m = Memory::load(dir.path());
        assert_eq!(m.records.len(), 1);
        assert_eq!(m.motion_bias(""), 1.0);
    }
}
