//! Template registry contract tests - no network.

use do_music::templates::{builtin_templates, load_all_from, starter_json, user_template_path};
use std::path::Path;

#[test]
fn builtins_are_five_with_expected_names_and_kinds() {
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
    // Four instrumentals + one vocal preset.
    assert_eq!(builtins.iter().filter(|t| t.instrumental).count(), 4);
}

#[test]
fn load_all_from_overrides_builtin_and_skips_corrupt() {
    let dir = tempfile::tempdir().unwrap();
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
fn starter_json_round_trips_into_registry() {
    let dir = tempfile::tempdir().unwrap();
    let path = user_template_path(dir.path(), "test-x");
    std::fs::write(&path, starter_json("test-x").unwrap()).unwrap();
    let all = load_all_from(dir.path());
    assert!(all.iter().any(|t| t.name == "test-x" && !t.builtin));
    assert!(starter_json("").is_err());
}
