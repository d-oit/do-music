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

/// Pan drift as a fraction of the available zoom crop margin.
///
/// The pan is centered at `margin/2` and the valid range is `[0, margin]`,
/// so anything above 0.5 is silently clamped by zoompan.
const DRIFT_AMPLITUDE: f64 = 0.45;

/// FFmpeg args that render one fallback (Zoompan) segment.
///
/// Cost model: `zoompan` reads a supersampled source once per output frame,
/// so the scaler dominates this filter chain. The supersample factor is
/// therefore tier-driven (`Quality::supersample`) rather than the old fixed
/// 5760x3240; `balanced` (2x = 3840x2160) keeps the anti-jitter benefit at
/// roughly 4/9 of the pixel throughput.
///
/// Motion: a 30 s breathing cycle zooming 1.00-1.032 with cosine easing plus
/// a deterministic per-scene pan drift (60 s period) over the available zoom
/// crop, so the source is never read outside its bounds. Meditation
/// stillness is preserved; the eye tracks motion without feeling `Ken Burns`.
///
/// v3 (2026-09-04): tier-driven supersample/scaler/grain, and the still is
/// decoded once (`-framerate` on the loop input) instead of being re-fed at
/// output rate.
pub fn zoompan_args(scene_index: usize, duration_secs: f64, quality: Quality) -> Vec<String> {
    // Alternating inhale/exhale per scene; slight per-scene period/amplitude
    // jitter avoids the "metronome" feel when chaining 10x61.8s segments.
    let inhale = scene_index.is_multiple_of(2);
    // Deterministic drift: scene_index hashes to an angle, keeping renders
    // reproducible. The multiplier scales the available crop margin, and the
    // pan is centered at half that margin, so the amplitude must stay <= 0.5
    // or zoompan clamps at the bounds and the drift visibly stalls at the
    // extremes of every cycle. `DRIFT_AMPLITUDE` keeps a safety margin.
    let drift_deg = (scene_index as f64 * 47.0) % 360.0;
    let drift_rad = drift_deg.to_radians();
    let drift_x = drift_rad.cos() * DRIFT_AMPLITUDE;
    let drift_y = drift_rad.sin() * DRIFT_AMPLITUDE;
    // Per-scene breathing amplitude 0.028-0.032 (instead of fixed 0.03).
    let amp = 0.028 + (scene_index as f64 % 5.0) * 0.001;
    let breathing = if inhale {
        format!("1.0+{amp:.3}*(1.0-cos(2*PI*on/900))/2")
    } else {
        format!("1.0+{amp:.3}*(1.0+cos(2*PI*on/900))/2")
    };
    let ss = quality.supersample();
    let (sw, sh) = (OUTPUT_WIDTH * ss, OUTPUT_HEIGHT * ss);
    let flags = quality.scale_flags();
    let grain = match quality.grain() {
        0 => String::new(),
        g => format!(",noise=alls={g}:allf=t+u"),
    };
    let vf = format!(
        "scale={sw}:{sh}:flags={flags},zoompan=z='{breathing}':x='(iw-iw/zoom)/2+{drift_x:.4}*(iw-iw/zoom)*sin(2*PI*on/1800)':y='(ih-ih/zoom)/2+{drift_y:.4}*(ih-ih/zoom)*cos(2*PI*on/1800)':d=1:s={OUTPUT_WIDTH}x{OUTPUT_HEIGHT}:fps={VIDEO_FPS},vignette=PI/6,eq=contrast=1.03:saturation=1.05{grain},format=yuv420p"
    );
    let mut args: Vec<String> = vec![
        "-y".into(),
        // Decode the still once and let zoompan generate the timeline.
        "-loop".into(),
        "1".into(),
        "-framerate".into(),
        VIDEO_FPS.to_string(),
        "-i".into(),
        "{input}".into(),
        "-t".into(),
        format!("{duration_secs:.3}"),
        "-vf".into(),
        vf,
        "-an".into(),
    ];
    args.extend(quality.intermediate_encode_args());
    args.push("{output}".into());
    args
}

/// Render one fallback segment with FFmpeg (breathing zoompan).
pub async fn render_zoompan_segment(
    image: &Path,
    output: &Path,
    scene_index: usize,
    duration_secs: f64,
    quality: Quality,
) -> Result<()> {
    let args: Vec<String> = zoompan_args(scene_index, duration_secs, quality)
        .into_iter()
        .map(|a| {
            a.replace("{input}", &image.display().to_string())
                .replace("{output}", &output.display().to_string())
        })
        .collect();
    run_ffmpeg(&args).await
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
    fn zoompan_args_supersample_vignette_grain_and_intermediate_codec() {
        let args = zoompan_args(0, 61.8, Quality::Balanced);
        let vf = args.iter().find(|a| a.contains("zoompan")).unwrap();
        // Balanced supersamples 2x (3840x2160), not the old fixed 5760x3240.
        assert!(vf.starts_with("scale=3840:2160:flags=lanczos"), "{vf}");
        assert!(vf.contains("vignette=PI/6"));
        assert!(vf.contains("eq=contrast=1.03:saturation=1.05"));
        assert!(vf.contains("noise=alls=5:allf=t+u"));
        assert!(vf.contains("s=1920x1080"));
        assert!(vf.contains("fps=30"));
        // Zoom stays in the 1.0-1.028 range for an inhale.
        assert!(vf.contains("z='1.0+0.028*(1.0-cos(2*PI*on/900))/2'"));
        assert!(vf.contains("sin(2*PI*on/1800)"), "pan drift missing: {vf}");
        // Pan must stay inside the crop; see DRIFT_AMPLITUDE.
        assert!(DRIFT_AMPLITUDE <= 0.5);
        // Intermediates are cheap x264, not x265 medium.
        assert!(args.contains(&"libx264".to_string()));
        assert!(args.contains(&"veryfast".to_string()));
        assert!(args.contains(&"-an".to_string()));
        // The still is decoded once at output rate.
        let fr = args.iter().position(|a| a == "-framerate").unwrap();
        assert_eq!(args[fr + 1], "30");
        // even inhales from min zoom; odd exhales
        let odd = zoompan_args(1, 61.8, Quality::Balanced);
        let vf_odd = odd.iter().find(|a| a.contains("zoompan")).unwrap();
        assert!(vf_odd.contains("z='1.0+0.029*(1.0+cos(2*PI*on/900))/2'"));
    }

    #[test]
    fn pan_drift_never_exceeds_the_available_crop_margin() {
        // x = margin/2 + m*margin*sin(t) must stay within [0, margin] for
        // every scene, otherwise zoompan clamps and the drift stalls at the
        // extremes of each cycle.
        for scene in 0..360usize {
            let rad = ((scene as f64 * 47.0) % 360.0).to_radians();
            for m in [rad.cos() * DRIFT_AMPLITUDE, rad.sin() * DRIFT_AMPLITUDE] {
                assert!(
                    m.abs() <= 0.5 + 1e-9,
                    "drift {m} clamps the crop in scene {scene}"
                );
            }
        }
    }

    #[test]
    fn zoompan_quality_tiers_trade_scaler_cost_for_fidelity() {
        let fast = zoompan_args(0, 10.0, Quality::Fast);
        let vf = fast.iter().find(|a| a.contains("zoompan")).unwrap();
        assert!(vf.starts_with("scale=1920:1080:flags=bicubic"), "{vf}");
        assert!(!vf.contains("noise="), "fast tier must skip grain: {vf}");
        assert!(fast.contains(&"ultrafast".to_string()));

        let high = zoompan_args(0, 10.0, Quality::High);
        let vf = high.iter().find(|a| a.contains("zoompan")).unwrap();
        assert!(vf.starts_with("scale=5760:3240:flags=lanczos"), "{vf}");
        assert!(vf.contains("noise=alls=6:allf=t+u"));
        assert!(high.contains(&"libx265".to_string()));
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
