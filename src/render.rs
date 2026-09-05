//! FFmpeg execution for the highlight video: segment render, loop-extend,
//! xfade chain, mux, and probes.
//!
//! Pure arg builders are offline-testable here; spawning happens through
//! `run_ffmpeg`. Cost-sensitive choices (supersample factor, scaler, grain,
//! intermediate codec/preset) come from `Quality` so a preview render and an
//! archive render share one code path. Intermediates are throwaway and encode
//! fast; the final chain and mux stay YouTube-safe H.264/AAC: MP4 fast-start,
//! BT.709, progressive 30 fps, stereo 48 kHz. ffmpeg must be on PATH.
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::encode::{final_encode_args, youtube_video_encode_args};
use crate::quality::Quality;
use crate::video::{OUTPUT_HEIGHT, OUTPUT_WIDTH, VIDEO_FPS, XFADE_SECONDS};

/// Render one fallback segment with a default calm shot.
///
/// Kept for callers without a music-driven plan; the video pipeline uses
/// `render_shot_segment` with a `Shot` chosen per scene.
pub async fn render_zoompan_segment(
    image: &Path,
    output: &Path,
    scene_index: usize,
    duration_secs: f64,
    quality: Quality,
) -> Result<()> {
    let shot = crate::motion::plan_shot(scene_index, crate::motion::Energy::new(0.35), 0);
    render_shot_segment(image, output, &shot, duration_secs, quality).await
}

/// Render one scene with a planned cinematic shot (camera move + atmosphere).
pub async fn render_shot_segment(
    image: &Path,
    output: &Path,
    shot: &crate::motion::Shot,
    duration_secs: f64,
    quality: Quality,
) -> Result<()> {
    let args: Vec<String> = shot_args(shot, duration_secs, quality)
        .into_iter()
        .map(|a| {
            a.replace("{input}", &image.display().to_string())
                .replace("{output}", &output.display().to_string())
        })
        .collect();
    run_ffmpeg(&args).await
}

/// FFmpeg args rendering one scene from a planned `Shot`.
pub fn shot_args(shot: &crate::motion::Shot, duration_secs: f64, quality: Quality) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-loop".into(),
        "1".into(),
        "-framerate".into(),
        VIDEO_FPS.to_string(),
        "-i".into(),
        "{input}".into(),
        "-t".into(),
        format!("{duration_secs:.3}"),
        "-vf".into(),
        crate::motion::filter_chain(shot, quality, duration_secs),
        "-an".into(),
    ];
    args.extend(quality.intermediate_encode_args());
    args.push("{output}".into());
    args
}

/// Extend a short H3 clip (5 s) to `target_secs` by looping it.
///
/// Perf note (v3): the old chain ran `minterpolate=fps=30` *after* an
/// explicit `fps=30`, i.e. motion interpolation with nothing to interpolate
/// — pure cost (often minutes per segment) for zero visual change. The loop
/// seam is instead softened by a short crossfade-free `tmix` on the
/// `high` tier only; cheaper tiers just cut, which is imperceptible on
/// meditative footage.
///
/// `-an` drops the H3-delivered synchronized audio; the mux adds the music.
pub fn loop_extend_args(
    input: &Path,
    output: &Path,
    target_secs: f64,
    source_secs: f64,
    quality: Quality,
) -> Vec<String> {
    let loops = (target_secs / source_secs).ceil() as u32 - 1;
    let smooth = match quality {
        Quality::High => ",tmix=frames=2:weights=1 1",
        _ => "",
    };
    let flags = quality.scale_flags();
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-stream_loop".into(),
        loops.to_string(),
        "-i".into(),
        input.display().to_string(),
        "-t".into(),
        format!("{target_secs:.3}"),
        "-vf".into(),
        format!("scale={OUTPUT_WIDTH}:{OUTPUT_HEIGHT}:flags={flags},fps={VIDEO_FPS}{smooth}"),
        "-an".into(),
    ];
    args.extend(quality.intermediate_encode_args());
    args.push(output.display().to_string());
    args
}

/// Extend a short H3 clip to its target length (loop + normalize).
pub async fn loop_extend_clip(
    input: &Path,
    output: &Path,
    target_secs: f64,
    source_secs: f64,
    quality: Quality,
) -> Result<()> {
    let args = loop_extend_args(input, output, target_secs, source_secs, quality);
    run_ffmpeg(&args).await
}

/// Chain rendered segments into one YouTube-ready video with a named xfade
/// transition. The final encode is deliberately H.264 rather than HEVC:
/// YouTube's current upload guidance names H.264 High Profile, progressive
/// scan, two B frames, a closed GOP, BT.709 and MP4 fast-start.
///
/// `transition` is any ffmpeg xfade name (`fade`, `fadewhite` = light-flash
/// cut, `wipeleft`, `circleopen`, ...). `segment_secs` carries each segment's
/// real duration, so both the uniform layout and highlight-snapped scenes of
/// differing lengths stay frame-accurate.
pub async fn xfade_chain(
    segments: &[PathBuf],
    output: &Path,
    transition: &str,
    segment_secs: &[f64],
    codec: &str,
    quality: Quality,
) -> Result<()> {
    xfade_chain_varied(
        segments,
        output,
        &[],
        transition,
        segment_secs,
        codec,
        quality,
    )
    .await
}

/// Chain segments using a per-cut transition list.
///
/// `transitions[i]` names the transition into segment `i + 1`; when the list
/// is shorter than the cut count the `fallback` name is used, so a caller
/// with no per-cut plan keeps the old single-transition behaviour.
#[allow(clippy::too_many_arguments)] // one ffmpeg invocation's worth of knobs
pub async fn xfade_chain_varied(
    segments: &[PathBuf],
    output: &Path,
    transitions: &[String],
    fallback: &str,
    segment_secs: &[f64],
    codec: &str,
    quality: Quality,
) -> Result<()> {
    if segments.is_empty() {
        return Err(anyhow!("video chain needs at least one segment"));
    }
    let encode_args = final_encode_args(codec, quality)?;
    let mut args: Vec<String> = vec!["-y".into()];
    for s in segments {
        args.push("-i".into());
        args.push(s.display().to_string());
    }

    if segments.len() == 1 {
        args.extend(encode_args);
        args.push("-an".into());
        args.push(output.display().to_string());
        return run_ffmpeg(&args).await;
    }

    if segment_secs.len() != segments.len() {
        return Err(anyhow!(
            "xfade chain got {} segments but {} durations",
            segments.len(),
            segment_secs.len()
        ));
    }
    let offsets = crate::highlights::variable_xfade_offsets(segment_secs);
    let mut parts = Vec::new();
    let mut prev = "[0:v]".to_string();
    for (i, offset) in offsets.iter().enumerate() {
        let out = format!("[v{}]", i + 1);
        let name = transitions.get(i).map(String::as_str).unwrap_or(fallback);
        parts.push(format!(
            "{prev}[{}:v]xfade=transition={name}:duration={XFADE_SECONDS}:offset={offset:.3}{out}",
            i + 1
        ));
        prev = out;
    }
    args.push("-filter_complex".into());
    args.push(parts.join(";"));
    args.push("-map".into());
    args.push(prev);
    args.push("-an".into());
    args.extend(encode_args);
    args.push(output.display().to_string());
    run_ffmpeg(&args).await
}

pub use crate::encode::FINAL_CODEC_CHOICES;

/// Mux a rendered video with the generated audio track.
///
/// The chain is already H.264; this step only adds stereo AAC-LC at 48 kHz,
/// applies a conservative music loudness target, and puts the MP4 index first
/// for faster YouTube/browser playback. Audio is trimmed to `total_secs`.
pub fn mux_args(video: &Path, audio: &Path, output: &Path, total_secs: f64) -> Vec<String> {
    vec![
        "-y".into(),
        "-i".into(),
        video.display().to_string(),
        "-i".into(),
        audio.display().to_string(),
        "-map".into(),
        "0:v".into(),
        "-map".into(),
        "1:a".into(),
        "-c:v".into(),
        "copy".into(),
        "-af".into(),
        "loudnorm=I=-14:TP=-1:LRA=11".into(),
        "-c:a".into(),
        "aac".into(),
        "-profile:a".into(),
        "aac_low".into(),
        "-b:a".into(),
        "384k".into(),
        "-ar".into(),
        "48000".into(),
        "-ac".into(),
        "2".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-shortest".into(),
        "-t".into(),
        format!("{total_secs:.3}"),
        output.display().to_string(),
    ]
}

/// YouTube-compatible H.264 re-encode args (for HEVC → AVC compat).
///
/// Audio is passed through when already AAC (as produced by
/// `mux_video_audio`).
pub fn youtube_h264_args(input: &Path, output: &Path, quality: Quality) -> Vec<String> {
    let mut args = vec!["-y".into(), "-i".into(), input.display().to_string()];
    args.extend(youtube_video_encode_args(quality));
    args.extend(["-c:a".into(), "copy".into(), output.display().to_string()]);
    args
}

/// Re-encode a HEVC video to YouTube-safe H.264.
pub async fn transcode_to_h264(input: &Path, output: &Path, quality: Quality) -> Result<()> {
    run_ffmpeg(&youtube_h264_args(input, output, quality)).await
}

/// Execute the mux plan.
pub async fn mux_video_audio(
    video: &Path,
    audio: &Path,
    output: &Path,
    total_secs: f64,
) -> Result<()> {
    let args = mux_args(video, audio, output, total_secs);
    run_ffmpeg(&args).await
}

/// Run FFmpeg with the given argument list.
async fn run_ffmpeg(args: &[String]) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args(args)
        .status()
        .await
        .context("failed to spawn ffmpeg (is it installed?)")?;
    if !status.success() {
        return Err(anyhow!("ffmpeg failed: {:?}", args));
    }
    Ok(())
}

/// Materialize any ffmpeg-readable audio as 22050 Hz mono 16-bit PCM WAV.
///
/// The python visualizer's analysis is numpy-only, so it consumes plain
/// PCM WAV; this keeps the Python side free of decode dependencies.
pub async fn materialize_pcm_wav(input: &Path, output: &Path) -> Result<()> {
    let args: Vec<String> = [
        "-y",
        "-v",
        "error",
        "-i",
        &input.display().to_string(),
        "-ac",
        "1",
        "-ar",
        "22050",
        "-sample_fmt",
        "s16",
        output.to_str().context("non-utf8 wav path")?,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    run_ffmpeg(&args).await
}

/// Raw inter-frame luma delta treated as maximum visual motion.
///
/// See `measure_motion` for how this was calibrated.
pub const MOTION_FULL_SCALE: f64 = 0.5;

/// Mean absolute frame-to-frame luma change of a video, normalized to 0..1.
///
/// This is the autotune feedback signal: how much the finished picture
/// actually moves. Sampled at 2 fps on a tiny 64x36 grayscale decode, so a
/// ten-minute video costs well under a second to measure.
///
/// `MOTION_FULL_SCALE` maps this raw signal onto 0..1. It is calibrated,
/// not guessed: rendering the whole move vocabulary at 2 fps/64x36 gives a
/// mean inter-frame luma delta of ~0.011 for a static `hold`, ~0.16-0.35
/// for `breathe`, and ~0.32-0.50 for an energetic `push-in`. Half a grey
/// level per pixel per sample is therefore "as lively as this vocabulary
/// gets", so that is full scale.
pub async fn measure_motion(path: &Path) -> Result<f64> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-vf",
            "fps=2,scale=64:36",
            "-an",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "-",
        ])
        .output()
        .await
        .context("failed to spawn ffmpeg for motion measurement")?;
    let frame = 64 * 36;
    let data = out.stdout;
    if data.len() < frame * 2 {
        return Ok(0.0);
    }
    let frames = data.len() / frame;
    let mut total = 0.0f64;
    for i in 1..frames {
        let (a, b) = (
            &data[(i - 1) * frame..i * frame],
            &data[i * frame..(i + 1) * frame],
        );
        let diff: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
        total += diff as f64 / frame as f64;
    }
    let mean = total / (frames - 1) as f64;
    Ok((mean / 255.0 / MOTION_FULL_SCALE).clamp(0.0, 1.0))
}

/// Probe the duration of a media file via ffprobe.
pub async fn probe_duration(path: &Path) -> Result<f64> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .await
        .context("failed to spawn ffprobe")?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim()
        .parse::<f64>()
        .context(format!("ffprobe returned unparsable duration: {text:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{Energy, Move, plan_shot};

    #[test]
    fn shot_args_wrap_the_motion_chain_with_cheap_intermediates() {
        let shot = plan_shot(1, Energy::new(0.5), 10);
        let args = shot_args(&shot, 61.8, Quality::Balanced);
        let vf = args.iter().find(|a| a.contains("scale=")).unwrap();
        assert!(vf.starts_with("scale=3840:2160"), "{vf}");
        assert!(vf.ends_with("format=yuv420p"), "{vf}");
        assert!(args.contains(&"libx264".to_string()));
        assert!(args.contains(&"-an".to_string()));
        let fr = args.iter().position(|a| a == "-framerate").unwrap();
        assert_eq!(args[fr + 1], "30");
        let t = args.iter().position(|a| a == "-t").unwrap();
        assert_eq!(args[t + 1], "61.800");
    }

    #[test]
    fn opening_scene_uses_the_calm_default_shot() {
        let shot = plan_shot(0, Energy::new(0.9), 10);
        assert_eq!(shot.camera, Move::Breathe);
    }

    #[test]
    fn loop_extend_args_drop_audio_and_skip_pointless_minterpolate() {
        let args = loop_extend_args(
            Path::new("in.mp4"),
            Path::new("out.mp4"),
            61.8,
            5.0,
            Quality::Balanced,
        );
        assert!(args.contains(&"-an".to_string()));
        let li = args.iter().position(|a| a == "-stream_loop").unwrap();
        assert_eq!(args[li + 1], "12"); // ceil(61.8/5)-1
        let vf = args.iter().find(|a| a.contains("fps=")).unwrap();
        // minterpolate at the same fps as its input was a no-op costing minutes.
        assert!(!vf.contains("minterpolate"), "{vf}");
        assert!(vf.contains("fps=30"));
        assert!(args.contains(&"libx264".to_string()));

        // Only the archive tier pays for seam smoothing.
        let high = loop_extend_args(
            Path::new("in.mp4"),
            Path::new("out.mp4"),
            61.8,
            5.0,
            Quality::High,
        );
        let vf = high.iter().find(|a| a.contains("fps=")).unwrap();
        assert!(vf.contains("tmix=frames=2"), "{vf}");
    }

    #[test]
    fn loop_extend_loops_ten_times_for_six_second_clips() {
        let args = loop_extend_args(
            Path::new("in.mp4"),
            Path::new("out.mp4"),
            60.0,
            6.0,
            Quality::Balanced,
        );
        let li = args.iter().position(|a| a == "-stream_loop").unwrap();
        assert_eq!(args[li + 1], "9");
        let ti = args.iter().position(|a| a == "-t").unwrap();
        assert_eq!(args[ti + 1], "60.000");
    }

    #[test]
    fn mux_maps_video_and_audio_with_youtube_audio_settings() {
        let args = mux_args(
            Path::new("v.mp4"),
            Path::new("a.mp3"),
            Path::new("o.mp4"),
            600.0,
        );
        assert!(args.contains(&"loudnorm=I=-14:TP=-1:LRA=11".to_string()));
        assert!(args.contains(&"384k".to_string()));
        assert!(args.contains(&"48000".to_string()));
        assert!(args.contains(&"+faststart".to_string()));
        assert!(!args.contains(&"hvc1".to_string()));
        // The mux must never re-encode the finished chain.
        assert!(args.contains(&"copy".to_string()));
        assert_eq!(args.iter().filter(|a| a.as_str() == "-map").count(), 2);
        let ti = args.iter().position(|a| a == "-t").unwrap();
        assert_eq!(args[ti + 1], "600.000");
    }

    #[test]
    fn youtube_h264_uses_x264_crf18_and_copy_audio() {
        let args = youtube_h264_args(
            Path::new("in.mp4"),
            Path::new("out.mp4"),
            Quality::default(),
        );
        assert!(args.contains(&"libx264".to_string()));
        let crf = args.iter().position(|a| a == "-crf").unwrap();
        assert_eq!(args[crf + 1], "18");
        assert!(args.contains(&"copy".to_string()));
        assert!(args.contains(&"yuv420p".to_string()));
        assert!(args.contains(&"high".to_string()));
        assert!(args.contains(&"+faststart".to_string()));
    }
}
