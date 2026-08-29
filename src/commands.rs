//! Standalone command handlers (setup, providers, optimize, template) plus
//! template resolution shared by `mix`/`video`. Split out of `main.rs` to
//! keep the CLI entry under the 500-line rule.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use do_music::api::PROVIDER_TABLE;
use do_music::config::{self, Settings};
use do_music::providers;
use do_music::render;
use do_music::templates;
use tokio::fs;

/// Manage generation templates (builtin + user JSON).
#[derive(Subcommand, Debug)]
pub enum TemplateCommand {
    /// List builtin and user templates.
    List,
    /// Print one template as pretty JSON.
    Show { name: String },
    /// Write a starter user template JSON.
    New { name: String },
}

/// Apply a template: fills any of prompt/duration/instrumental the caller
/// left as defaults; explicit CLI args always win.
pub fn resolve_template(
    name: Option<String>,
    prompt: Option<String>,
    duration: Option<String>,
    default_duration: &str,
    instrumental: bool,
) -> Result<(String, String, bool)> {
    let Some(name) = name else {
        return Ok((
            prompt.ok_or_else(|| anyhow!("missing prompt (or pass --template)"))?,
            duration.unwrap_or_else(|| default_duration.to_string()),
            instrumental,
        ));
    };
    let template = templates::find(&name)
        .ok_or_else(|| anyhow!("unknown template {name:?} (try `do-music template list`)"))?;
    let prompt = prompt.unwrap_or_else(|| template.prompt.clone());
    let duration = duration.unwrap_or_else(|| template.duration.clone());
    let instrumental = instrumental || template.instrumental && prompt == template.prompt;
    Ok((prompt, duration, instrumental))
}

/// With `--optimize`: rewrite the prompt via the LLM and print the result.
pub async fn maybe_optimize(prompt: &str, optimize: bool) -> Result<String> {
    if !optimize {
        return Ok(prompt.to_string());
    }
    let settings = Settings::load()?;
    let rewritten = chat_rewrite(&settings, prompt).await?;
    println!("✓ optimized: {rewritten}");
    Ok(rewritten)
}

/// One plain-text LLM rewrite at temperature 0.3.
async fn chat_rewrite(settings: &Settings, prompt: &str) -> Result<String> {
    providers::chat_completion(
        &settings.api_key,
        &settings.llm_base_url,
        &settings.llm_model,
        providers::OPTIMIZE_SYSTEM_PROMPT,
        prompt,
        0.3,
        false,
    )
    .await
}

/// `setup`: resolve key, probe providers, write `.env`, print next steps.
pub async fn run_setup(
    api_key: Option<String>,
    llm_model: Option<String>,
    music_model: Option<String>,
    video_model: Option<String>,
) -> Result<()> {
    let env_path = std::path::Path::new(".env");
    let mut values = if env_path.exists() {
        config::load_env_file(env_path).unwrap_or_default()
    } else {
        Default::default()
    };
    // Key resolution: --api-key > environment > existing .env value.
    let key = match api_key {
        Some(k) if !k.trim().is_empty() => k,
        _ => std::env::var("GMI_API_KEY")
            .ok()
            .or_else(|| values.get("GMI_API_KEY").cloned())
            .filter(|k| !k.trim().is_empty())
            .context("no API key found: set GMI_API_KEY or pass --api-key")?,
    };
    values.insert("GMI_API_KEY".into(), key.clone());
    if let Some(m) = llm_model {
        values.insert("GMI_LLM_MODEL".into(), m);
    }
    if let Some(m) = music_model {
        values.insert("GMI_MUSIC_MODEL".into(), m);
    }
    if let Some(m) = video_model {
        values.insert("GMI_VIDEO_MODEL".into(), m);
    }
    values
        .entry("GMI_LLM_MODEL".into())
        .or_insert_with(|| "MiniMaxAI/MiniMax-M2.7".into());
    values
        .entry("GMI_MUSIC_MODEL".into())
        .or_insert_with(|| "minimax-music-3.0".into());
    values
        .entry("GMI_VIDEO_MODEL".into())
        .or_insert_with(|| "MiniMax-H3".into());
    values
        .entry("GMI_LLM_BASE_URL".into())
        .or_insert_with(|| "https://api.gmi-serving.com/v1".into());
    values.entry("GMI_MUSIC_URL".into()).or_insert_with(|| {
        "https://console.gmicloud.ai/api/v1/ie/requestqueue/apikey/requests".into()
    });

    // Probe with the resolved values before persisting.
    let settings = Settings {
        api_key: key,
        llm_base_url: values
            .get("GMI_LLM_BASE_URL")
            .cloned()
            .unwrap_or_else(|| "https://api.gmi-serving.com/v1".into()),
        llm_model: values
            .get("GMI_LLM_MODEL")
            .cloned()
            .unwrap_or_else(|| "MiniMaxAI/MiniMax-M2.7".into()),
        music_url: values.get("GMI_MUSIC_URL").cloned().unwrap_or_else(|| {
            "https://console.gmicloud.ai/api/v1/ie/requestqueue/apikey/requests".into()
        }),
        music_model: values
            .get("GMI_MUSIC_MODEL")
            .cloned()
            .unwrap_or_else(|| "minimax-music-3.0".into()),
        video_model: values
            .get("GMI_VIDEO_MODEL")
            .cloned()
            .unwrap_or_else(|| "MiniMax-H3".into()),
    };
    println!("Probing providers...");
    let statuses = providers::probe_providers(&settings).await;
    print_provider_rows(&statuses);

    config::write_env_file(env_path, &values)?;
    println!("✓ wrote {}", env_path.display());
    println!("Next: do-music template list | do-music mix --template calm-monk");
    Ok(())
}

/// `providers`: metadata table plus live probe results.
pub async fn run_providers() -> Result<()> {
    println!("{:<26} {:<7} {:<45} FREE", "MODEL", "KIND", "PRICE");
    for (model, kind, price, campaign_free) in PROVIDER_TABLE {
        println!(
            "{model:<26} {kind:<7} {price:<45} {}",
            if *campaign_free { "yes" } else { "no" }
        );
    }
    let settings = Settings::load()?;
    println!("\nLive probe:");
    let statuses = providers::probe_providers(&settings).await;
    print_provider_rows(&statuses);
    Ok(())
}

/// One aligned table row per probe status (secret-free by construction).
fn print_provider_rows(statuses: &[providers::ProviderStatus]) {
    for row in statuses {
        println!("{:<26} {:<7} {}", row.model, row.kind, row.status);
    }
}

/// `visual`: generative audio-reactive video from an existing track.
///
/// Probe the audio, materialize it as 22050 Hz mono PCM WAV (numpy-only
/// analysis), run the embedded python visualizer (frames piped to
/// ffmpeg x265), then mux the video with the original audio.
pub async fn run_visual(
    audio: &Path,
    style: &str,
    palette: &str,
    mirror: bool,
    seed: u64,
    output: Option<PathBuf>,
) -> Result<()> {
    use do_music::visual;

    if !audio.exists() {
        return Err(anyhow!("audio not found: {}", audio.display()));
    }
    if !visual::STYLE_CHOICES.iter().any(|s| s.0 == style) {
        return Err(anyhow!(
            "unknown --style {style}; choices: flow, bloom, plasma"
        ));
    }
    if !visual::PALETTE_CHOICES.contains(&palette) {
        return Err(anyhow!(
            "unknown --palette {palette}; choices: zen, ink, abyss, ember"
        ));
    }
    visual::check_python()?;
    let package_parent = visual::ensure_package()?;

    let total_secs = render::probe_duration(audio).await?;
    println!(
        "Visualizing {} ({:.1}s) as {style}/{palette}",
        audio.display(),
        total_secs
    );

    // Isolated work dir: 22050 Hz mono WAV + raw x265 video (no audio).
    let work_seed = seed;
    let work = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        let base = std::env::temp_dir().join(format!("do-music-visual-{work_seed}"));
        std::fs::create_dir_all(&base)?;
        Ok(base)
    })
    .await
    .context("work dir task panicked")??;
    let wav = work.join("audio.wav");
    let raw_video = work.join("visual-video-only.mp4");
    render::materialize_pcm_wav(audio, &wav).await?;

    // Long tracks use the cheaper x265 preset (plan contingency: >40 min
    // encode budget); previews keep medium.
    let preset = if total_secs > 120.0 { "fast" } else { "medium" };
    let args = visual::python_args(&wav, style, palette, mirror, seed, &raw_video, preset);
    let status = tokio::process::Command::new("python3")
        .args(&args)
        .env("PYTHONPATH", &package_parent)
        .status()
        .await
        .context("failed to spawn python3")?;
    if !status.success() {
        let _ = fs::remove_dir_all(&work).await;
        return Err(anyhow!("visualizer exited with {status}"));
    }

    let out = match output {
        Some(p) => p,
        None => PathBuf::from("do-music-output")
            .join(format!(
                "visual-{style}-{palette}-{}",
                audio
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("track")
            ))
            .with_extension("mp4"),
    };
    render::mux_video_audio(&raw_video, audio, &out, total_secs).await?;
    let _ = fs::remove_dir_all(&work).await;
    println!("Done: {}", out.display());
    Ok(())
}

/// `optimize`: one plain-text LLM rewrite of the user's idea.
pub async fn run_optimize(prompt: &str, json: bool) -> Result<()> {
    let settings = Settings::load()?;
    let rewritten = chat_rewrite(&settings, prompt).await?;
    if json {
        println!(
            "{}",
            serde_json::json!({"prompt": rewritten, "model": settings.llm_model})
        );
    } else {
        println!("{rewritten}");
    }
    Ok(())
}

/// Template subcommands: list / show / new.
pub fn run_template(cmd: TemplateCommand) -> Result<()> {
    match cmd {
        TemplateCommand::List => {
            println!("{:<18} {:<7} {:<13} SOURCE", "NAME", "DUR", "INSTRUMENTAL");
            for t in templates::load_all() {
                println!(
                    "{:<18} {:<7} {:<13} {}",
                    t.name,
                    t.duration,
                    if t.instrumental { "yes" } else { "no" },
                    if t.builtin { "builtin" } else { "user" }
                );
            }
            Ok(())
        }
        TemplateCommand::Show { name } => {
            let t = templates::find(&name).ok_or_else(|| {
                anyhow!("unknown template {name:?} (try `do-music template list`)")
            })?;
            println!("{}", serde_json::to_string_pretty(&t)?);
            Ok(())
        }
        TemplateCommand::New { name } => {
            if templates::find(&name).is_some() {
                anyhow::bail!("template {name:?} already exists");
            }
            let dir = templates::user_templates_dir();
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
            let path = templates::user_template_path(&dir, &name);
            std::fs::write(&path, templates::starter_json(&name)?)?;
            println!("created {}", path.display());
            Ok(())
        }
    }
}
