//! Art direction: turn a music idea into a visual brief.
//!
//! The builtin scene arc is a fixed ten-shot monk/lake story, so every video
//! looked the same regardless of the music. This module asks the LLM to act
//! as an art director — deriving a palette, a shot arc and motion notes from
//! the music prompt — and optionally grounds that in live web research.
//!
//! Layer rules: prompt construction and response parsing are pure and live
//! here; the HTTP transports stay in `providers.rs`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// System prompt for the art-direction call.
///
/// Constrained hard to JSON because the output feeds directly into image
/// prompts and the ffmpeg grade; prose would need brittle scraping.
pub const ART_SYSTEM_PROMPT: &str = "You are an art director for music videos. \
Given a piece of music, design its visuals. Return strict JSON with fields: \
palette:string (a short comma-separated colour/lighting description), \
style:string (a rendering style suffix usable in an image prompt, e.g. \
'minimalist gouache painting, soft diffused light, painterly texture'), \
negative:string (things the image generator must avoid), \
mood:string, \
energy:number (0..1, how much visual motion the music wants), \
scenes:string[] (one vivid image prompt per scene, forming a visual arc with \
a beginning, development and resolution; each under 220 characters, no camera \
directions, no text or watermarks). \
Match the music's culture and instrumentation. Return JSON only.";

/// System prompt used to distil web research into visual references.
pub const RESEARCH_SYSTEM_PROMPT: &str = "You summarise visual research for an art director. \
Given search results about a musical style, extract concrete visual language: \
recurring imagery, colour palettes, lighting, materials, cultural motifs. \
Return 4-8 short bullet phrases, plain text, one per line, no preamble, no URLs.";

/// A complete visual brief for one video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Brief {
    #[serde(default)]
    pub palette: String,
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub negative: String,
    #[serde(default)]
    pub mood: String,
    #[serde(default = "default_energy")]
    pub energy: f64,
    #[serde(default)]
    pub scenes: Vec<String>,
}

fn default_energy() -> f64 {
    0.35
}

impl Default for Brief {
    fn default() -> Self {
        Self {
            palette: String::new(),
            style: String::new(),
            negative: String::new(),
            mood: String::new(),
            energy: default_energy(),
            scenes: Vec::new(),
        }
    }
}

impl Brief {
    /// Parse an LLM art-direction response.
    ///
    /// Tolerant of the usual drift (markdown fences, a wrapper object) but
    /// fails loudly when no scene list can be found, because silently
    /// falling back would hide a broken art-direction call behind the
    /// builtin arc.
    pub fn parse(text: &str) -> Result<Self> {
        let cleaned = strip_code_fence(text);
        let value: serde_json::Value =
            serde_json::from_str(cleaned).context("art direction is not valid JSON")?;
        // Some models wrap the payload; accept one level of nesting.
        let value = value
            .get("brief")
            .or_else(|| value.get("art_direction"))
            .cloned()
            .unwrap_or(value);
        let mut brief: Brief =
            serde_json::from_value(value).context("art direction JSON has unexpected shape")?;
        brief.scenes.retain(|s| !s.trim().is_empty());
        if brief.scenes.is_empty() {
            anyhow::bail!("art direction returned no scenes");
        }
        if !brief.energy.is_finite() {
            brief.energy = default_energy();
        }
        brief.energy = brief.energy.clamp(0.0, 1.0);
        Ok(brief)
    }

    /// Convert to the scene-list shape the video planner already consumes.
    pub fn to_scene_list(&self) -> crate::video::SceneList {
        crate::video::SceneList {
            prompts: self.scenes.clone(),
            style: non_empty(&self.combined_style()),
            negative: non_empty(&self.negative),
        }
    }

    /// Style suffix for image prompts: rendering style plus palette.
    pub fn combined_style(&self) -> String {
        match (non_empty(&self.style), non_empty(&self.palette)) {
            (Some(s), Some(p)) => format!("{s}, {p}"),
            (Some(s), None) => s,
            (None, Some(p)) => p,
            (None, None) => String::new(),
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Strip a ```json fence if the model wrapped its answer in one.
pub fn strip_code_fence(text: &str) -> &str {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.trim_start_matches('\n')
        .trim_end_matches('`')
        .trim_end()
        .trim_start()
}

/// Build the user message for the art-direction call.
pub fn art_user_prompt(music_prompt: &str, scene_count: usize, research: Option<&str>) -> String {
    let mut out = format!(
        "Music: {music_prompt}\n\nDesign exactly {scene_count} scenes forming one visual arc."
    );
    if let Some(notes) = research.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str("\n\nVisual research to draw on:\n");
        out.push_str(notes);
    }
    out
}

/// Build the search query used for visual research on a music idea.
pub fn research_query(music_prompt: &str) -> String {
    // Keep it short: search engines do better with a focused phrase than
    // with a whole generation prompt.
    let words: Vec<&str> = music_prompt.split_whitespace().take(12).collect();
    format!(
        "{} music video visual style palette imagery",
        words.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "palette": "deep indigo, pale gold",
        "style": "minimalist gouache painting",
        "negative": "text, watermark",
        "mood": "contemplative",
        "energy": 0.3,
        "scenes": ["a still lake at dawn", "mist over reeds", "stars on water"]
    }"#;

    #[test]
    fn parses_a_full_brief() {
        let b = Brief::parse(SAMPLE).unwrap();
        assert_eq!(b.scenes.len(), 3);
        assert_eq!(b.palette, "deep indigo, pale gold");
        assert!((b.energy - 0.3).abs() < 1e-9);
    }

    #[test]
    fn tolerates_code_fences_and_wrapper_objects() {
        let fenced = format!("```json\n{SAMPLE}\n```");
        assert_eq!(Brief::parse(&fenced).unwrap().scenes.len(), 3);
        let wrapped = format!("{{\"brief\": {SAMPLE} }}");
        assert_eq!(Brief::parse(&wrapped).unwrap().scenes.len(), 3);
    }

    #[test]
    fn rejects_briefs_without_scenes() {
        assert!(Brief::parse(r#"{"palette":"blue","scenes":[]}"#).is_err());
        assert!(Brief::parse(r#"{"palette":"blue","scenes":["  "]}"#).is_err());
        assert!(Brief::parse("not json").is_err());
    }

    #[test]
    fn energy_is_clamped_and_defaulted() {
        let hot = Brief::parse(r#"{"energy": 9, "scenes":["a"]}"#).unwrap();
        assert_eq!(hot.energy, 1.0);
        let missing = Brief::parse(r#"{"scenes":["a"]}"#).unwrap();
        assert!((missing.energy - 0.35).abs() < 1e-9);
    }

    #[test]
    fn scene_list_carries_style_palette_and_negative() {
        let b = Brief::parse(SAMPLE).unwrap();
        let list = b.to_scene_list();
        assert_eq!(list.prompts.len(), 3);
        let style = list.style.unwrap();
        assert!(style.contains("gouache"), "{style}");
        assert!(style.contains("indigo"), "{style}");
        assert_eq!(list.negative.as_deref(), Some("text, watermark"));
    }

    #[test]
    fn blank_style_and_palette_produce_no_suffix() {
        let b = Brief::parse(r#"{"style":"  ","palette":"","scenes":["a"]}"#).unwrap();
        assert!(b.combined_style().is_empty());
        assert_eq!(b.to_scene_list().style, None);
    }

    #[test]
    fn user_prompt_states_the_scene_count_and_folds_in_research() {
        let p = art_user_prompt("calm piano", 7, Some("- soft focus\n- warm light"));
        assert!(p.contains("calm piano"));
        assert!(p.contains("exactly 7 scenes"));
        assert!(p.contains("soft focus"));
        let bare = art_user_prompt("calm piano", 7, Some("   "));
        assert!(!bare.contains("research"));
    }

    #[test]
    fn research_query_is_short_and_focused() {
        let long = "a ".repeat(200);
        let q = research_query(&long);
        assert!(q.split_whitespace().count() <= 20, "{q}");
        assert!(q.contains("visual style"));
    }
}
