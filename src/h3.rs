//! Hailuo H3 (MiniMax video) requestqueue adapter: submit, poll, download.
//!
//! Split from `providers.rs`, which was becoming a facade over four distinct
//! provider transports; H3 is the only one with its own submit+poll protocol
//! and the only consumer is the video pipeline.

use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use std::path::Path;
use std::time::Duration;

use crate::api;
use crate::providers::download_media;

/// Submit an H3 request: POST the requestqueue envelope, return `request_id`.
///
/// Non-2xx → `Err` with the HTTP code plus a short secret-free body
/// fragment (e.g. 402 "Insufficient credits").
pub async fn submit_h3(
    key: &str,
    url: &str,
    model: &str,
    prompt: &str,
    first_frame_image: &str,
) -> Result<String> {
    let body = api::h3_request(model, prompt, first_frame_image);
    let response = Client::new()
        .post(url)
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .context("H3 submit failed")?;
    let status = response.status();
    let value: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        return Err(anyhow!(
            "H3 submit returned HTTP {status}: {}",
            crate::providers::short_body(&value)
        ));
    }
    value
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("no request_id in H3 submit response: {value}"))
}

/// Poll a requestqueue request until terminal state, returning the media URL.
///
/// Polls every 5 s; terminal on `success` (via `parse_video_status` key
/// aliases), errors on `failed`/`cancelled`, gives up after `timeout`.
pub async fn poll_request(
    key: &str,
    url: &str,
    request_id: &str,
    timeout: Duration,
) -> Result<String> {
    let poll_url = format!("{url}/{request_id}");
    let deadline = tokio::time::Instant::now() + timeout;
    let client = Client::new();
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let resp: serde_json::Value = client
            .get(&poll_url)
            .bearer_auth(key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(media) = crate::video::parse_video_status(&resp) {
            return Ok(media);
        }
        let status = resp.get("status").and_then(|s| s.as_str()).unwrap_or("?");
        if status == "failed" || status == "cancelled" {
            anyhow::bail!("request {request_id} ended with status {status}");
        }
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("request {request_id} timed out after {timeout:?}");
        }
    }
}

/// Animate one scene image via the H3 requestqueue: submit, poll, download.
pub async fn animate_h3(
    key: &str,
    url: &str,
    model: &str,
    motion_prompt: &str,
    first_frame_image: &str,
    output: &Path,
) -> Result<()> {
    let request_id = submit_h3(key, url, model, motion_prompt, first_frame_image).await?;
    let clip = poll_request(key, url, &request_id, Duration::from_secs(600)).await?;
    download_media(&clip, output).await
}
