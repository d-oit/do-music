//! CLI and orchestration only: every network transport lives in
//! `providers.rs`, every pure computation in the library modules, and the
//! video-command pipeline (scene planning → segments → transitions → mux)
//! lives in `video_pipeline.rs`.

mod commands;
mod video_inputs;
mod video_pipeline;
use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use do_music::assembly;
use do_music::config::{self, Settings};
use do_music::duration::Duration;
use do_music::plan;
use do_music::providers;
use std::path::PathBuf;
use tokio::fs;

#[derive(Parser, Debug)]
#[command(
    name = "do-music",
    version,
    about = "Tiny Suno-like CLI for MiniMax Music 3.0"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<CommandKind>,
    prompt: Option<String>,
    #[arg(long, default_value = "4m")]
    duration: String,
    #[arg(long)]
    instrumental: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, short)]
    output: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum CommandKind {
    /// Evaluate a prompt and print the structured MusicSpec JSON.
    Improve { prompt: String },
    /// Generate music (long-form = multiple tracks + local assembly).
    Mix {
        /// Positional prompt; omit when `--template` supplies it.
        prompt: Option<String>,
        /// Requested length; omit to take the template's duration.
        #[arg(long)]
        duration: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        instrumental: bool,
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Fill prompt/duration/instrumental from a named template.
        #[arg(long)]
        template: Option<String>,
        /// Rewrite the prompt with the LLM before generating.
        #[arg(long)]
        optimize: bool,
    },
    /// Generate the 10-minute highlight video (music + scenes + motion).
    Video {
        /// Positional prompt; omit when `--template` supplies it.
        prompt: Option<String>,
        /// Requested length; omit to take the template's duration.
        #[arg(long)]
        duration: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        instrumental: bool,
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// I2V model id on the GMI requestqueue.
        #[arg(long, default_value = "MiniMax-H3")]
        video_model: String,
        /// Force the ffmpeg breathing-zoom fallback even if H3 is available.
        #[arg(long)]
        no_i2v: bool,
        /// JSON scene list file: {"prompts":[...],"style":...,"negative":...}.
        /// One prompt per musical section; overrides the builtin monk arc.
        #[arg(long)]
        scenes: Option<PathBuf>,
        /// Reuse this audio track instead of generating one.
        #[arg(long)]
        audio: Option<PathBuf>,
        /// ffmpeg xfade transition between scenes (fade, fadewhite, ...).
        #[arg(long, default_value = "fade")]
        xfade: String,
        /// Parallel ffmpeg fallback segment renders; 0 = one per CPU core.
        #[arg(long, default_value = "0")]
        jobs: usize,
        /// Final video codec; H.264 is the YouTube-compatible default.
        #[arg(long, default_value = "h264")]
        video_codec: String,
        /// Render tier: fast (preview), balanced (default), high (archive).
        #[arg(long, default_value = "balanced")]
        quality: String,
        /// Cut scenes on even timings instead of snapping to musical highlights.
        #[arg(long)]
        no_highlights: bool,
        /// Skip LLM art direction and use the builtin monk scene arc.
        #[arg(long)]
        no_art_direction: bool,
        /// Ground art direction in live web research about the musical style.
        #[arg(long)]
        research: bool,
        /// Do not read or write the self-tuning render memory.
        #[arg(long)]
        no_autotune: bool,
        /// Seconds a scene cut may move to reach a highlight.
        #[arg(long, default_value = "8.0")]
        highlight_window: f64,
        /// Fill prompt/duration/instrumental from a named template.
        #[arg(long)]
        template: Option<String>,
        /// Rewrite the prompt with the LLM before generating.
        #[arg(long)]
        optimize: bool,
    },
    /// Render generative audio-reactive visuals for an existing track.
    Visual {
        /// Audio track to visualize (mp3/wav/...).
        audio: PathBuf,
        /// Generative engine.
        #[arg(long, default_value = "flow")]
        style: String,
        /// Color palette.
        #[arg(long, default_value = "zen")]
        palette: String,
        /// Horizontally mirror the frame (kaleidoscope feel).
        #[arg(long)]
        mirror: bool,
        /// Deterministic engine seed.
        #[arg(long, default_value = "42")]
        seed: u64,
        /// Video codec; H.264 is the YouTube-compatible default.
        #[arg(long, default_value = "h264")]
        codec: String,
        /// Feature multiplier before the style EMA (1.0 neutral).
        #[arg(long, default_value = "1.0")]
        sensitivity: f64,
        /// Palette brightness gain after the style render (1.0 neutral).
        #[arg(long, default_value = "1.0")]
        gain: f64,
        /// Output video path.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Configure `.env`: resolve the API key, probe providers, persist.
    Setup {
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        llm_model: Option<String>,
        #[arg(long)]
        music_model: Option<String>,
        #[arg(long)]
        video_model: Option<String>,
    },
    /// Show the provider table plus live probe results.
    Providers,
    /// Rewrite one prompt into a rich MiniMax Music 3.0 prompt.
    Optimize {
        prompt: String,
        /// Emit {"prompt": ..., "model": ...} instead of plain text.
        #[arg(long)]
        json: bool,
    },
    /// Manage generation templates (builtin + user JSON).
    Template {
        #[command(subcommand)]
        cmd: commands::TemplateCommand,
    },
}
#[tokio::main]
async fn main() -> Result<()> {
    config::apply_env_file();
    let cli = Cli::parse();
    match cli.command {
        Some(CommandKind::Setup {
            api_key,
            llm_model,
            music_model,
            video_model,
        }) => commands::run_setup(api_key, llm_model, music_model, video_model).await,
        Some(CommandKind::Providers) => commands::run_providers().await,
        Some(CommandKind::Optimize { prompt, json }) => commands::run_optimize(&prompt, json).await,
        Some(CommandKind::Template { cmd }) => commands::run_template(cmd),
        Some(CommandKind::Improve { prompt }) => {
            let settings = Settings::load()?;
            let spec = providers::evaluate(
                &settings.api_key,
                &settings.llm_base_url,
                &settings.llm_model,
                &prompt,
                false,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&spec)?);
            Ok(())
        }
        Some(CommandKind::Mix {
            prompt,
            duration,
            dry_run,
            output,
            instrumental,
            template,
            optimize,
        }) => {
            let (prompt, duration, instrumental) =
                commands::resolve_template(template, prompt, duration, "60m", instrumental)?;
            let prompt = commands::maybe_optimize(&prompt, optimize).await?;
            run_generation(&prompt, &duration, dry_run, instrumental, output).await
        }
        Some(CommandKind::Video {
            prompt,
            duration,
            dry_run,
            instrumental,
            output,
            video_model,
            no_i2v,
            scenes,
            audio,
            xfade,
            jobs,
            video_codec,
            quality,
            no_highlights,
            no_art_direction,
            research,
            no_autotune,
            highlight_window,
            template,
            optimize: _,
        }) => {
            let (prompt, duration, instrumental) = match (&audio, &prompt) {
                // With --audio the track exists; the prompt is unused.
                (Some(_), None) => (
                    String::new(),
                    duration.unwrap_or_else(|| "10m".into()),
                    instrumental,
                ),
                _ => commands::resolve_template(template, prompt, duration, "10m", instrumental)?,
            };
            let job = video_pipeline::VideoJob {
                prompt,
                duration,
                dry_run,
                instrumental,
                output,
                video_model,
                no_i2v,
                scenes,
                audio,
                xfade,
                jobs,
                video_codec,
                quality,
                highlights: !no_highlights,
                art_direct: !no_art_direction,
                research,
                autotune: !no_autotune,
                highlight_window,
            };
            video_pipeline::run_video(job).await
        }
        Some(CommandKind::Visual {
            audio,
            style,
            palette,
            mirror,
            seed,
            codec,
            sensitivity,
            gain,
            output,
        }) => {
            commands::run_visual(
                &audio,
                &style,
                &palette,
                mirror,
                seed,
                &codec,
                sensitivity,
                gain,
                output,
            )
            .await
        }
        None => {
            let prompt = cli.prompt.ok_or_else(|| anyhow!("missing prompt"))?;
            run_generation(
                &prompt,
                &cli.duration,
                cli.dry_run,
                cli.instrumental,
                cli.output,
            )
            .await
        }
    }
}

async fn run_generation(
    prompt: &str,
    duration: &str,
    dry_run: bool,
    instrumental: bool,
    output: Option<PathBuf>,
) -> Result<()> {
    let settings = Settings::load()?;
    let requested = Duration::parse(duration)?;
    println!("Analyzing prompt...");
    let mut spec = providers::evaluate(
        &settings.api_key,
        &settings.llm_base_url,
        &settings.llm_model,
        prompt,
        instrumental,
    )
    .await?;
    if instrumental {
        spec.vocals = "none".into();
        spec.lyrics = "[Inst]".into();
    }
    println!("v prompt score: {:.1}/10", spec.score);
    if spec.score < 3.0 {
        println!(
            "[WARN] low prompt score {:.1}/10 - the idea may be under-specified; consider adding genre, mood, instruments, or vocals detail",
            spec.score
        );
    }
    println!("v style: {}", spec.genre.join(" / "));
    println!("v vocals: {}", spec.vocals);

    let tracks = plan::plan_tracks(requested);
    println!("Planning {} section(s)...", tracks.len());
    for (i, seconds) in tracks.iter().enumerate() {
        println!(" [{}/{}] ~{}m", i + 1, tracks.len(), seconds / 60);
    }
    if dry_run {
        println!("Dry run: no music generation request sent.");
        return Ok(());
    }

    let dir = PathBuf::from("do-music-output");
    fs::create_dir_all(&dir).await?;
    let mut files = Vec::new();
    for (i, _) in tracks.iter().enumerate() {
        let section_prompt = plan::section_prompt(&spec, i, tracks.len());
        println!("Generating [{}/{}]...", i + 1, tracks.len());
        let path = dir.join(format!("track-{:02}.mp3", i + 1));
        providers::generate_music(
            &settings.api_key,
            &settings.music_url,
            &settings.music_model,
            &section_prompt,
            &spec.lyrics,
            &path,
        )
        .await?;
        files.push(path);
    }
    let out =
        output.unwrap_or_else(|| dir.join(assembly::default_output_name(requested.seconds())));
    // YouTube long-form (≥30 m) gets loudnorm + 8 s crossfades; short mixes keep the
    // fast lossless concat so a 4 m test stays bit-identical.
    let use_youtube = requested.seconds() >= 30 * 60 && files.len() > 1;
    if use_youtube {
        println!(
            "Assembling {} tracks with 8 s crossfades + loudnorm -14 LUFS (YouTube)…",
            files.len()
        );
        assembly::assemble_youtube(&files, &out).await?;
        // Chapters sidecar for YouTube description paste.
        let chapters = assembly::youtube_chapters(&tracks);
        let chap_path = out.with_extension("chapters.txt");
        tokio::fs::write(&chap_path, &chapters).await?;
        println!("Chapters: {}\n{}", chap_path.display(), chapters);
        // Keep track intermediates for debugging; delete with `rm do-music-output/track-*.mp3` when done.
        println!(
            "Tip: `rm do-music-output/track-*.mp3` to reclaim ~30 MB after verifying {}",
            out.display()
        );
    } else {
        assembly::assemble(&files, &out).await?;
    }
    println!("Done: {}", out.display());
    Ok(())
}
