//! `visual` command plumbing: Python availability check, embedded
//! visualizer package materialization, and arg building. No planning
//! logic and no network I/O here — pure packaging.

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};

/// One CLI flag for the python visualizer, in clap-ValueEnum string form.
pub struct VisualStyle(pub &'static str);

pub const STYLE_CHOICES: &[VisualStyle] = &[
    VisualStyle("flow"),
    VisualStyle("bloom"),
    VisualStyle("plasma"),
    VisualStyle("waves"),
    VisualStyle("rings"),
    VisualStyle("spectrum"),
    VisualStyle("kaleido"),
];

/// Supported `--palette` choices; must match python `PALETTES` keys.
pub const PALETTE_CHOICES: &[&str] = &["zen", "ink", "abyss", "ember", "aurora", "neon"];

/// Supported output codecs; H.264 is the YouTube-compatible default.
pub const CODEC_CHOICES: &[&str] = &["h264", "hevc", "av1"];

/// Embedded package source, grouped per file (path inside `visualizer/`).
const PACKAGE_FILES: &[(&str, &str)] = &[
    ("__init__.py", include_str!("../visualizer/__init__.py")),
    ("__main__.py", include_str!("../visualizer/__main__.py")),
    ("analysis.py", include_str!("../visualizer/analysis.py")),
    ("cli.py", include_str!("../visualizer/cli.py")),
    ("highlights.py", include_str!("../visualizer/highlights.py")),
    ("palette.py", include_str!("../visualizer/palette.py")),
    ("styles.py", include_str!("../visualizer/styles.py")),
    ("validate.py", include_str!("../visualizer/validate.py")),
];

/// Test accessor: the embedded package manifest (for integration tests).
pub fn package_files() -> &'static [(&'static str, &'static str)] {
    PACKAGE_FILES
}

/// Root directory of the checked-out package (for running from source).
pub fn source_package_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("visualizer")
}

/// Ensure a writable `visualizer/` package exists and return its parent
/// (to use as `PYTHONPATH`). Reuses the in-tree package when present;
/// otherwise materializes the embedded sources under the target dir.
pub fn ensure_package() -> Result<PathBuf> {
    let src = source_package_dir();
    let probe = src.join("cli.py");
    if probe.is_file() {
        return Ok(src
            .parent()
            .context("visualizer package dir has no parent")?
            .to_path_buf());
    }
    let base = std::env::temp_dir().join("do-music-visualizer");
    let dst = base.join("visualizer");
    for (name, body) in PACKAGE_FILES {
        let path = dst.join(name);
        if path.is_file() && fs::read_to_string(&path)? == *body {
            continue;
        }
        fs::create_dir_all(&dst)?;
        fs::write(&path, body).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(base)
}

/// Video encoding options passed to the embedded visualizer.
#[derive(Debug, Clone, Copy)]
pub struct EncodeOptions<'a> {
    pub codec: &'a str,
    pub preset: &'a str,
    /// Feature multiplier before the style EMA (1.0 = neutral).
    pub sensitivity: f64,
    /// Palette brightness gain after the style render (1.0 = neutral).
    pub gain: f64,
}

/// Build the `python3 -m visualizer` argv. `wav` must already be a
/// 22050 Hz mono PCM file.
pub fn python_args(
    wav: &Path,
    style: &str,
    palette: &str,
    mirror: bool,
    seed: u64,
    encoding: EncodeOptions<'_>,
    out: &Path,
) -> Vec<String> {
    let mut args = vec![
        "-m".to_string(),
        "visualizer".to_string(),
        wav.display().to_string(),
        "--style".to_string(),
        style.to_string(),
        "--palette".to_string(),
        palette.to_string(),
        "--seed".to_string(),
        seed.to_string(),
    ];
    if mirror {
        args.push("--mirror".to_string());
    }
    args.push("--preset".to_string());
    args.push(encoding.preset.to_string());
    args.push("--codec".to_string());
    args.push(encoding.codec.to_string());
    if encoding.sensitivity != 1.0 {
        args.push("--sensitivity".to_string());
        args.push(format!("{}", encoding.sensitivity));
    }
    if encoding.gain != 1.0 {
        args.push("--gain".to_string());
        args.push(format!("{}", encoding.gain));
    }
    args.push("-o".to_string());
    args.push(out.display().to_string());
    args
}

/// Build the `python3 -m visualizer.highlights` argv for `wav`, writing the
/// highlight JSON document to `out`.
pub fn highlight_args(wav: &Path, out: &Path) -> Vec<String> {
    vec![
        "-m".to_string(),
        "visualizer.highlights".to_string(),
        wav.display().to_string(),
        "-o".to_string(),
        out.display().to_string(),
    ]
}

/// Verify `python3` can import numpy; return its version banner.
pub fn check_python() -> Result<String> {
    let out = std::process::Command::new("python3")
        .args(["-c", "import numpy; print(numpy.__version__)"])
        .output()
        .map_err(|e| anyhow!("python3 not found ({e}); the visual engine needs python3 + numpy"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "python3 cannot import numpy ({}); the visual engine needs python3 + numpy",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_args_contains_flags_and_omits_mirror() {
        let args = python_args(
            Path::new("/tmp/a.wav"),
            "flow",
            "zen",
            false,
            42,
            EncodeOptions {
                codec: "h264",
                preset: "medium",
                sensitivity: 1.0,
                gain: 1.0,
            },
            Path::new("/tmp/v.mp4"),
        );
        let text = args.join(" ");
        for flag in [
            "-m visualizer",
            "--style flow",
            "--palette zen",
            "--seed 42",
            "--preset medium",
            "--codec h264",
            "-o /tmp/v.mp4",
        ] {
            assert!(text.contains(flag), "missing {flag} in {text}");
        }
        assert!(
            !text.contains("--mirror"),
            "mirror must be omitted when false"
        );
        // Neutral sensitivity/gain must not be forwarded to python.
        assert!(!text.contains("--sensitivity"));
        assert!(!text.contains("--gain"));
    }

    #[test]
    fn python_args_forwards_sensitivity_and_gain_when_nonneutral() {
        let args = python_args(
            Path::new("/tmp/a.wav"),
            "kaleido",
            "neon",
            true,
            7,
            EncodeOptions {
                codec: "av1",
                preset: "fast",
                sensitivity: 1.5,
                gain: 0.8,
            },
            Path::new("/tmp/v.mp4"),
        );
        let text = args.join(" ");
        assert!(text.contains("--sensitivity 1.5"), "text: {text}");
        assert!(text.contains("--gain 0.8"), "text: {text}");
        assert!(text.contains("--mirror"));
    }

    #[test]
    fn highlight_args_target_the_highlights_module() {
        let args = highlight_args(Path::new("/tmp/a.wav"), Path::new("/tmp/marks.json"));
        let text = args.join(" ");
        assert!(text.contains("-m visualizer.highlights"), "{text}");
        assert!(text.contains("/tmp/a.wav"));
        assert!(text.contains("-o /tmp/marks.json"));
    }

    #[test]
    fn embedded_package_files_are_nonempty() {
        for (name, body) in PACKAGE_FILES {
            assert!(!body.is_empty(), "{name} embedded empty");
            assert!(body.len() > 50, "{name} suspiciously small");
        }
    }
}
