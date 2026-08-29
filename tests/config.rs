//! `.env` parse/merge/serialize contract tests - no network, no global env
//! mutation.

use do_music::config::{load_env_file, parse_env_text, write_env_file};
use std::collections::BTreeMap;
use std::path::Path;

#[test]
fn parse_env_text_handles_comments_quotes_and_blanks() {
    let text = "# leading comment\n\nGMI_API_KEY=abc123\nGMI_LLM_MODEL=\"MiniMaxAI/MiniMax-M3\"\nSINGLE='quoted'\nnot-a-pair\n=nokey\n";
    let map = parse_env_text(text);
    assert_eq!(map.get("GMI_API_KEY").map(String::as_str), Some("abc123"));
    assert_eq!(
        map.get("GMI_LLM_MODEL").map(String::as_str),
        Some("MiniMaxAI/MiniMax-M3")
    );
    assert_eq!(map.get("SINGLE").map(String::as_str), Some("quoted"));
    assert_eq!(map.len(), 3);
}

#[test]
fn load_env_file_reads_real_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".env");
    std::fs::write(&path, "A=1\n# c\nB=\"two\"\n").unwrap();
    let map = load_env_file(&path).unwrap();
    assert_eq!(map.get("A").map(String::as_str), Some("1"));
    assert_eq!(map.get("B").map(String::as_str), Some("two"));
    assert!(load_env_file(Path::new("/nonexistent/.env")).is_err());
}

#[test]
fn write_env_file_refuses_empty_key_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".env");
    let mut values = BTreeMap::new();
    values.insert("GMI_API_KEY".to_string(), String::new());
    assert!(write_env_file(&path, &values).is_err());

    values.insert("GMI_API_KEY".to_string(), "k".to_string());
    values.insert("GMI_VIDEO_MODEL".to_string(), "MiniMax-H3".to_string());
    write_env_file(&path, &values).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    // BTreeMap iterates sorted: GMI_API_KEY < GMI_VIDEO_MODEL.
    assert_eq!(text, "GMI_API_KEY=k\nGMI_VIDEO_MODEL=MiniMax-H3\n");
    assert_eq!(parse_env_text(&text), values);
}

#[test]
fn merged_env_layers_real_environment_over_file() {
    let mut file_map = BTreeMap::new();
    file_map.insert("GMI_LLM_MODEL".into(), "from-file".into());
    file_map.insert("DO_MUSIC_ONLY".into(), "file".into());
    let merged = do_music::config::merged_env(&file_map);
    // Real env vars all present.
    for (k, v) in std::env::vars() {
        assert_eq!(merged.get(&k).map(String::as_str), Some(v.as_str()));
    }
    // File-only key survives.
    assert_eq!(
        merged.get("DO_MUSIC_ONLY").map(String::as_str),
        Some("file")
    );
}
