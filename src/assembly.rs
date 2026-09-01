use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::{fs, process::Command};

/// Deterministic plan for turning generated tracks into one output file.
/// Planning never touches audio, so it can be tested offline; `assemble`
/// executes the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssemblyStep {
    /// A single track: plain copy, no FFmpeg needed.
    Copy { from: PathBuf, to: PathBuf },
    /// Multiple tracks: concatenate with FFmpeg.
    Concat {
        inputs: Vec<PathBuf>,
        output: PathBuf,
    },
}

/// Plan assembly without touching audio: one file → copy, many → concat.
pub fn plan_assembly(files: &[PathBuf], output: &Path) -> Result<AssemblyStep> {
    match files {
        [] => Err(anyhow!("no tracks to assemble")),
        [one] => Ok(AssemblyStep::Copy {
            from: one.clone(),
            to: output.to_path_buf(),
        }),
        many => Ok(AssemblyStep::Concat {
            inputs: many.to_vec(),
            output: output.to_path_buf(),
        }),
    }
}

/// FFmpeg concat-list content: one `file '<path>'` line per input.
///
/// FFmpeg resolves concat-list entries relative to the **list file's
/// directory**, not the process CWD, so each path is rewritten relative to
/// the list file's parent. Absolute paths are kept as-is.
pub fn concat_list_content(list_path: &Path, files: &[PathBuf]) -> String {
    let base = list_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    files
        .iter()
        .map(|p| {
            let rel = p.strip_prefix(&base).unwrap_or(p);
            format!("file '{}'", rel.display())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Default output filename for a requested total length.
pub fn default_output_name(total_seconds: u64) -> &'static str {
    if total_seconds >= 60 * 60 {
        "mix-60m.mp3"
    } else {
        "song.mp3"
    }
}

/// Crossfade duration between tracks for YouTube assembly, in seconds.
pub const YOUTUBE_CROSSFADE_SECS: f64 = 8.0;

/// Loudness targets for YouTube (`-14 LUFS` is YouTube's normalization point).
pub const YOUTUBE_LUFS: &str = "-14";

/// Build the `filter_complex` for gapless YouTube assembly: chain `acrossfade`
/// clips with triangular curves, then `loudnorm` to YouTube LUFS.
///
/// Returns `None` for 0 or 1 input (no filter needed). For `n` inputs the
/// filter has `n` audio inputs `[0:a]...[n-1:a]` and one output `[aout]`.
pub fn youtube_filter_complex(n: usize) -> Option<String> {
    if n <= 1 {
        return None;
    }
    // Chain: [0:a][1:a]acrossfade=d=8:c1=tri:c2=tri[a01]; [a01][2:a]acrossfade=...
    let mut parts = Vec::new();
    let mut prev = "[0:a]".to_string();
    for i in 1..n {
        let is_last = i == n - 1;
        let out = if is_last {
            // Reserve [tmp] for pre-loudnorm, then add loudnorm step.
            "[tmp]".to_string()
        } else {
            format!("[a{:02}]", i)
        };
        parts.push(format!(
            "{prev}[{i}:a]acrossfade=d={}:c1=tri:c2=tri{}",
            YOUTUBE_CROSSFADE_SECS, out
        ));
        prev = out;
    }
    let mut filter = parts.join(";");
    filter.push_str(&format!(
        ";[tmp]loudnorm=I={}:TP=-1:LRA=11[aout]",
        YOUTUBE_LUFS
    ));
    Some(filter)
}

/// YouTube chapters content for a long-form mix: `00:00 Opening` etc.
///
/// `tracks` are the per-track lengths in seconds as returned by `plan_tracks`.
/// Chapters use the `SECTION_PHASES` names so the description matches the
/// musical arc. Output is ready to paste into YouTube's description (must
/// start at 00:00 to enable chapters).
pub fn youtube_chapters(tracks: &[u64]) -> String {
    use crate::plan::SECTION_PHASES;
    let mut out = String::new();
    let mut cursor = 0u64;
    for (i, &secs) in tracks.iter().enumerate() {
        let phase = SECTION_PHASES[((i as f64 / tracks.len() as f64 * SECTION_PHASES.len() as f64)
            as usize)
            .min(SECTION_PHASES.len() - 1)];
        let m = cursor / 60;
        let s = cursor % 60;
        // For 60m the YouTube format wants HH:MM:SS when >=60m; keep MM:SS for short, HH for long.
        if tracks.iter().sum::<u64>() >= 3600 {
            let h = m / 60;
            let mm = m % 60;
            out.push_str(&format!("{h:02}:{mm:02}:{s:02} {phase}\n"));
        } else {
            out.push_str(&format!("{m:02}:{s:02} {phase}\n"));
        }
        cursor += secs;
        // Subtract crossfade overlap from chapter start so chapters align to the audible seam.
        if i + 1 < tracks.len() {
            let overlap = YOUTUBE_CROSSFADE_SECS as u64;
            cursor = cursor.saturating_sub(overlap);
        }
    }
    out
}

/// Execute a YouTube-quality assembly: re-encode with crossfades + loudnorm.
///
/// Uses `acrossfade` (triangular, 8 s) chained across all inputs and a final
/// `loudnorm=I=-14:TP=-1:LRA=11` to hit YouTube's target. Output is MP3 320k
/// (or keep extension: `.mp3` → `libmp3lame 320k`, else `aac 320k`).
pub async fn assemble_youtube(files: &[PathBuf], output: &Path) -> Result<()> {
    if files.is_empty() {
        return Err(anyhow!("no tracks to assemble"));
    }
    if files.len() == 1 {
        let is_mp3 = output.extension().and_then(|e| e.to_str()) == Some("mp3");
        let mut args = vec![
            "-y".to_string(),
            "-i".to_string(),
            files[0].display().to_string(),
        ];
        args.extend([
            "-af".to_string(),
            format!("loudnorm=I={}:TP=-1:LRA=11", YOUTUBE_LUFS),
        ]);
        if is_mp3 {
            args.extend([
                "-c:a".to_string(),
                "libmp3lame".to_string(),
                "-b:a".to_string(),
                "320k".to_string(),
            ]);
        } else {
            args.extend([
                "-c:a".to_string(),
                "aac".to_string(),
                "-b:a".to_string(),
                "320k".to_string(),
            ]);
        }
        args.push(output.display().to_string());
        let status = Command::new("ffmpeg").args(&args).status().await?;
        if !status.success() {
            return Err(anyhow!("FFmpeg youtube assembly failed"));
        }
        return Ok(());
    }
    let filter = youtube_filter_complex(files.len()).expect("n>1");
    let mut args: Vec<String> = vec!["-y".to_string()];
    for f in files {
        args.push("-i".to_string());
        args.push(f.display().to_string());
    }
    args.push("-filter_complex".to_string());
    args.push(filter);
    args.push("-map".to_string());
    args.push("[aout]".to_string());
    let is_mp3 = output.extension().and_then(|e| e.to_str()) == Some("mp3");
    if is_mp3 {
        args.extend([
            "-c:a".to_string(),
            "libmp3lame".to_string(),
            "-b:a".to_string(),
            "320k".to_string(),
        ]);
    } else {
        args.extend([
            "-c:a".to_string(),
            "aac".to_string(),
            "-b:a".to_string(),
            "320k".to_string(),
        ]);
    }
    args.push(output.display().to_string());
    let status = Command::new("ffmpeg").args(&args).status().await?;
    if !status.success() {
        return Err(anyhow!("FFmpeg youtube assembly failed"));
    }
    Ok(())
}

/// Execute an assembly plan: copy a single track, or FFmpeg-concat many.
pub async fn assemble(files: &[PathBuf], output: &Path) -> Result<()> {
    match plan_assembly(files, output)? {
        AssemblyStep::Copy { from, to } => {
            fs::copy(&from, &to).await?;
        }
        AssemblyStep::Concat { inputs, output } => {
            let ffmpeg = Command::new("ffmpeg")
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            if !ffmpeg.map(|s| s.success()).unwrap_or(false) {
                return Err(anyhow!("FFmpeg is required for long-form assembly"));
            }
            let list = output.with_extension("concat.txt");
            fs::write(&list, concat_list_content(&list, &inputs)).await?;
            let status = Command::new("ffmpeg")
                .args(["-y", "-f", "concat", "-safe", "0", "-i"])
                .arg(&list)
                .args(["-c", "copy"])
                .arg(&output)
                .status()
                .await?;
            fs::remove_file(list).await.ok();
            if !status.success() {
                return Err(anyhow!("FFmpeg assembly failed"));
            }
        }
    }
    Ok(())
}
