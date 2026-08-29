//! Configuration: `.env` loading and persisted settings.
//!
//! Pure logic here (parse, merge, serialize); nothing performs I/O beyond
//! the two documented file touchpoints (`apply_env_file`, `write_env_file`)
//! and the env lookups in [`Settings::load`]. Layer rule: main.rs calls
//! `apply_env_file()` before anything else; env vars always beat `.env`.

use anyhow::{Context, Result, anyhow};
use std::collections::BTreeMap;
use std::path::Path;

/// Parse a `.env`-style file into `KEY=VALUE` pairs.
///
/// Skips blank lines and `#` comments, trims whitespace, strips one pair of
/// surrounding single or double quotes from values. Later duplicates win.
pub fn load_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(parse_env_text(&text))
}

/// Pure `.env` text parser (unit-testable core of `load_env_file`).
pub fn parse_env_text(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        let mut value = value.trim();
        for q in ['"', '\''] {
            if value.len() >= 2 && value.starts_with(q) && value.ends_with(q) {
                value = &value[1..value.len() - 1];
                break;
            }
        }
        map.insert(key, value.to_string());
    }
    map
}

/// Layer `std::env` over a file map: env wins, file fills the gaps.
///
/// Keys present in the real environment keep their env value; the rest fall
/// back to `file_map`. Pure helper so precedence is testable without
/// spawning processes or mutating global env.
pub fn merged_env(file_map: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut merged = file_map.clone();
    for (key, value) in std::env::vars() {
        merged.insert(key, value);
    }
    merged
}

/// Apply `./.env` (if present) to the process environment.
///
/// Only keys **not already set** in the environment are applied: env vars
/// always beat `.env`. Called once, first thing in `main`, before any
/// threads exist, so mutating the environment is sound.
pub fn apply_env_file() {
    let Ok(map) = load_env_file(Path::new(".env")) else {
        return;
    };
    for (key, value) in map {
        if std::env::var_os(&key).is_none() {
            // Safety: single-threaded at this point (first call in main).
            unsafe { std::env::set_var(&key, &value) };
        }
    }
}

/// Write sorted `KEY=VALUE` lines. Refuses to persist an empty `GMI_API_KEY`.
///
/// Values are never logged anywhere by this module.
pub fn write_env_file(path: &Path, values: &BTreeMap<String, String>) -> Result<()> {
    if values
        .get("GMI_API_KEY")
        .is_some_and(|v| v.trim().is_empty())
    {
        return Err(anyhow!("refusing to write an empty GMI_API_KEY"));
    }
    let mut body = String::new();
    for (key, value) in values {
        body.push_str(key);
        body.push('=');
        body.push_str(value);
        body.push('\n');
    }
    std::fs::write(path, body).with_context(|| format!("cannot write {}", path.display()))
}

/// Resolved runtime configuration (all values from env, with defaults).
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub api_key: String,
    pub llm_base_url: String,
    pub llm_model: String,
    pub music_url: String,
    pub music_model: String,
    pub video_model: String,
}

impl Settings {
    /// Read settings from the environment. `GMI_API_KEY` is required.
    pub fn load() -> Result<Self> {
        Ok(Self {
            api_key: env_required("GMI_API_KEY")?,
            llm_base_url: env_or("GMI_LLM_BASE_URL", "https://api.gmi-serving.com/v1"),
            llm_model: env_or("GMI_LLM_MODEL", "MiniMaxAI/MiniMax-M2.7"),
            music_url: env_or(
                "GMI_MUSIC_URL",
                "https://console.gmicloud.ai/api/v1/ie/requestqueue/apikey/requests",
            ),
            music_model: env_or("GMI_MUSIC_MODEL", "minimax-music-3.0"),
            video_model: env_or("GMI_VIDEO_MODEL", "MiniMax-H3"),
        })
    }
}

fn env_required(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("{key} is not set"))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_text_skips_comments_blanks_and_keeps_quotes_off() {
        let text = "# comment\n\nGMI_API_KEY=abc\nGMI_LLM_MODEL=\"MiniMaxAI/MiniMax-M3\"\nQ='single'\nBAD LINE WITHOUT EQUALS\n=emptykey\n";
        let map = parse_env_text(text);
        assert_eq!(map.get("GMI_API_KEY").map(String::as_str), Some("abc"));
        assert_eq!(
            map.get("GMI_LLM_MODEL").map(String::as_str),
            Some("MiniMaxAI/MiniMax-M3")
        );
        assert_eq!(map.get("Q").map(String::as_str), Some("single"));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn parse_env_text_later_duplicates_win() {
        let map = parse_env_text("A=1\nA=2\n");
        assert_eq!(map.get("A").map(String::as_str), Some("2"));
    }

    #[test]
    fn merged_env_prefers_real_environment() {
        let mut file_map = BTreeMap::new();
        file_map.insert("GMI_LLM_MODEL".to_string(), "file-model".to_string());
        file_map.insert("DO_MUSIC_TEST_ONLY".to_string(), "from-file".to_string());
        let merged = merged_env(&file_map);
        // The real env has no GMI_LLM_MODEL set in tests; file value stands.
        assert_eq!(
            merged.get("DO_MUSIC_TEST_ONLY").map(String::as_str),
            Some("from-file")
        );
        // Every real env var is present in the merged map.
        for (k, v) in std::env::vars() {
            assert_eq!(merged.get(&k).map(String::as_str), Some(v.as_str()));
        }
    }

    #[test]
    fn write_env_file_refuses_empty_api_key() {
        let mut values = BTreeMap::new();
        values.insert("GMI_API_KEY".to_string(), String::new());
        assert!(write_env_file(Path::new("/tmp/never-written.env"), &values).is_err());
    }

    #[test]
    fn write_env_file_round_trips_sorted_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let mut values = BTreeMap::new();
        values.insert("GMI_API_KEY".to_string(), "secret".to_string());
        values.insert("GMI_LLM_MODEL".to_string(), "m".to_string());
        write_env_file(&path, &values).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "GMI_API_KEY=secret\nGMI_LLM_MODEL=m\n");
        let back = parse_env_text(&text);
        assert_eq!(back, values);
    }
}
