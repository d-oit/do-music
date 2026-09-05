//! Highlight-video pipeline: scene planning, per-scene cinematic animation,
//! transition chain, and final audio mux. Orchestration only; network
//! transports live in `providers.rs` and the music-driven inputs (art
//! direction, highlight analysis, autotune write-back) in `video_inputs.rs`.
//!
//! H3 returns short clips, so successful AI shots are loop-extended to the
//! planned segment duration before the final chain. This is essential for
//! keeping scene boundaries synchronized with the music.
use anyhow::{Context, Result};
use do_music::autotune::Memory;
use do_music::config::Settings;
use do_music::duration::Duration;
use do_music::motion::{self, Energy};
use do_music::quality::Quality;
use do_music::render;
use do_music::video;
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::video_inputs::{art_direct, detect_highlights, record_render};

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
    /// Render tier trading wall-clock time against fidelity.
    pub quality: String,
    /// Snap scene cuts to detected musical highlights.
    pub highlights: bool,
    /// Derive scene imagery from the music via the LLM art director.
    pub art_direct: bool,
    /// Ground art direction in live web research.
    pub research: bool,
    /// Read/write the self-tuning render memory.
    pub autotune: bool,
    /// How far (seconds) a cut may move from its uniform position to reach a
    /// highlight.
    pub highlight_window: f64,
}

/// Orchestrate the highlight video path: scene list (builtin or --scenes),
/// music (reuse --audio or generate), scene images, per-scene animation
/// (H3 with ffmpeg fallback), transition chain, and the final mux. Music
/// and images are reused when already present.
pub(crate) async fn run_video(job: VideoJob) -> Result<()> {
    let settings = Settings::load()?;
    let quality = Quality::parse(&job.quality)?;
    let dir = PathBuf::from("do-music-output");
    fs::create_dir_all(&dir).await?;

    // Scene prompts, in precedence order: an explicit --scenes file, then
    // LLM art direction derived from the music, then the builtin monk arc.
    let brief = match (&job.scenes, job.art_direct, job.dry_run) {
        (None, true, false) => {
            art_direct(&settings, &job, video::plan_scene_count(600)).await
        }
        _ => None,
    };
    let scene_list = match (&job.scenes, &brief) {
        (Some(path), _) => {
            let text = fs::read_to_string(path)
                .await
                .with_context(|| format!("reading scene list {}", path.display()))?;
            video::SceneList::parse(&text)?
        }
        (None, Some(b)) => b.to_scene_list(),
        (None, None) => video::SceneList {
            prompts: video::SCENE_PROMPTS.iter().map(|s| s.to_string()).collect(),
            style: None,
            negative: None,
        },
    };
    // Art-directed runs get their own frame cache: the images belong to this
    // brief, and reusing the monk stills under a new palette would be wrong.
    let frames_dir = match (&job.scenes, &brief) {
        (Some(_), _) => dir.join("custom-frames"),
        (None, Some(_)) => dir.join("directed-frames"),
        (None, None) => dir.join("monk-frames"),
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
        quality,
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

/// Usable CPU count for the fallback render pool.
fn available_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
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
    quality: Quality,
) -> Result<()> {
    // Custom scene list: one segment per listed prompt. Builtin arc: the
    // canonical 10-scene layout scaled to the requested total.
    let scene_count = match &job.scenes {
        Some(_) => scene_list.prompts.len(),
        None => video::plan_scene_count(total_seconds),
    };
    let scenes = video::ensure_scene_images(frames_dir, scene_list, scene_count).await?;
    let image_paths: Vec<PathBuf> = scenes.iter().map(|(p, _)| p.clone()).collect();
    let mut segments = video::plan_video_segments(
        &image_paths,
        total_seconds,
        scene_count,
        &scene_list.prompts,
    )?;

    // Highlight snapping: move scene cuts onto detected musical moments so
    // transitions land with the music instead of on a metronome. Segments
    // become individually-timed; the chain math already handles that.
    let mut snapped = false;
    let wants_highlights = job.highlights && scene_count > 1;
    let detected = match (wants_highlights, audio) {
        (true, Some(audio_path)) => detect_highlights(audio_path, dir, scene_count).await,
        _ => None,
    };
    let analysis = detected;
    if let Some(marks) = &analysis {
        let durations =
            marks.segment_durations(scene_count, total_seconds as f64, job.highlight_window);
        let moved = durations
            .iter()
            .zip(segments.iter())
            .filter(|(new, old)| (*new - old.duration).abs() > 0.05)
            .count();
        for (seg, secs) in segments.iter_mut().zip(&durations) {
            seg.duration = *secs;
        }
        snapped = moved > 0;
        println!(
            "♪ {} highlight marks detected; {moved}/{scene_count} scene cuts snapped to the music",
            marks.marks.len()
        );
    }

    // Self-tuning: past renders tell us whether our motion has been running
    // flatter or busier than the music under it.
    let memory = if job.autotune {
        Memory::load(dir)
    } else {
        Memory::default()
    };
    let genre = job.prompt.split_whitespace().next().unwrap_or("").to_lowercase();
    let bias = if job.autotune {
        let b = memory.motion_bias(&genre);
        println!("🧠 {}", memory.summary(&genre));
        b
    } else {
        1.0
    };

    // Per-scene shot plan: each scene's camera move is chosen from the
    // vocabulary and scaled by the musical energy underneath it.
    let shots: Vec<motion::Shot> = (0..scene_count)
        .map(|i| {
            let raw = analysis
                .as_ref()
                .map(|h| h.energy_for(i))
                .unwrap_or(0.35);
            motion::plan_shot(i, Energy::new(raw * bias), scene_count)
        })
        .collect();
    let move_names: Vec<&str> = shots.iter().map(|s| s.camera.name()).collect();
    println!("🎬 shots: {}", move_names.join(" → "));

    // Transition vocabulary: strong musical cuts get a light flash, softer
    // ones a wipe, the rest a plain dissolve.
    let transitions: Vec<String> = (1..scene_count)
        .map(|i| {
            let strength = analysis.as_ref().map(|h| h.cut_strength(i)).unwrap_or(0.0);
            motion::transition_for(strength, i).to_string()
        })
        .collect();

    let seg_secs: Vec<f64> = segments.iter().map(|s| s.duration).collect();
    let seg_dir = dir.join(if snapped {
        format!("video-segments-{scene_count}x-highlight")
    } else {
        format!("video-segments-{}x{:.1}", scene_count, seg_secs[0])
    });
    fs::create_dir_all(&seg_dir).await?;

    // H3 attempts stay sequential: the first failure permanently disables
    // i2v for the run (a 402/503 will not heal mid-run), so parallel calls
    // would only pile onto a dead provider. Cached segments and the ffmpeg
    // zoompan fallback, by contrast, are independent and run concurrently.
    let mut slots: Vec<Option<PathBuf>> = vec![None; segments.len()];
    let mut i2v_disabled = job.no_i2v;
    let mut fallback: Vec<(usize, PathBuf, PathBuf, f64, motion::Shot)> = Vec::new();
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
                render::loop_extend_clip(&h3_raw, &out, seg.duration, source_secs, quality).await
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
            fallback.push((i, seg.image.clone(), out, seg.duration, shots[i]));
        }
    }

    // Render fallback segments with a rolling pool of `jobs` ffmpeg
    // processes. A fixed-chunk loop would idle every finished worker until
    // the slowest segment of its batch completed; refilling as soon as one
    // finishes keeps all cores busy to the end of the queue.
    //
    // 0 means "auto": one ffmpeg per core, capped so a long scene list on a
    // big machine does not thrash memory with many 4K supersample buffers.
    let jobs = match job.jobs {
        0 => available_parallelism().clamp(1, 8),
        n => n,
    };
    let total = segments.len();
    let mut done = 0usize;
    let mut queue = fallback.into_iter();
    let mut set = tokio::task::JoinSet::new();
    loop {
        while set.len() < jobs {
            let Some((i, image, out, dur, shot)) = queue.next() else {
                break;
            };
            set.spawn(async move {
                render::render_shot_segment(&image, &out, &shot, dur, quality)
                    .await
                    .map(|()| (i, out))
            });
        }
        let Some(res) = set.join_next().await else {
            break;
        };
        let (i, out) = res.context("fallback render task panicked")??;
        slots[i] = Some(out);
        done += 1;
        println!("✓ segment {}/{} (ffmpeg, {done} rendered)", i + 1, total);
    }
    let rendered: Vec<PathBuf> = slots
        .into_iter()
        .map(|s| s.expect("segment rendered"))
        .collect();

    let chained = dir.join("video-xfade.mp4");
    render::xfade_chain_varied(
        &rendered,
        &chained,
        &transitions,
        &job.xfade,
        &seg_secs,
        &job.video_codec,
        quality,
    )
    .await?;
    let produced = match audio {
        Some(audio_path) => {
            let out = final_path(dir, job);
            render::mux_video_audio(&chained, audio_path, &out, total_seconds as f64).await?;
            out
        }
        None => {
            let out = video_only_path(dir, job);
            tokio::fs::rename(&chained, &out).await?;
            out
        }
    };

    // Close the learning loop: measure what we actually produced and record
    // it, so the next render can correct its motion budget.
    if job.autotune {
        record_render(dir, memory, &produced, &shots, analysis.as_ref(), &genre, bias).await;
    }
    Ok(())
}
