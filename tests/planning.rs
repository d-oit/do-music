//! Deterministic tests for prompt parsing, duration planning and section
//! prompts — no live API calls.
//!
//! See `.agents/skills/duration-planning` and `.agents/skills/prompt-evaluator`.

use do_music::duration::Duration;
use do_music::plan::{MAX_TRACK_SECONDS, MusicSpec, SECTION_PHASES, plan_tracks, section_prompt};

fn spec() -> MusicSpec {
    MusicSpec {
        intent: "calm study music".into(),
        genre: vec!["ambient".into()],
        mood: vec!["calm".into()],
        tempo: "slow".into(),
        instruments: vec!["piano".into()],
        vocals: "none".into(),
        energy: "low".into(),
        arrangement: "minimal, looping".into(),
        style_prompt: "warm ambient piano, soft pads".into(),
        lyrics: "[Inst]".into(),
        score: 8.0,
    }
}

#[test]
fn duration_parse_accepts_valid_boundaries() {
    assert_eq!(Duration::parse("3m").unwrap().seconds(), 180);
    assert_eq!(Duration::parse("60m").unwrap().seconds(), 3600);
    assert_eq!(Duration::parse("180s").unwrap().seconds(), 180);
    assert_eq!(Duration::parse("3600s").unwrap().seconds(), 3600);
}

#[test]
fn duration_parse_accepts_whitespace_and_case() {
    assert_eq!(Duration::parse("  4M ").unwrap().seconds(), 240);
}

#[test]
fn duration_parse_rejects_out_of_range() {
    for bad in ["2m", "61m", "179s", "3601s", "0m"] {
        assert!(Duration::parse(bad).is_err(), "{bad} should be rejected");
    }
}

#[test]
fn duration_parse_rejects_garbage() {
    for bad in ["", "abc", "10", "1h", "m", "4"] {
        assert!(Duration::parse(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn duration_minutes_floors() {
    assert_eq!(Duration::parse("3m").unwrap().minutes(), 3);
    assert_eq!(Duration::parse("3600s").unwrap().minutes(), 60);
}

#[test]
fn plan_single_track_below_ten_minutes() {
    assert_eq!(plan_tracks(Duration::parse("3m").unwrap()), vec![180]);
    assert_eq!(plan_tracks(Duration::parse("9m").unwrap()), vec![540]);
}

#[test]
fn plan_splits_at_ten_minutes() {
    assert_eq!(
        plan_tracks(Duration::parse("10m").unwrap()),
        vec![240, 240, 120]
    );
}

#[test]
fn plan_sixty_minutes_is_fifteen_chunks() {
    let tracks = plan_tracks(Duration::parse("60m").unwrap());
    assert_eq!(tracks.len(), 15);
    assert!(tracks.iter().all(|&t| t == MAX_TRACK_SECONDS));
    assert_eq!(tracks.iter().sum::<u64>(), 3600);
}

#[test]
fn plan_invariant_chunks_and_total() {
    for minutes in 3..=60 {
        let duration = Duration::parse(&format!("{minutes}m")).unwrap();
        let tracks = plan_tracks(duration);
        // Always: the sum is exactly the requested total.
        assert_eq!(tracks.iter().sum::<u64>(), minutes * 60);
        if minutes < 10 {
            // Short form is a single track, which may exceed the chunk cap.
            assert_eq!(tracks, vec![duration.seconds()]);
        } else {
            // Long form splits into chunks capped at MAX_TRACK_SECONDS.
            assert!(
                tracks.iter().all(|&t| t <= MAX_TRACK_SECONDS),
                "{minutes}m produced an over-long chunk"
            );
        }
    }
}

#[test]
fn section_prompt_names_position_and_phase() {
    let p = section_prompt(&spec(), 0, 3);
    assert!(p.contains("warm ambient piano, soft pads"));
    assert!(p.contains("section 1/3"));
    assert!(p.contains("opening phase"));
    assert!(p.contains("minimal, looping"));
}

#[test]
fn section_prompt_phases_wrap() {
    for (i, phase) in SECTION_PHASES.iter().enumerate() {
        let p = section_prompt(&spec(), i, SECTION_PHASES.len());
        assert!(p.contains(&format!("{phase} phase")), "phase {phase}");
    }
    // Beyond the palette the last phase repeats; it must never panic.
    let p = section_prompt(&spec(), 99, 100);
    assert!(p.contains("section 100/100"));
}
