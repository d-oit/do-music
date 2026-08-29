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
