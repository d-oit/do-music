//! Generation templates: five builtins plus user JSON overrides.
//!
//! Pure logic (`load_all_from`) is offline-testable; `load_all` is a thin
//! wrapper over the user templates directory. Layer rule: no network here.

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One generation preset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Template {
    pub name: String,
    pub prompt: String,
    /// Duration string in the CLI's `10m`-style syntax (validated at use).
    pub duration: String,
    pub instrumental: bool,
    /// True for the five shipped presets; user copies set this to false.
    /// Defaults to false so user JSON may omit it.
    #[serde(default)]
    pub builtin: bool,
}

/// The five shipped presets (exact contents per plan).
///
/// A function rather than a `const`: `String` construction is not const.
pub fn builtin_templates() -> Vec<Template> {
    vec![
        Template {
            name: "calm-monk".to_string(),
            prompt: "calm zen meditation, soft piano and ambient pads, gentle lake-morning atmosphere, 60 bpm, spacious reverb".to_string(),
            duration: "10m".to_string(),
            instrumental: true,
            builtin: true,
        },
        Template {
            name: "lofi-study".to_string(),
            prompt: "warm lo-fi hip hop beat, dusty vinyl texture, mellow electric piano, soft swing, 75 bpm".to_string(),
            duration: "25m".to_string(),
            instrumental: true,
            builtin: true,
        },
        Template {
            name: "deep-focus".to_string(),
            prompt: "minimal ambient focus music, one steady soft synth pad, no melody, very low movement, 50 bpm".to_string(),
            duration: "45m".to_string(),
            instrumental: true,
            builtin: true,
        },
        Template {
            name: "sleep-drift".to_string(),
            prompt: "slow ambient sleep soundscape, warm analog drones, faint tape hiss, no rhythm, long gradual fades".to_string(),
            duration: "60m".to_string(),
            instrumental: true,
            builtin: true,
        },
        Template {
            name: "cinematic-rise".to_string(),
            prompt: "epic cinematic orchestral build, soaring strings and taiko drums, hopeful climax with choir swell".to_string(),
            duration: "4m".to_string(),
            instrumental: false,
            builtin: true,
        },
    ]
}

/// Builtins plus every valid `*.json` in `dir`.
///
/// A corrupt user file is skipped, not fatal. A user entry with the same
/// name as a builtin **overrides** it and keeps `builtin: false`.
pub fn load_all_from(dir: &Path) -> Vec<Template> {
    let mut templates: Vec<Template> = builtin_templates();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return templates;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut template) = serde_json::from_str::<Template>(&text) else {
            continue;
        };
        template.builtin = false;
        if let Some(slot) = templates.iter_mut().find(|t| t.name == template.name) {
            *slot = template;
        } else {
            templates.push(template);
        }
    }
    templates
}
/// User templates live under `$HOME/.config/do-music/templates`; falls
/// back to `./.do-music-templates` when `HOME` is unset.
pub fn user_templates_dir() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => PathBuf::from(home).join(".config/do-music/templates"),
        _ => PathBuf::from(".do-music-templates"),
    }
}

/// Builtins plus user templates from the default directory.
pub fn load_all() -> Vec<Template> {
    load_all_from(&user_templates_dir())
}

/// Look up one template by name (user overrides win via `load_all_from`).
pub fn find(name: &str) -> Option<Template> {
    load_all().into_iter().find(|t| t.name == name)
}

/// Starter JSON for `template new`.
pub fn starter_json(name: &str) -> Result<String> {
    if name.trim().is_empty() {
        return Err(anyhow!("template name must not be empty"));
    }
    serde_json::to_string_pretty(&Template {
        name: name.to_string(),
        prompt: "describe the music: genre, instruments, tempo, mood".to_string(),
        duration: "4m".to_string(),
        instrumental: false,
        builtin: false,
    })
    .context("cannot serialize starter template")
}

/// Path of the user JSON file for `name` inside `dir`.
pub fn user_template_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_five_with_expected_names() {
        let builtins = builtin_templates();
        let names: Vec<&str> = builtins.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "calm-monk",
                "lofi-study",
                "deep-focus",
                "sleep-drift",
                "cinematic-rise"
            ]
        );
        assert!(builtins.iter().all(|t| t.builtin));
    }

    #[test]
    fn load_all_from_overrides_builtin_and_skips_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        // Valid override of calm-monk + a corrupt file + a non-json file.
        std::fs::write(
            dir.path().join("calm-monk.json"),
            r#"{"name":"calm-monk","prompt":"user version","duration":"5m","instrumental":false}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("broken.json"), "{not json").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignore me").unwrap();
        let all = load_all_from(dir.path());
        assert_eq!(all.len(), 5);
        let calm = all.iter().find(|t| t.name == "calm-monk").unwrap();
        assert_eq!(calm.prompt, "user version");
        assert!(!calm.builtin);
    }

    #[test]
    fn load_all_from_adds_new_user_templates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mine.json"),
            r#"{"name":"mine","prompt":"p","duration":"3m","instrumental":true}"#,
        )
        .unwrap();
        let all = load_all_from(dir.path());
        assert_eq!(all.len(), 6);
        let mine = all.iter().find(|t| t.name == "mine").unwrap();
        assert!(!mine.builtin);
    }

    #[test]
    fn load_all_from_missing_dir_returns_builtins() {
        let all = load_all_from(Path::new("/nonexistent/do-music-templates"));
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn starter_json_round_trips_through_load_all_from() {
        let dir = tempfile::tempdir().unwrap();
        let path = user_template_path(dir.path(), "test-x");
        std::fs::write(&path, starter_json("test-x").unwrap()).unwrap();
        let all = load_all_from(dir.path());
        assert_eq!(all.len(), 6);
        assert!(all.iter().any(|t| t.name == "test-x" && !t.builtin));
    }
}
