//! GMI Cloud HTTP transports: LLM chat, music generation, Pollinations
//! image fetch, and the provider probe. The H3 requestqueue adapter lives
//! in `h3.rs`; web research in `research.rs`.
//!
//! Layer rule: adapters only — no planning rules here. All error text is
//! secret-free: keys and auth headers are never formatted into messages.

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use std::{path::Path, time::Duration};

use crate::api::{self, ChatMessage, ChatRequest, LlmResponse, ResponseFormat};
use crate::config::Settings;

/// One LLM chat completion; returns trimmed assistant content.
pub async fn chat_completion(
    key: &str,
    base: &str,
    model: &str,
    system: &str,
    user: &str,
    temperature: f32,
    json_mode: bool,
) -> Result<String> {
    let body = ChatRequest {
        model,
        messages: vec![
            ChatMessage {
                role: "system",
                content: system,
            },
            ChatMessage {
                role: "user",
                content: user,
            },
        ],
        temperature,
        response_format: if json_mode {
            ResponseFormat::json_object()
        } else {
            ResponseFormat::text()
        },
    };
    let response: LlmResponse = Client::new()
        .post(format!("{base}/chat/completions"))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("invalid LLM response")?;
    Ok(response
        .choices
        .first()
        .context("LLM returned no choices")?
        .message
        .content
        .trim()
        .into())
}

/// Music-director system prompt for `evaluate` (moved verbatim from main.rs).
pub const SPEC_SYSTEM_PROMPT: &str = r#"You are a concise music director. Convert the user's idea into strict JSON with fields: intent:string, genre:string[], mood:string[], tempo:string, instruments:string[], vocals:string, energy:string, arrangement:string, style_prompt:string, lyrics:string, score:number. Preserve intent. Resolve contradictions. For instrumental music lyrics must be [Inst]. style_prompt must be usable directly by MiniMax Music 3.0 and stay under 2000 characters. lyrics must stay under 3500 characters. Score generation readiness from 0 to 10. Return JSON only."#;

/// Prompt-engineer system prompt for the `optimize` command.
pub const OPTIMIZE_SYSTEM_PROMPT: &str = "You are a music prompt engineer. Rewrite the user's idea into ONE rich generation prompt for MiniMax Music 3.0: style, instrumentation, tempo, mood, dynamics, production texture. Plain text, max 120 words, no preamble. Preserve the user's intent.";

/// Evaluate a prompt into a `MusicSpec` (one retry on invalid JSON).
pub async fn evaluate(
    key: &str,
    base: &str,
    model: &str,
    prompt: &str,
    instrumental: bool,
) -> Result<crate::plan::MusicSpec> {
    let user = if instrumental {
        format!("Instrumental request: {prompt}")
    } else {
        prompt.into()
    };
    let content = chat_completion(key, base, model, SPEC_SYSTEM_PROMPT, &user, 0.2, true).await?;
    match api::parse_spec(&content) {
        Ok(spec) => Ok(spec),
        Err(_first) => {
            let content =
                chat_completion(key, base, model, SPEC_SYSTEM_PROMPT, &user, 0.2, true).await?;
            api::parse_spec(&content).map_err(|_| {
                anyhow!("LLM returned invalid MusicSpec JSON twice; raw content: {content}")
            })
        }
    }
}

/// Ask the LLM for a visual brief (art direction) for a music idea.
///
/// One retry on unparsable JSON, mirroring `evaluate`: these models
/// occasionally emit prose despite `json_mode`.
pub async fn art_direct(
    key: &str,
    base: &str,
    model: &str,
    music_prompt: &str,
    scene_count: usize,
    research: Option<&str>,
) -> Result<crate::artdirection::Brief> {
    use crate::artdirection::{ART_SYSTEM_PROMPT, Brief, art_user_prompt};
    let user = art_user_prompt(music_prompt, scene_count, research);
    let content = chat_completion(key, base, model, ART_SYSTEM_PROMPT, &user, 0.7, true).await?;
    match Brief::parse(&content) {
        Ok(b) => Ok(b),
        Err(_first) => {
            let content =
                chat_completion(key, base, model, ART_SYSTEM_PROMPT, &user, 0.4, true).await?;
            Brief::parse(&content)
                .map_err(|e| anyhow!("LLM returned an unusable visual brief twice: {e}"))
        }
    }
}

/// Submit one music generation and download the audio to `output`.
pub async fn generate_music(
    key: &str,
    url: &str,
    model: &str,
    prompt: &str,
    lyrics: &str,
    output: &Path,
) -> Result<()> {
    let body = api::music_request(model, lyrics, prompt);
    let response = Client::new()
        .post(url)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    let value: serde_json::Value = response.json().await?;
    let media = api::find_string(
        &value,
        &[
            "audio_url",
            "audioUrl",
            "url",
            "download_url",
            "downloadUrl",
        ],
    )
    .ok_or_else(|| anyhow!("GMI response contained no audio URL: {value}"))?;
    download_media(&media, output).await
}

/// Download a media URL to `dest`.
pub async fn download_media(url: &str, dest: &Path) -> Result<()> {
    let bytes = Client::new()
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    tokio::fs::write(dest, bytes).await?;
    Ok(())
}

/// Fetch one Pollinations still into `dest`, validating magic bytes and size.
///
/// Returns the saved size in bytes. One GET only: the retry ladder lives
/// with the URL construction in `video::ensure_scene_images`.
pub async fn fetch_scene_image(url: &str, dest: &Path) -> Result<u64> {
    let response = Client::new()
        .get(url)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .context("scene image fetch failed")?;
    let status = response.status();
    let bytes = response.bytes().await?;
    if !status.is_success() {
        return Err(anyhow!("image fetch returned HTTP {status}"));
    }
    if bytes.len() <= 20 * 1024 {
        return Err(anyhow!("image too small ({} bytes)", bytes.len()));
    }
    let is_png = bytes.starts_with(b"\x89PNG");
    let is_jpeg = bytes.starts_with(&[0xFF, 0xD8, 0xFF]);
    if !is_png && !is_jpeg {
        return Err(anyhow!("image is neither PNG nor JPEG"));
    }
    tokio::fs::write(dest, &bytes).await?;
    Ok(bytes.len() as u64)
}

/// Result row of a provider probe.
#[derive(Debug, Clone)]
pub struct ProviderStatus {
    pub model: String,
    pub kind: &'static str,
    pub price: &'static str,
    pub campaign_free: bool,
    /// Human-readable live status (HTTP code or short note; secret-free).
    pub status: String,
}

/// Trim a JSON body to a short fragment for error messages (secret-free:
/// requestqueue bodies carry no credentials).
pub(crate) fn short_body(value: &serde_json::Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 200 {
        let cut: String = text.chars().take(200).collect();
        format!("{cut}…")
    } else {
        text
    }
}

/// Probe every provider in [`api::PROVIDER_TABLE`] that has a live check.
///
/// * Chat models: one cheap `"Reply with the single word: ok"` call.
/// * `minimax-music-3.0`: membership in the models list (never a music POST
///   — that is a full paid generation after the campaign).
/// * `MiniMax-H3`: minimal POST (`prompt:"ping"`); a 402 costs nothing.
///
/// The models GET doubles as the auth check; a 401/403 is noted on rows.
pub async fn probe_providers(settings: &Settings) -> Vec<ProviderStatus> {
    let client = Client::new();
    let models_root = settings
        .music_url
        .rsplit_once('/')
        .map(|(root, _)| root.to_string())
        .unwrap_or_else(|| settings.music_url.clone());
    let models_url = format!("{models_root}/models");
    let models_get = async {
        let response = client
            .get(&models_url)
            .bearer_auth(&settings.api_key)
            .send()
            .await?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        Ok::<(reqwest::StatusCode, serde_json::Value), reqwest::Error>((status, value))
    };
    let (models_status, models_body) = match models_get.await {
        Ok(pair) => pair,
        Err(_) => (
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!(null),
        ),
    };
    let auth_note = if models_status.is_success() {
        String::new()
    } else {
        format!(" (auth/models GET: {models_status})")
    };
    let music_in_list = models_status.is_success()
        && serde_json::to_string(&models_body)
            .map(|s| s.contains("minimax-music-3.0"))
            .unwrap_or(false);

    let mut rows = Vec::new();
    for (model, kind, price, campaign_free) in api::PROVIDER_TABLE {
        let status = match *kind {
            "llm" => match chat_completion(
                &settings.api_key,
                &settings.llm_base_url,
                model,
                "You are a health probe.",
                "Reply with the single word: ok",
                0.0,
                false,
            )
            .await
            {
                Ok(_) => format!("200 ok{auth_note}"),
                Err(e) => format!("error: {e:#}"),
            },
            "music" => {
                let note = match *model {
                    "minimax-music-3.0" => {
                        if music_in_list {
                            "in models list, free during MiniMax Week"
                        } else {
                            "not in models list"
                        }
                    }
                    _ => "paid per track",
                };
                format!("{note}{auth_note}")
            }
            "video" => {
                let body = api::h3_request(model, "ping", "https://example.com/none.png");
                match client
                    .post(&settings.music_url)
                    .bearer_auth(&settings.api_key)
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let s = resp.status();
                        let note = match s.as_u16() {
                            402 => "insufficient credits",
                            503 => "upstream capacity",
                            200 => "accepted",
                            _ => "",
                        };
                        format!("{s} {note}").trim_end().to_string()
                    }
                    Err(_) => "unreachable".into(),
                }
            }
            _ => "no live probe".into(),
        };
        rows.push(ProviderStatus {
            model: (*model).to_string(),
            kind,
            price,
            campaign_free: *campaign_free,
            status,
        });
    }
    rows
}
