//! Render quality tiers: one knob trading wall-clock time against fidelity.
//!
//! The video path spends nearly all of its time in two places: rendering
//! per-scene animation from a still, and encoding. Both scale with the
//! supersample resolution and the encoder preset, so both live here instead
//! of being hard-coded in `render.rs`/`encode.rs`.
//!
//! Pure data; no I/O, fully offline-testable.

use anyhow::{Result, bail};

/// Selectable render tiers; `balanced` is the default.
pub const QUALITY_CHOICES: &[&str] = &["fast", "balanced", "high"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// Preview-grade: no supersample, fastest encoder presets, no grain.
    Fast,
    /// Default: 2x supersample, fast intermediates, quality final encode.
    #[default]
    Balanced,
    /// Archive-grade: 3x supersample, slow presets, HEVC intermediates.
    High,
}

impl Quality {
    /// Parse a CLI tier name.
    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "fast" => Ok(Self::Fast),
            "balanced" => Ok(Self::Balanced),
            "high" => Ok(Self::High),
            other => bail!("unsupported quality {other:?} (choose from {QUALITY_CHOICES:?})"),
        }
    }

    /// Supersample factor applied to the 1920x1080 canvas before zoompan.
    ///
    /// Supersampling is what removes the pixel-snap jitter of zoompan, but
    /// it is quadratic in cost: the old fixed 3x (5760x3240) chain paid ~9x
    /// the scaler cost of the output canvas on every scene.
    pub fn supersample(self) -> u32 {
        match self {
            Self::Fast => 1,
            Self::Balanced => 2,
            Self::High => 3,
        }
    }

    /// Scaler used for the supersample step (visually irrelevant at 1x).
    pub fn scale_flags(self) -> &'static str {
        match self {
            Self::Fast => "bicubic",
            Self::Balanced | Self::High => "lanczos",
        }
    }

    /// Film grain strength; 0 disables the `noise` filter entirely.
    pub fn grain(self) -> u32 {
        match self {
            Self::Fast => 0,
            Self::Balanced => 5,
            Self::High => 6,
        }
    }

    /// Encoder for intermediate segments: `(codec, preset, crf)`.
    ///
    /// Intermediates are decoded exactly once by the xfade chain, so they
    /// only need to survive a single generation loss. x264 at a fast preset
    /// and a low CRF does that for a fraction of x265 `medium`'s cost.
    pub fn intermediate_encoder(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Fast => ("libx264", "ultrafast", "18"),
            Self::Balanced => ("libx264", "veryfast", "16"),
            Self::High => ("libx265", "medium", "18"),
        }
    }

    /// Preset for the final (delivery) encode.
    pub fn final_preset(self) -> &'static str {
        match self {
            Self::Fast => "veryfast",
            Self::Balanced => "medium",
            Self::High => "slow",
        }
    }

    /// Full ffmpeg args for an intermediate segment encode.
    pub fn intermediate_encode_args(self) -> Vec<String> {
        let (codec, preset, crf) = self.intermediate_encoder();
        [
            "-c:v", codec, "-preset", preset, "-crf", crf, "-pix_fmt", "yuv420p",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_every_advertised_choice() {
        for name in QUALITY_CHOICES {
            assert!(Quality::parse(name).is_ok(), "{name} not implemented");
        }
        assert!(Quality::parse("ultra").is_err());
    }

    #[test]
    fn supersample_cost_grows_with_tier_and_default_is_cheaper_than_old_3x() {
        assert_eq!(Quality::Fast.supersample(), 1);
        assert_eq!(Quality::Balanced.supersample(), 2);
        assert_eq!(Quality::High.supersample(), 3);
        // The previous hard-coded chain was always 3x (5760x3240).
        assert!(Quality::default().supersample() < 3);
    }

    #[test]
    fn intermediates_are_cheap_by_default_and_x265_only_at_high() {
        assert_eq!(Quality::Balanced.intermediate_encoder().0, "libx264");
        assert_eq!(Quality::Balanced.intermediate_encoder().1, "veryfast");
        assert_eq!(Quality::High.intermediate_encoder().0, "libx265");
        let args = Quality::Fast.intermediate_encode_args();
        assert!(args.contains(&"ultrafast".to_string()));
        assert!(args.contains(&"yuv420p".to_string()));
    }

    #[test]
    fn fast_tier_drops_grain_and_uses_cheap_scaler() {
        assert_eq!(Quality::Fast.grain(), 0);
        assert_eq!(Quality::Fast.scale_flags(), "bicubic");
        assert_eq!(Quality::High.scale_flags(), "lanczos");
        assert_eq!(Quality::Fast.final_preset(), "veryfast");
    }
}
