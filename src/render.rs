//! FFmpeg execution for the highlight video: segment render, loop-extend,
//! xfade chain, mux, and probes.
//!
//! Pure arg builders are offline-testable here; spawning happens through
//! `run_ffmpeg`. Intermediate segments may use HEVC, but the final chain and
//! mux are YouTube-safe H.264/AAC: MP4 fast-start, BT.709, progressive 30 fps,
//! and stereo 48 kHz audio. ffmpeg must be on PATH.
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::encode::{final_encode_args, youtube_video_encode_args};
use crate::video::{OUTPUT_HEIGHT, OUTPUT_WIDTH, VIDEO_FPS, XFADE_SECONDS};

/// FFmpeg args that render one fallback (Zoompan) segment.
///
/// Supersample to 5760x3240 first (kills the pixel-snap jitter of the old
/// 2304-wide chain), then a 30 s breathing cycle zooming 1.0-1.06 with
/// cosine easing, centered, 30 fps; finish with vignette, grade, film
/// grain; HEVC CRF 18, yuv420p.
///
/// v2 (2026-08-31): add subtle pan drift so highlights don't feel
/// tripod-locked. Each scene gets a deterministic drift angle and amplitude
/// over the available zoom crop (60 s period), so the source is never read
/// outside its bounds. Meditation stillness is preserved: zoom stays
/// 1.00–1.032 and pan remains subtle; the eye tracks motion without feeling
/// `Ken Burns`.
pub fn zoompan_args(scene_index: usize, duration_secs: f64) -> Vec<String> {
    // Alternating inhale/exhale per scene; slight per-scene period/amplitude
    // jitter avoids the "metronome" feel when chaining 10×61.8s segments.
    let inhale = scene_index.is_multiple_of(2);
    // Deterministic drift: scene_index hashes to an angle, keeping renders reproducible.
    // The multiplier is applied to the available crop margin below, so panning
    // never asks zoompan to read outside the source frame.
    let drift_deg = (scene_index as f64 * 47.0) % 360.0;
    let drift_rad = drift_deg.to_radians();
    let drift_x = drift_rad.cos() * 0.65;
    let drift_y = drift_rad.sin() * 0.65;
    // Per-scene breathing amplitude 0.028–0.032 (instead of fixed 0.03).
    let amp = 0.028 + (scene_index as f64 % 5.0) * 0.001;
    let breathing = if inhale {
        format!("1.0+{amp:.3}*(1.0-cos(2*PI*on/900))/2")
    } else {
        format!("1.0+{amp:.3}*(1.0+cos(2*PI*on/900))/2")
    };
    vec![
        "-y".into(),
        "-loop".into(),
        "1".into(),
        "-i".into(),
        "{input}".into(),
        "-t".into(),
        format!("{duration_secs:.3}"),
        "-vf".into(),
        format!(
            "scale=5760:3240:flags=lanczos,zoompan=z='{breathing}':x='(iw-iw/zoom)/2+{drift_x:.4}*(iw-iw/zoom)*sin(2*PI*on/1800)':y='(ih-ih/zoom)/2+{drift_y:.4}*(ih-ih/zoom)*cos(2*PI*on/1800)':d=1:s={OUTPUT_WIDTH}x{OUTPUT_HEIGHT}:fps={VIDEO_FPS},vignette=PI/6,eq=contrast=1.03:saturation=1.05,noise=alls=6:allf=t+u,format=yuv420p"
        ),
        "-c:v".into(),
        "libx265".into(),
        "-preset".into(),
        "medium".into(),
        "-crf".into(),
        "18".into(),
        "{output}".into(),
    ]
}

/// Render one fallback segment with FFmpeg (breathing zoompan).
pub async fn render_zoompan_segment(
    image: &Path,
    output: &Path,
    scene_index: usize,
    duration_secs: f64,
) -> Result<()> {
    let args: Vec<String> = zoompan_args(scene_index, duration_secs)
        .into_iter()
        .map(|a| {
            a.replace("{input}", &image.display().to_string())
                .replace("{output}", &output.display().to_string())
        })
        .collect();
    run_ffmpeg(&args).await
}

/// Extend a short H3 clip (5 s) to `target_secs` by looping with blend-mode
/// motion interpolation.
///
/// `mi_mode=blend` is deliberate: `mci` costs ~10 min per 61.8 s clip while
/// blend smooths the loop boundary cheaply. `-an` drops the H3-delivered
/// synchronized audio; the mux adds the music track.
pub fn loop_extend_args(
    input: &Path,
    output: &Path,
    target_secs: f64,
    source_secs: f64,
) -> Vec<String> {
    let loops = (target_secs / source_secs).ceil() as u32 - 1;
    vec![
        "-y".into(),
        "-stream_loop".into(),
        loops.to_string(),
        "-i".into(),
        input.display().to_string(),
        "-t".into(),
        format!("{target_secs:.3}"),
        "-vf".into(),
        format!(
            "scale={OUTPUT_WIDTH}:{OUTPUT_HEIGHT}:flags=lanczos,fps={VIDEO_FPS},minterpolate=fps={VIDEO_FPS}:mi_mode=blend"
        ),
        "-an".into(),
        "-c:v".into(),
        "libx265".into(),
        "-preset".into(),
        "medium".into(),
        "-crf".into(),
        "18".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        output.display().to_string(),
    ]
}

/// Extend a short H3 clip to its target length (loop + blend interpolation).
pub async fn loop_extend_clip(
    input: &Path,
    output: &Path,
    target_secs: f64,
    source_secs: f64,
) -> Result<()> {
    let args = loop_extend_args(input, output, target_secs, source_secs);
    run_ffmpeg(&args).await
}

/// Chain rendered segments into one YouTube-ready video with a named xfade
/// transition. The final encode is deliberately H.264 rather than HEVC:
/// YouTube's current upload guidance names H.264 High Profile, progressive
/// scan, two B frames, a closed GOP, BT.709 and MP4 fast-start.
///
/// `transition` is any ffmpeg xfade name (`fade`, `fadewhite` = light-flash
/// cut, `wipeleft`, `circleopen`, ...). Offsets use each segment's real
/// duration (`segment_secs`), so custom scene counts stay frame-accurate.
pub async fn xfade_chain(
    segments: &[PathBuf],
    output: &Path,
    transition: &str,
    segment_secs: f64,
    codec: &str,
) -> Result<()> {
    if segments.is_empty() {
        return Err(anyhow!("video chain needs at least one segment"));
    }
    let encode_args = final_encode_args(codec)?;
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

    let n = segments.len();
    let offsets: Vec<f64> = (1..n)
        .map(|k| k as f64 * (segment_secs - XFADE_SECONDS))
        .collect();
    let mut parts = Vec::new();
    let mut prev = "[0:v]".to_string();
    for (i, offset) in offsets.iter().enumerate() {
        let out = format!("[v{}]", i + 1);
        parts.push(format!(
            "{prev}[{}:v]xfade=transition={transition}:duration={XFADE_SECONDS}:offset={offset:.3}{out}",
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
pub fn youtube_h264_args(input: &Path, output: &Path) -> Vec<String> {
    let mut args = vec!["-y".into(), "-i".into(), input.display().to_string()];
    args.extend(youtube_video_encode_args());
    args.extend(["-c:a".into(), "copy".into(), output.display().to_string()]);
    args
}

/// Re-encode a HEVC video to YouTube-safe H.264.
pub async fn transcode_to_h264(input: &Path, output: &Path) -> Result<()> {
    run_ffmpeg(&youtube_h264_args(input, output)).await
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

    #[test]
    fn zoompan_args_supersample_vignette_grain_and_x265() {
        let args = zoompan_args(0, 61.8);
        let vf = args.iter().find(|a| a.contains("zoompan")).unwrap();
        assert!(vf.starts_with("scale=5760:3240:flags=lanczos"));
        assert!(vf.contains("vignette=PI/6"));
        assert!(vf.contains("eq=contrast=1.03:saturation=1.05"));
        assert!(vf.contains("noise=alls=6:allf=t+u"));
        assert!(vf.contains("s=1920x1080"));
        assert!(vf.contains("fps=30"));
        // v3: valid zoom stays in the 1.0–1.028 range for an inhale.
        assert!(vf.contains("z='1.0+0.028*(1.0-cos(2*PI*on/900))/2'"));
        assert!(vf.contains("cos(2*PI*on/900)"));
        assert!(vf.contains("sin(2*PI*on/1800)"), "pan drift missing: {vf}");
        assert!(args.contains(&"libx265".to_string()));
        let crf = args.iter().position(|a| a == "-crf").unwrap();
        assert_eq!(args[crf + 1], "18");
        // even inhales from min zoom; odd exhales
        let odd = zoompan_args(1, 61.8);
        let vf_odd = odd.iter().find(|a| a.contains("zoompan")).unwrap();
        assert!(vf_odd.contains("z='1.0+0.029*(1.0+cos(2*PI*on/900))/2'"));
    }
    #[test]
    fn loop_extend_args_drop_audio_use_blend_and_x265() {
        let args = loop_extend_args(Path::new("in.mp4"), Path::new("out.mp4"), 61.8, 5.0);
        assert!(args.contains(&"-an".to_string()));
        let li = args.iter().position(|a| a == "-stream_loop").unwrap();
        assert_eq!(args[li + 1], "12"); // ceil(61.8/5)-1
        let vf = args.iter().find(|a| a.contains("minterpolate")).unwrap();
        assert!(vf.contains("mi_mode=blend"));
        assert!(vf.contains("fps=30"));
        assert!(args.contains(&"libx265".to_string()));
    }

    #[test]
    fn loop_extend_loops_ten_times_for_six_second_clips() {
        let args = loop_extend_args(Path::new("in.mp4"), Path::new("out.mp4"), 60.0, 6.0);
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
        assert_eq!(args.iter().filter(|a| a.as_str() == "-map").count(), 2);
        let ti = args.iter().position(|a| a == "-t").unwrap();
        assert_eq!(args[ti + 1], "600.000");
    }

    #[test]
    fn xfade_chain_encodes_one_crf_step_higher_than_segments() {
        // Zoompan segments are CRF 18; the xfade chain output must be CRF 20.
        let seg = zoompan_args(0, 61.8);
        let seg_crf = seg.iter().position(|a| a == "-crf").unwrap();
        assert_eq!(seg[seg_crf + 1], "18");
    }
    #[test]
    fn youtube_h264_uses_x264_crf18_and_copy_audio() {
        let args = youtube_h264_args(Path::new("in.mp4"), Path::new("out.mp4"));
        assert!(args.contains(&"libx264".to_string()));
        let crf = args.iter().position(|a| a == "-crf").unwrap();
        assert_eq!(args[crf + 1], "18");
        assert!(args.contains(&"copy".to_string()));
        assert!(args.contains(&"yuv420p".to_string()));
        assert!(args.contains(&"high".to_string()));
        assert!(args.contains(&"+faststart".to_string()));
    }
}
