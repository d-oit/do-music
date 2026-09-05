//! Music-driven inputs for the video pipeline: LLM art direction (with
//! optional web research), musical highlight analysis, and the self-tuning
//! render memory write-back.
//!
//! Split out of `video_pipeline.rs` to keep both files under the ~500-line
//! rule. Everything here is best-effort by design: each function degrades to
//! a sensible default and prints a note rather than failing a render that
//! could otherwise succeed.

use anyhow::Context;
use do_music::artdirection::Brief;
use do_music::autotune::{Memory, RenderRecord};
use do_music::config::Settings;
use do_music::highlights::Highlights;
use do_music::motion;
use do_music::providers;
use do_music::render;
use std::path::Path;
use tokio::fs;

use crate::video_pipeline::VideoJob;

/// Ask the LLM to art-direct this video, optionally after web research.
///
/// Best-effort: any failure prints a note and returns `None`, leaving the
/// caller on the builtin scene arc. A broken art-direction call must not
/// cost the user a render they could otherwise have had.
pub(crate) async fn art_direct(settings: &Settings, job: &VideoJob, scene_count: usize) -> Option<Brief> {
    let research = if job.research {
        match providers::research_visual_style(
            &settings.api_key,
            &settings.llm_base_url,
            &settings.llm_model,
            &job.prompt,
        )
        .await
        {
            Ok(Some(notes)) => {
                println!("🔎 visual research: {} reference notes", notes.lines().count());
                Some(notes)
            }
            Ok(None) => {
                println!("🔎 visual research returned nothing usable — continuing without it");
                None
            }
            Err(e) => {
                println!("⚠ visual research failed ({e:#}) — continuing without it");
                None
            }
        }
    } else {
        None
    };
    match providers::art_direct(
        &settings.api_key,
        &settings.llm_base_url,
        &settings.llm_model,
        &job.prompt,
        scene_count,
        research.as_deref(),
    )
    .await
    {
        Ok(brief) => {
            println!(
                "🎨 art direction: {} scenes, palette \"{}\", energy {:.2}",
                brief.scenes.len(),
                brief.palette,
                brief.energy
            );
            Some(brief)
        }
        Err(e) => {
            println!("⚠ art direction failed ({e:#}) — using the builtin scene arc");
            None
        }
    }
}

/// Analyze `audio` for musical highlights via the embedded python module.
///
/// Returns `None` (with a printed note) when python/numpy is unavailable or
/// the analysis fails: highlight snapping is an enhancement, never a reason
/// to abort a render that would otherwise succeed.
pub(crate) async fn detect_highlights(audio: &Path, dir: &Path, buckets: usize) -> Option<Highlights> {
    let run = async {
        do_music::visual::check_python()?;
        let package_parent = do_music::visual::ensure_package()?;
        let wav = dir.join("highlight-analysis.wav");
        let marks = dir.join("highlight-marks.json");
        render::materialize_pcm_wav(audio, &wav).await?;
        let args = do_music::visual::highlight_args(&wav, &marks, buckets);
        let status = tokio::process::Command::new("python3")
            .args(&args)
            .env("PYTHONPATH", &package_parent)
            .status()
            .await
            .context("failed to spawn python3 for highlight analysis")?;
        if !status.success() {
            anyhow::bail!("highlight analysis exited with {status}");
        }
        let text = fs::read_to_string(&marks).await?;
        let _ = fs::remove_file(&wav).await;
        Highlights::parse(&text)
    };
    match run.await {
        Ok(h) => Some(h),
        Err(e) => {
            println!("⚠ highlight analysis unavailable ({e:#}) — using even scene pacing");
            None
        }
    }
}

/// Measure the finished video and append a record to the render memory.
///
/// This is the sensor half of the self-tuning loop: it compares how much
/// the output actually moves against how energetic the music was, so the
/// next render can correct its motion budget.
///
/// Best-effort throughout: a failed measurement must never turn a completed
/// render into an error.
pub(crate) async fn record_render(
    dir: &Path,
    mut memory: Memory,
    produced: &Path,
    shots: &[motion::Shot],
    analysis: Option<&Highlights>,
    genre: &str,
    bias: f64,
) {
    let visual_motion = match render::measure_motion(produced).await {
        Ok(v) => v,
        Err(e) => {
            println!("⚠ could not measure output motion ({e:#}) — not recording this render");
            return;
        }
    };
    let music_energy = analysis
        .map(|h| h.mean_energy)
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(0.35);
    let distinct: std::collections::HashSet<&str> = shots.iter().map(|s| s.camera.name()).collect();
    let shot_variety = if shots.is_empty() {
        0.0
    } else {
        distinct.len() as f64 / shots.len() as f64
    };
    let record = RenderRecord {
        genre: genre.to_string(),
        music_energy,
        visual_motion,
        shot_variety,
        bias_used: bias,
    };
    println!(
        "🧠 recorded: music energy {music_energy:.2}, visual motion {visual_motion:.2}, shot variety {shot_variety:.2}"
    );
    if let Err(e) = memory.append(dir, record) {
        println!("⚠ could not persist render memory ({e:#})");
    }
}
