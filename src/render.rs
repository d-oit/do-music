//! FFmpeg execution for the highlight video: segment render, loop-extend,
//! xfade chain, mux, and probes.
//!
//! Pure arg builders are offline-testable here; spawning happens through
//! `run_ffmpeg`. Quality chain (per revamp plan): supersampled zoompan with
//! vignette + grain + grade, HEVC CRF 18 segments / CRF 20 chain,
//! `-an` + `minterpolate=mi_mode=blend` loop-extend for H3 clips,
//! `-tag:v hvc1` mux. ffmpeg must be on PATH (`/home/doit/bin` locally).
use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::video::{OUTPUT_HEIGHT, OUTPUT_WIDTH, VIDEO_FPS, XFADE_SECONDS};

/// FFmpeg args that render one fallback (Zoompan) segment.
///
/// Supersample to 5760x3240 first (kills the pixel-snap jitter of the old
/// 2304-wide chain), then a 30 s breathing cycle zooming 1.0-1.06 with
/// cosine easing, centered, 30 fps; finish with vignette, grade, film
/// grain; HEVC CRF 18, yuv420p.
pub fn zoompan_args(scene_index: usize, duration_secs: f64) -> Vec<String> {
    // Odd scenes inhale (start at zoom 1.0), even scenes exhale (start at
    // 1.06) so consecutive scenes breathe in opposition, like the prior build.
    let phase = if scene_index.is_multiple_of(2) {
        '-'
    } else {
        '+'
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
            "scale=5760:3240:flags=lanczos,zoompan=z='{phase}0.03+0.03*cos(2*PI*on/900)':x='iw/2-(iw/zoom)/2':y='ih/2-(ih/zoom)/2':d=1:s={OUTPUT_WIDTH}x{OUTPUT_HEIGHT}:fps={VIDEO_FPS},vignette=PI/6,eq=contrast=1.03:saturation=1.05,noise=alls=6:allf=t+u,format=yuv420p"
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

/// Chain rendered segments into one video with a named xfade transition.
///
/// `transition` is any ffmpeg xfade name (`fade`, `fadewhite` = light-flash
/// cut, `wipeleft`, `circleopen`, ...). Offsets use each segment's real
/// duration (`segment_secs`), so custom scene counts stay frame-accurate.
/// Final chain encodes one CRF step higher (20) than the segments (18).
pub async fn xfade_chain(
    segments: &[PathBuf],
    output: &Path,
    transition: &str,
    segment_secs: f64,
) -> Result<()> {
    if segments.len() < 2 {
        return Err(anyhow!("xfade chain needs at least two segments"));
    }
    let n = segments.len();
    let offsets: Vec<f64> = (1..n)
        .map(|k| k as f64 * (segment_secs - XFADE_SECONDS))
        .collect();
    let mut args: Vec<String> = vec!["-y".into()];
    for s in segments {
        args.push("-i".into());
        args.push(s.display().to_string());
    }
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
    args.extend([
        "-c:v".into(),
        "libx265".into(),
        "-preset".into(),
        "medium".into(),
        "-crf".into(),
        "20".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        output.display().to_string(),
    ]);
    run_ffmpeg(&args).await
}

/// Mux a rendered video with the generated audio track.
///
/// `-tag:v hvc1` keeps HEVC-in-mp4 playable outside ffplay. Video stream is
/// copied untouched; audio is loudness-normalized and trimmed to `total_secs`.
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
        "-tag:v".into(),
        "hvc1".into(),
        "-af".into(),
        "loudnorm=I=-16:TP=-1.5:LRA=11".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        "-ar".into(),
        "48000".into(),
        "-shortest".into(),
        "-t".into(),
        format!("{total_secs:.3}"),
        output.display().to_string(),
    ]
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
        assert!(vf.contains("z='-0.03+0.03*cos"));
        assert!(args.contains(&"libx265".to_string()));
        let crf = args.iter().position(|a| a == "-crf").unwrap();
        assert_eq!(args[crf + 1], "18");
        // even index inhales from min zoom; odd exhales
        let odd = zoompan_args(1, 61.8);
        let vf_odd = odd.iter().find(|a| a.contains("zoompan")).unwrap();
        assert!(vf_odd.contains("z='+0.03+0.03*cos"));
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
    fn mux_maps_video_and_audio_with_loudnorm_and_hvc1() {
        let args = mux_args(
            Path::new("v.mp4"),
            Path::new("a.mp3"),
            Path::new("o.mp4"),
            600.0,
        );
        assert!(args.contains(&"loudnorm=I=-16:TP=-1.5:LRA=11".to_string()));
        assert!(args.contains(&"hvc1".to_string()));
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
}
