//! Highlight-video pipeline: scene planning, per-scene H3/ffmpeg animation,
//! transition chain, and final audio mux. Orchestration only; every network
//! transport lives in `providers.rs`.
use anyhow::{Context, Result};
use do_music::config::Settings;
use do_music::duration::Duration;
use do_music::providers;
use do_music::render;
use do_music::video::{self, AnimationKind};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Everything the video pipeline needs, gathered from the CLI before work.
pub struct VideoJob {
    pub prompt: String,
    pub duration: String,
    pub dry_run: bool,
    pub instrumental: bool,
    pub output: Option<PathBuf>,
    pub video_model: String,
    pub no_i2v: bool,
    pub scenes: Option<PathBuf>,
    pub audio: Option<PathBuf>,
    pub xfade: String,
}

/// Orchestrate the highlight video path: scene list (builtin or --scenes),
/// music (reuse --audio or generate), scene images, per-scene animation
/// (H3 with ffmpeg fallback), transition chain, and the final mux. Music
/// and images are reused when already present.
pub(crate) async fn run_video(job: VideoJob) -> Result<()> {
    let settings = Settings::load()?;
    let dir = PathBuf::from("do-music-output");
    fs::create_dir_all(&dir).await?;

    // Scene prompts: user list when given, else the builtin monk arc.
    let scene_list = match &job.scenes {
        Some(path) => {
            let text = fs::read_to_string(path)
                .await
                .with_context(|| format!("reading scene list {}", path.display()))?;
            video::SceneList::parse(&text)?
        }
        None => video::SceneList {
            prompts: video::SCENE_PROMPTS.iter().map(|s| s.to_string()).collect(),
            style: None,
            negative: None,
        },
    };
    let frames_dir = match &job.scenes {
        Some(_) => dir.join("custom-frames"),
        None => dir.join("monk-frames"),
    };
    fs::create_dir_all(&frames_dir).await?;

    // Total length: with a scene list the music leads - the video lands
    // exactly on the probed audio duration (one segment per listed prompt);
    // otherwise the requested length drives the plan.
    let (total_seconds, audio_path) = match (&job.scenes, &job.audio) {
        (Some(_), Some(a)) if a.exists() => {
            let secs = render::probe_duration(a).await?;
            let secs = secs.round().max(6.0) as u64;
            println!(
                "Music leads: {} scenes cut to {:.0}s of audio ({})",
                scene_list.prompts.len(),
                secs,
                a.display()
            );
            (secs, Some(a.clone()))
        }
        _ => {
            let requested = Duration::parse(&job.duration)?;
            let audio_path = resolve_audio(&job, &dir, requested.seconds()).await?;
            (requested.seconds(), audio_path)
        }
    };

    let frames_dir = match &job.scenes {
        Some(_) => dir.join("custom-frames"),
        None => dir.join("monk-frames"),
    };

    // Video: render segments, chain with the transition, mux.
    let final_path = final_path(&dir, &job);
    render_video(
        &settings,
        &dir,
        &frames_dir,
        total_seconds,
        audio_path.as_deref(),
        &scene_list,
        &job,
    )
    .await?;
    println!("Done: {}", final_path.display());
    Ok(())
}

/// Audio reuse/generation decision for the requested-length path.
///
/// Explicit `--audio`, else `--output` if it exists, else the working 10m
/// mix, else generate. Never trust a same-named file of the wrong length:
/// probe duration. Dry runs render video-only.
async fn resolve_audio(job: &VideoJob, dir: &Path, requested_secs: u64) -> Result<Option<PathBuf>> {
    let _settings = Settings::load()?;
    match (&job.audio, &job.output) {
        (Some(a), _) if a.exists() => return Ok(Some(a.clone())),
        (_, Some(p)) if p.exists() => return Ok(Some(p.clone())),
        _ => {}
    }
    let candidate = dir.join("working-calm-10m.mp3");
    let usable = candidate.exists()
        && render::probe_duration(&candidate)
            .await
            .map(|d| (d - requested_secs as f64).abs() <= 5.0)
            .unwrap_or(false);
    if usable {
        return Ok(Some(candidate));
    }
    if job.dry_run {
        println!("Dry run: no music generation request sent.");
        return Ok(None);
    }
    crate::run_generation(
        &job.prompt,
        &job.duration,
        false,
        job.instrumental,
        job.output.clone(),
    )
    .await?;
    anyhow::bail!("audio generated; rerun the same command to mux it into the video");
}

/// Final mux name: `--output` when given, else derived from the scene list.
fn final_path(dir: &Path, job: &VideoJob) -> PathBuf {
    match &job.output {
        Some(p) => p.clone(),
        None => dir.join(match &job.scenes {
            Some(list_path) => format!(
                "{}-{}m.mp4",
                list_path.file_stem().unwrap_or_default().to_string_lossy(),
                job.duration.trim_end_matches('m')
            ),
            None => format!(
                "monk-meditation-{}m.mp4",
                job.duration.trim_end_matches('m')
            ),
        }),
    }
}

/// Video-only dry-run output name (same derivation, fixed suffix).
fn video_only_path(dir: &Path, job: &VideoJob) -> PathBuf {
    let mut p = final_path(dir, job);
    p.set_extension("");
    let name = format!(
        "{}-video-only.mp4",
        p.file_name().unwrap_or_default().to_string_lossy()
    );
    dir.join(name)
}

/// Render the planned video: images → segments → transition chain → mux.
///
/// Each scene tries H3 first (unless `no_i2v`); the first failure prints one
/// warning and switches the rest of the run to the ffmpeg fallback (a 402 or
/// 503 will not heal mid-run). Cached `seg-NN.mp4` files are reused.
async fn render_video(
    settings: &Settings,
    dir: &Path,
    frames_dir: &Path,
    total_seconds: u64,
    audio: Option<&Path>,
    scene_list: &video::SceneList,
    job: &VideoJob,
) -> Result<()> {
    // Custom scene list: one segment per listed prompt. Builtin arc: the
    // canonical 10-scene layout scaled to the requested total.
    let scene_count = match &job.scenes {
        Some(_) => scene_list.prompts.len(),
        None => video::plan_scene_count(total_seconds),
    };
    let scenes = video::ensure_scene_images(frames_dir, scene_list, scene_count).await?;
    let image_paths: Vec<PathBuf> = scenes.iter().map(|(p, _)| p.clone()).collect();
    let segments = video::plan_video_segments(
        &image_paths,
        total_seconds,
        scene_count,
        &scene_list.prompts,
    )?;
    let seg_secs = segments[0].duration;
    let seg_dir = dir.join(format!(
        "video-segments-{}x{:.1}",
        scene_count, segments[0].duration
    ));
    fs::create_dir_all(&seg_dir).await?;

    let mut rendered = Vec::new();
    let mut i2v_disabled = job.no_i2v;
    for (i, seg) in segments.iter().enumerate() {
        let out = seg_dir.join(format!("seg-{:02}.mp4", i + 1));
        if out.exists() {
            rendered.push(out);
            continue;
        }
        let used = if i2v_disabled {
            AnimationKind::Zoompan
        } else {
            let (_, image_url) = &scenes[i];
            match providers::animate_h3(
                &settings.api_key,
                &settings.music_url,
                &job.video_model,
                &video::h3_motion_prompt(&seg.motion_prompt, None),
                image_url,
                &out,
            )
            .await
            {
                Ok(()) => AnimationKind::Ai,
                Err(e) => {
                    println!(
                        "⚠ H3 unavailable ({e}) — using supersampled ffmpeg fallback for remaining scenes"
                    );
                    i2v_disabled = true;
                    AnimationKind::Zoompan
                }
            }
        };
        if used == AnimationKind::Zoompan {
            render::render_zoompan_segment(&seg.image, &out, i, seg.duration).await?;
        }
        println!("✓ segment {}/{}", i + 1, segments.len());
        rendered.push(out);
    }

    let chained = dir.join("video-xfade.mp4");
    render::xfade_chain(&rendered, &chained, &job.xfade, seg_secs).await?;
    match audio {
        Some(audio_path) => {
            render::mux_video_audio(
                &chained,
                audio_path,
                &final_path(dir, job),
                total_seconds as f64,
            )
            .await?;
        }
        None => {
            tokio::fs::rename(&chained, &video_only_path(dir, job)).await?;
        }
    }
    Ok(())
}
