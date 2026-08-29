use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::plan::MusicSpec;

/// LLM chat-completions request (prompt-evaluator path).
#[derive(Debug, Serialize)]
pub struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<ChatMessage<'a>>,
    pub temperature: f32,
    pub response_format: ResponseFormat,
}

#[derive(Debug, Serialize)]
pub struct ChatMessage<'a> {
    pub role: &'a str,
    pub content: &'a str,
}

#[derive(Debug, Serialize)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

impl ResponseFormat {
    pub fn json_object() -> Self {
        Self {
            kind: "json_object",
        }
    }

    /// Plain-text mode for the `optimize` rewrite (no JSON envelope).
    pub fn text() -> Self {
        Self { kind: "text" }
    }
}

/// Music generation request envelope for MiniMax Music 3.0.
///
/// Note: there is deliberately **no duration field** — the provider never
/// accepts one. Long-form is multiple tracks plus local assembly.
#[derive(Debug, Serialize)]
pub struct MusicRequest<'a> {
    pub model: &'a str,
    pub payload: MusicPayload<'a>,
}

#[derive(Debug, Serialize)]
pub struct MusicPayload<'a> {
    pub lyrics: &'a str,
    pub prompt: &'a str,
    pub sample_rate: u32,
    pub bitrate: u32,
    pub format: &'a str,
}

/// Build the music request body for one track with the standard codec settings.
pub fn music_request<'a>(model: &'a str, lyrics: &'a str, prompt: &'a str) -> MusicRequest<'a> {
    MusicRequest {
        model,
        payload: MusicPayload {
            lyrics,
            prompt,
            sample_rate: 44_100,
            bitrate: 256_000,
            format: "mp3",
        },
    }
}

/// LLM chat-completions response envelope (parse-only).
#[derive(Debug, Deserialize)]
pub struct LlmResponse {
    pub choices: Vec<LlmChoice>,
}

#[derive(Debug, Deserialize)]
pub struct LlmChoice {
    pub message: LlmMessage,
}

#[derive(Debug, Deserialize)]
pub struct LlmMessage {
    pub content: String,
}

/// Parse LLM content into a `MusicSpec` (pure; no network involved).
///
/// Tolerates markdown code fences: models sometimes wrap JSON in
/// ` ```json ... ``` ` despite `response_format: json_object`, so a leading
/// fence line and a trailing fence line are stripped before parsing.
pub fn parse_spec(content: &str) -> Result<MusicSpec> {
    let trimmed = content.trim();
    let without_leading_fence = match trimmed.strip_prefix("```") {
        Some(rest) => rest
            .trim_start_matches(['j', 's', 'o', 'n', '\n', '\r'])
            .trim_start(),
        None => trimmed,
    };
    let without_trailing_fence = match without_leading_fence.strip_suffix("```") {
        Some(rest) => rest.trim_end(),
        None => without_leading_fence,
    };
    serde_json::from_str(without_trailing_fence).context("LLM returned invalid MusicSpec JSON")
}
#[derive(Debug, Serialize)]
pub struct H3Request<'a> {
    pub model: &'a str,
    pub payload: H3Payload<'a>,
}

/// MiniMax-H3 payload; key names match the documented quickstart exactly:
/// prompt, resolution "2K", duration 5, ratio "16:9", first_frame_image.
#[derive(Debug, Serialize)]
pub struct H3Payload<'a> {
    pub prompt: &'a str,
    pub resolution: &'a str,
    pub duration: u32,
    pub ratio: &'a str,
    pub first_frame_image: &'a str,
}

/// Build the MiniMax-H3 request body (doc-verbatim defaults).
pub fn h3_request<'a>(
    model: &'a str,
    prompt: &'a str,
    first_frame_image: &'a str,
) -> H3Request<'a> {
    H3Request {
        model,
        payload: H3Payload {
            prompt,
            resolution: "2K",
            duration: 5,
            ratio: "16:9",
            first_frame_image,
        },
    }
}

/// Provider metadata: (model, kind, price, campaign_free).
///
/// `campaign_free` reflects the GMI MiniMax Week campaign (2026-08-24 →
/// 09-06); it describes pricing only, never liveness. Checked live by
/// `providers::probe_providers`.
pub const PROVIDER_TABLE: &[(&str, &str, &str, bool)] = &[
    (
        "MiniMaxAI/MiniMax-M2.7",
        "llm",
        "free during MiniMax Week (2026-08-24 → 09-06)",
        true,
    ),
    (
        "MiniMaxAI/MiniMax-M3",
        "llm",
        "free during MiniMax Week",
        true,
    ),
    (
        "minimax-music-3.0",
        "music",
        "free during MiniMax Week",
        true,
    ),
    ("minimax-music-2.5", "music", "paid per track", false),
    ("MiniMax-H3", "video", "$—/request, NOT in free week", false),
    ("Pollinations flux", "image", "free, 1024x576 cap", true),
];
/// the known key aliases.
pub fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(Value::String(s)) = map.get(*key) {
                    return Some(s.clone());
                }
            }
            for child in map.values() {
                if let Some(found) = find_string(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|v| find_string(v, keys)),
        _ => None,
    }
}
