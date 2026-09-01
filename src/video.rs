//! Video planning for highlight meditation videos.
//!
//! Pure planning (`plan_video_segments`, xfade offset math, scene prompts,
//! Pollinations URL construction, scene-image acquisition, H3 request
//! envelope) is offline-testable here; FFmpeg execution lives in
//! `src/render.rs`, HTTP transports in `src/providers.rs`.
//!
//! Layer rules: no music-planning rules here; `plan.rs`/`assembly.rs` unchanged.

use crate::api::find_string;
use crate::providers;
use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Frames per second of every rendered segment and the final video.
pub const VIDEO_FPS: u32 = 30;
/// Output canvas of the final video (Pollinations stills are upscaled to this).
pub const OUTPUT_WIDTH: u32 = 1920;
pub const OUTPUT_HEIGHT: u32 = 1080;
/// Crossfade duration between consecutive scenes, in seconds.
pub const XFADE_SECONDS: f64 = 2.0;
/// Length of one rendered scene segment, in seconds.
///
/// With 10 scenes: `10 * 61.8 - 9 * 2 = 600.0` seconds exactly.
pub const SEGMENT_SECONDS: f64 = 61.8;

/// Motion intensity for AI image-to-video; meditation requires stillness.
pub const MOTION_STRENGTH: f32 = 0.15;

/// One planned scene of the final video.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSegment {
    /// Source image for this scene (upscaled at render time).
    pub image: PathBuf,
    /// Per-scene motion prompt (I2V) or embedded in render args (fallback).
    pub motion_prompt: String,
    /// Which animation strategy renders this segment.
    pub animation: AnimationKind,
    /// Segment duration in seconds (before xfade overlap is accounted).
    pub duration: f64,
}

/// Scenes needed for a requested total: 10 scenes cover 600 s (the highlight
/// format); other totals scale linearly from the canonical 10×61.8 s layout.
pub fn plan_scene_count(total_seconds: u64) -> usize {
    if total_seconds <= 600 {
        return 10;
    }
    ((total_seconds as f64) / (SEGMENT_SECONDS - XFADE_SECONDS)).ceil() as usize
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationKind {
    /// AI image-to-video (Hailuo I2V), looped to `duration`.
    Ai,
    /// FFmpeg breathing `zoompan` with cubic easing (fallback).
    Zoompan,
}

/// Scene list: dawn→midnight arc, matching the zen gouache visual identity.
pub const SCENE_PROMPTS: [&str; 10] = [
    "gentle mist drifting over still lake, soft sunrise light slowly brightening, perfect water reflection undisturbed",
    "subtle chest rise of meditating monk, faint water ripples expanding slowly, sun disc steady",
    "dragonflies drifting as tiny points of light, leaves swaying almost imperceptibly, monk breathing slowly",
    "lotus petals trembling faintly on still water, one small cloud crossing the sky very slowly",
    "light beams through leaves shimmering faintly, dust motes floating, monk breathing slowly",
    "sun lowering gradually, reflection on water stretching longer, first stars appearing faintly",
    "sun disc touching horizon behind misty hills, reeds swaying barely, reflected fire shimmering softly",
    "crescent moon rising very slowly, last violet light fading from the lake surface",
    "Milky Way very slowly rotating, paper lantern flame flickering faintly, lake mirror perfectly still",
    "faint breath of mist over the frozen mirror lake, lantern point glowing steadily, deep midnight calm",
];

/// A user-supplied scene list: one prompt per musical section, plus an
/// optional shared style suffix and negative prompt (both Pollinations).
#[derive(Debug, Clone, PartialEq)]
pub struct SceneList {
    /// One prompt per section, in play order.
    pub prompts: Vec<String>,
    /// Style suffix appended to every Pollinations prompt.
    pub style: Option<String>,
    /// Negative prompt passed to Pollinations.
    pub negative: Option<String>,
}

impl SceneList {
    /// Parse a scene list from JSON: `{"prompts": [...], "style": ...,
    /// "negative": ...}`. Tolerant of either `prompts` alone or bare arrays.
    pub fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text).context("scene list is not valid JSON")?;
        let prompts = match &value {
            Value::Array(items) => items.to_vec(),
            Value::Object(_) => value
                .get("prompts")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("scene list needs a \"prompts\" array"))?
                .clone(),
            _ => anyhow::bail!("scene list must be an array or an object with \"prompts\""),
        };
        let prompts: Vec<String> = prompts
            .into_iter()
            .map(|v| match v {
                Value::String(s) if !s.trim().is_empty() => Ok(s),
                _ => anyhow::bail!("every scene prompt must be a non-empty string"),
            })
            .collect::<Result<_>>()?;
        if prompts.is_empty() {
            anyhow::bail!("scene list needs at least one prompt");
        }
        let style = find_string(&value, &["style"]).filter(|s| !s.trim().is_empty());
        let negative = find_string(&value, &["negative"]).filter(|s| !s.trim().is_empty());
        Ok(Self {
            prompts,
            style,
            negative,
        })
    }
}

/// Plan `scene_count` video segments totalling `total_seconds` after xfade.
///
/// Segment length is derived so the xfade chain lands exactly on
/// `total_seconds`: `n·L - (n-1)·X = total` → `L = (total + (n-1)·X)/n`.
/// For the canonical 10 scenes/600 s this reproduces `SEGMENT_SECONDS`
/// (61.8 s) exactly. `prompts` supplies per-scene motion/visual prompts
/// (cycled if shorter than `scene_count`); `images` must contain at least
/// `scene_count` paths; extra paths are ignored.
pub fn plan_video_segments(
    images: &[PathBuf],
    total_seconds: u64,
    scene_count: usize,
    prompts: &[String],
) -> Result<Vec<VideoSegment>> {
    if scene_count == 0 {
        return Err(anyhow!("scene count must be positive"));
    }
    if prompts.is_empty() {
        return Err(anyhow!("need at least one scene prompt"));
    }
    if images.len() < scene_count {
        return Err(anyhow!(
            "need {} scene images, found {}",
            scene_count,
            images.len()
        ));
    }
    let seg_secs = segment_seconds(scene_count, total_seconds);
    Ok((0..scene_count)
        .map(|i| VideoSegment {
            image: images[i].clone(),
            motion_prompt: prompts[i % prompts.len()].clone(),
            animation: AnimationKind::Zoompan,
            duration: seg_secs,
        })
        .collect())
}

/// Per-segment length landing the n-scene xfade chain exactly on `total`:
/// `L = (total + (n-1)·X) / n`.
pub fn segment_seconds(n: usize, total: u64) -> f64 {
    (total as f64 + (n.saturating_sub(1)) as f64 * XFADE_SECONDS) / n as f64
}

/// Total video seconds reachable with `n` segments chained by xfades.
pub fn reachable_seconds(n: usize) -> f64 {
    n as f64 * SEGMENT_SECONDS - (n.saturating_sub(1)) as f64 * XFADE_SECONDS
}

/// xfade offsets for a chain of `n` segments: `X = k * (61.8 - 2)`.
pub fn xfade_offsets(n: usize) -> Vec<f64> {
    (1..n)
        .map(|k| k as f64 * (SEGMENT_SECONDS - XFADE_SECONDS))
        .collect()
}

/// Status document returned while polling a video generation request.
#[derive(Debug, Deserialize)]
pub struct VideoStatus {
    pub status: String,
    #[serde(default)]
    pub outcome: Option<Value>,
}

/// Parse a poll response: terminal URL when `status == success`.
pub fn parse_video_status(value: &Value) -> Option<String> {
    let status = value.get("status")?.as_str()?;
    if status != "success" {
        return None;
    }
    find_string(
        value,
        &[
            "video_url",
            "videoUrl",
            "url",
            "download_url",
            "downloadUrl",
        ],
    )
}

/// Percent-encode a URL path/query component (RFC 3986 unreserved kept).
///
/// std-only replacement for the `urlencoding` crate: prompt text reaches a
/// Pollinations GET URL and must survive spaces, commas, and non-ASCII.
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Pollinations scene still: deterministic URL with a retry seed ladder.
///
/// Free tier caps at 1024x576; quality comes from supersampling at render,
/// not from the source resolution. `attempt` shifts the seed so a bad image
/// can be regenerated deterministically. `style`/`negative` come from the
/// scene list; when absent the zen gouache defaults apply.
pub fn pollinations_url(
    scene_index: usize,
    attempt: usize,
    prompt: &str,
    style: Option<&str>,
    negative: Option<&str>,
) -> String {
    let style = style.unwrap_or(
        "minimalist zen gouache painting, soft diffused light, muted palette, vast negative space, painterly texture",
    );
    let negative = negative
        .unwrap_or("closeup face, distorted hands, extra limbs, text, watermark, deformed anatomy");
    let prompt = percent_encode(&format!("{prompt}, {style}"));
    let negative = percent_encode(negative);
    let seed = 1000 * (attempt + 1) + scene_index + 1;
    format!(
        "https://image.pollinations.ai/prompt/{prompt}?width=1024&height=576&nologo=true&quality=hd&model=flux&seed={seed}&negative_prompt={negative}"
    )
}

/// Per-scene H3 motion prompt: the scene description plus stillness cues.
pub fn h3_motion_prompt(scene_text: &str, motion_cue: Option<&str>) -> String {
    format!(
        "{scene_text}, {}",
        motion_cue.unwrap_or("extremely slow and subtle meditative motion, no camera movement")
    )
}

/// Ensure `count` scene images exist, downloading missing ones from
/// Pollinations with deterministic seeds.
///
/// Reuses an existing `scene-NN.png` when larger than 20 KiB **and** its
/// adjacent `.url` sidecar matches the current prompt/style/negative URL.
/// Otherwise walks the seed ladder (attempts 0..4), sleeping 20 s after HTTP
/// 429 and 5 s after other failures; all four attempts exhausted names the scene.
pub async fn ensure_scene_images(
    frames_dir: &Path,
    scenes: &SceneList,
    count: usize,
) -> Result<Vec<(PathBuf, String)>> {
    let mut result = Vec::new();
    for i in 0..count {
        let path = frames_dir.join(format!("scene-{:02}.png", i + 1));
        let url = pollinations_url(
            i,
            0,
            &scenes.prompts[i % scenes.prompts.len()],
            scenes.style.as_deref(),
            scenes.negative.as_deref(),
        );
        let cache_key = path.with_extension("url");
        // Scene filenames are positional; the sidecar prevents a changed
        // prompt/style from silently reusing an image from an older project.
        let reusable = tokio::fs::metadata(&path)
            .await
            .map(|m| m.len() > 20 * 1024)
            .unwrap_or(false)
            && tokio::fs::read_to_string(&cache_key)
                .await
                .map(|cached| cached.trim() == url)
                .unwrap_or(false);
        if reusable {
            result.push((path.clone(), url));
            continue;
        }
        let mut last_err = String::new();
        let mut fetched = false;
        for attempt in 0..4 {
            let url = pollinations_url(
                i,
                attempt,
                &scenes.prompts[i % scenes.prompts.len()],
                scenes.style.as_deref(),
                scenes.negative.as_deref(),
            );
            match providers::fetch_scene_image(&url, &path).await {
                Ok(_) => {
                    tokio::fs::write(&cache_key, &url).await?;
                    result.push((path.clone(), url));
                    fetched = true;
                    break;
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    let wait = if msg.contains("429") { 20 } else { 5 };
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    last_err = msg;
                }
            }
        }
        if !fetched {
            anyhow::bail!(
                "scene {}/{}: all Pollinations attempts failed ({last_err})",
                i + 1,
                count
            );
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imgs(n: usize) -> Vec<PathBuf> {
        (1..=n)
            .map(|i| PathBuf::from(format!("/tmp/scene-{i:02}.png")))
            .collect()
    }

    fn prompts(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("scene prompt {i}")).collect()
    }

    #[test]
    fn ten_scenes_plan_to_exactly_600_seconds() {
        let segs = plan_video_segments(&imgs(10), 600, 10, &prompts(10)).unwrap();
        assert_eq!(segs.len(), 10);
        assert!(
            segs.iter()
                .all(|s| (s.duration - 61.8).abs() < f64::EPSILON)
        );
        assert_eq!(reachable_seconds(10), 600.0);
    }

    #[test]
    fn segment_seconds_generalizes_beyond_ten_scenes() {
        // L = (total + (n-1)*X) / n
        assert!((segment_seconds(10, 600) - 61.8).abs() < 1e-9);
        assert!((segment_seconds(6, 180) - 190.0 / 6.0).abs() < 1e-9);
        assert!((segment_seconds(1, 60) - 60.0).abs() < 1e-9);
        // And the identity holds for the derived layout.
        for (n, total) in [(6, 180), (10, 600), (7, 240)] {
            let l = segment_seconds(n, total);
            let reachable = n as f64 * l - (n as f64 - 1.0) * XFADE_SECONDS;
            assert!((reachable - total as f64).abs() < 1e-6);
        }
    }

    #[test]
    fn prompts_cycle_when_shorter_than_scene_count() {
        let segs = plan_video_segments(&imgs(6), 180, 6, &prompts(2)).unwrap();
        assert_eq!(segs[0].motion_prompt, "scene prompt 0");
        assert_eq!(segs[1].motion_prompt, "scene prompt 1");
        assert_eq!(segs[2].motion_prompt, "scene prompt 0");
    }

    #[test]
    fn scene_count_covers_canonical_and_longer_totals() {
        assert_eq!(plan_scene_count(600), 10);
        assert_eq!(plan_scene_count(60), 10);
    }

    #[test]
    fn fallback_and_ai_paths_share_segment_count_and_duration() {
        let a = plan_video_segments(&imgs(10), 600, 10, &prompts(10)).unwrap();
        // The plan is strategy-agnostic; rendering chooses the kind.
        assert_eq!(a.len(), 10);
        assert_eq!(a[0].duration, a[9].duration);
    }

    #[test]
    fn xfade_offsets_match_plan_document() {
        let offsets = xfade_offsets(10);
        assert_eq!(offsets.len(), 9);
        assert!((offsets[8] - 538.2).abs() < 1e-9);
    }

    #[test]
    fn too_few_images_is_an_error() {
        assert!(plan_video_segments(&imgs(3), 600, 10, &prompts(10)).is_err());
    }

    #[test]
    fn zero_scenes_or_empty_prompts_is_an_error() {
        assert!(plan_video_segments(&imgs(10), 600, 0, &prompts(10)).is_err());
        assert!(plan_video_segments(&imgs(10), 600, 10, &[]).is_err());
    }

    #[test]
    fn scene_list_parses_object_array_and_bare_array() {
        let obj =
            SceneList::parse(r#"{"prompts":["a","b"],"style":"oil painting","negative":"text"}"#)
                .unwrap();
        assert_eq!(obj.prompts, vec!["a", "b"]);
        assert_eq!(obj.style.as_deref(), Some("oil painting"));
        assert_eq!(obj.negative.as_deref(), Some("text"));

        let bare = SceneList::parse(r#"["x","y"]"#).unwrap();
        assert_eq!(bare.prompts, vec!["x", "y"]);
        assert_eq!(bare.style, None);
    }

    #[test]
    fn scene_list_rejects_empty_blank_and_nonstring_prompts() {
        assert!(SceneList::parse(r#"{"prompts":[]}"#).is_err());
        assert!(SceneList::parse(r#"{"prompts":["  "]}"#).is_err());
        assert!(SceneList::parse(r#"{"prompts":[42]}"#).is_err());
        assert!(SceneList::parse("not json").is_err());
        assert!(SceneList::parse(r#"{"style":"x"}"#).is_err());
    }

    #[test]
    fn scene_list_blank_style_falls_back_to_default() {
        let obj = SceneList::parse(r#"{"prompts":["a"],"style":"  ","negative":""}"#).unwrap();
        assert_eq!(obj.style, None);
        assert_eq!(obj.negative, None);
    }

    #[test]
    fn pollinations_url_uses_scene_prompt_and_list_style() {
        let list = SceneList::parse(r#"{"prompts":["koi at night"],"style":"art deco"}"#).unwrap();
        let a = pollinations_url(0, 0, &list.prompts[0], list.style.as_deref(), None);
        assert!(a.starts_with("https://image.pollinations.ai/prompt/"));
        assert!(a.contains("koi%20at%20night"));
        assert!(a.contains("art%20deco"));
        assert!(a.contains("width=1024&height=576"));
        assert!(a.contains("seed=1001&"));
        // Defaults still apply when style/negative are absent.
        let b = pollinations_url(0, 0, "p", None, None);
        assert!(b.contains("gouache"));
        // attempt shifts the seed rung: 1000*(attempt+1) + scene + 1
        assert!(pollinations_url(0, 1, "p", None, None).contains("seed=2001&"));
        assert!(pollinations_url(4, 0, "p", None, None).contains("seed=1005&"));
    }

    #[test]
    fn h3_motion_prompt_uses_scene_text_and_custom_cue() {
        let p = h3_motion_prompt("aurora spiral", None);
        assert!(p.contains("extremely slow and subtle meditative motion"));
        assert!(p.starts_with("aurora spiral, "));
        let q = h3_motion_prompt("aurora spiral", Some("slow barrel roll"));
        assert!(q.contains("slow barrel roll"));
        assert!(!q.contains("meditative"));
    }
}
