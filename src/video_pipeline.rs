//! Highlight-video pipeline: scene planning, per-scene H3/ffmpeg animation,
//! transition chain, and final audio mux. Orchestration only; every network
//! transport lives in `providers.rs`.
//!
//! H3 returns short clips, so successful AI shots are loop-extended to the
//! planned segment duration before the final chain. This is essential for
//! keeping scene boundaries synchronized with the music.
use anyhow::{Context, Result};
use do_music::config::Settings;
use do_music::duration::Duration;
use do_music::providers;
use do_music::render;
use do_music::video;
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
    /// Max ffmpeg fallback segments rendered concurrently.
    pub jobs: usize,
    /// Codec for the final chained video (see render::FINAL_CODEC_CHOICES).
    pub video_codec: String,
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
    if let Some(parent) = final_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).await?;
    }
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

/// Return true only for a cached segment whose duration matches the plan.
async fn cached_segment_is_usable(path: &Path, target_secs: f64) -> bool {
    path.exists()
        && render::probe_duration(path)
            .await
            .map(|actual| (actual - target_secs).abs() <= 0.75)
            .unwrap_or(false)
}

/// Render the planned video: images → segments → transition chain → mux.
///
/// Each scene tries H3 first (unless `no_i2v`); the first failure prints one
/// warning and switches the rest of the run to the ffmpeg fallback (a 402 or
/// 503 will not heal mid-run). Cached `seg-NN.mp4` files are reused only when
/// their duration matches the current plan.
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

    // H3 attempts stay sequential: the first failure permanently disables
    // i2v for the run (a 402/503 will not heal mid-run), so parallel calls
    // would only pile onto a dead provider. Cached segments and the ffmpeg
    // zoompan fallback, by contrast, are independent and run concurrently.
    let mut slots: Vec<Option<PathBuf>> = vec![None; segments.len()];
    let mut i2v_disabled = job.no_i2v;
    let mut fallback: Vec<(usize, PathBuf, PathBuf, f64)> = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let out = seg_dir.join(format!("seg-{:02}.mp4", i + 1));
        // A previous run may have left behind a short, pre-loop H3 clip. Do
        // not mistake it for a complete segment: the xfade offsets assume
        // every input covers the planned duration.
        if cached_segment_is_usable(&out, seg.duration).await {
            slots[i] = Some(out);
            println!("✓ segment {}/{} (cached)", i + 1, segments.len());
            continue;
        }

        let mut rendered_by_ai = false;
        if !i2v_disabled {
            let (_, image_url) = &scenes[i];
            let h3_raw = seg_dir.join(format!("seg-{:02}.h3.mp4", i + 1));
            let ai_result = async {
                if !h3_raw.exists() {
                    providers::animate_h3(
                        &settings.api_key,
                        &settings.music_url,
                        &job.video_model,
                        &video::h3_motion_prompt(&seg.motion_prompt, None),
                        image_url,
                        &h3_raw,
                    )
                    .await?;
                }
                let source_secs = render::probe_duration(&h3_raw).await?;
                if source_secs <= 0.0 {
                    anyhow::bail!("H3 returned a zero-length clip");
                }
                render::loop_extend_clip(&h3_raw, &out, seg.duration, source_secs).await
            }
            .await;
            match ai_result {
                Ok(()) => rendered_by_ai = true,
                Err(e) => {
                    println!(
                        "⚠ H3 unavailable ({e}) — using supersampled ffmpeg fallback for remaining scenes"
                    );
                    i2v_disabled = true;
                }
            }
        }
        if rendered_by_ai {
            slots[i] = Some(out);
            println!("✓ segment {}/{} (H3)", i + 1, segments.len());
        } else {
            fallback.push((i, seg.image.clone(), out, seg.duration));
        }
    }

    // Render fallback segments in bounded-parallel batches.
    let jobs = job.jobs.max(1);
    let total = segments.len();
    for batch in fallback.chunks(jobs) {
        let mut set = tokio::task::JoinSet::new();
        for (i, image, out, dur) in batch {
            let (i, image, out, dur) = (*i, image.clone(), out.clone(), *dur);
            set.spawn(async move { render::render_zoompan_segment(&image, &out, i, dur).await });
        }
        while let Some(res) = set.join_next().await {
            res.context("fallback render task panicked")??;
        }
        for (i, _, out, _) in batch {
            slots[*i] = Some(out.clone());
            println!("✓ segment {}/{} (ffmpeg)", i + 1, total);
        }
    }
    let rendered: Vec<PathBuf> = slots
        .into_iter()
        .map(|s| s.expect("segment rendered"))
        .collect();

    let chained = dir.join("video-xfade.mp4");
    render::xfade_chain(&rendered, &chained, &job.xfade, seg_secs, &job.video_codec).await?;
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
