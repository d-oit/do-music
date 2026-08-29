//! Deterministic tests for assembly *planning* — no FFmpeg invocation required
//! except the single-track copy path, which needs no external tools.
//!
//! See `.agents/skills/local-assembly`.

use std::path::{Path, PathBuf};

use do_music::assembly::{
    AssemblyStep, assemble, concat_list_content, default_output_name, plan_assembly,
};
use tempfile::tempdir;

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
fn single_file_plans_a_copy() {
    let step = plan_assembly(&[p("track-01.mp3")], &p("song.mp3")).unwrap();
    assert_eq!(
        step,
        AssemblyStep::Copy {
            from: p("track-01.mp3"),
            to: p("song.mp3")
        }
    );
}

#[test]
fn multiple_files_plan_a_concat_in_order() {
    let step = plan_assembly(&[p("a.mp3"), p("b.mp3"), p("c.mp3")], &p("mix.mp3")).unwrap();
    assert_eq!(
        step,
        AssemblyStep::Concat {
            inputs: vec![p("a.mp3"), p("b.mp3"), p("c.mp3")],
            output: p("mix.mp3")
        }
    );
}

#[test]
fn empty_input_is_an_error() {
    assert!(plan_assembly(&[], &p("out.mp3")).is_err());
}

#[test]
fn concat_list_paths_are_relative_to_list_file_directory() {
    let list = Path::new("do-music-output").join("song.concat.txt");
    let files = vec![
        PathBuf::from("do-music-output/track-01.mp3"),
        PathBuf::from("do-music-output/track 02.mp3"),
    ];
    let content = concat_list_content(&list, &files);
    assert_eq!(content, "file 'track-01.mp3'\nfile 'track 02.mp3'");
    // Entries outside the list dir stay relative to CWD (strip_prefix no-op)
    let content2 = concat_list_content(&list, &[PathBuf::from("other/track.mp3")]);
    assert_eq!(content2, "file 'other/track.mp3'");
}

#[test]
fn default_name_depends_on_total_length() {
    assert_eq!(default_output_name(180), "song.mp3");
    assert_eq!(default_output_name(3599), "song.mp3");
    assert_eq!(default_output_name(3600), "mix-60m.mp3");
    assert_eq!(default_output_name(7200), "mix-60m.mp3");
}

#[tokio::test]
async fn assemble_copies_a_single_track() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("track-01.mp3");
    let out = dir.path().join("song.mp3");
    std::fs::write(&src, b"fake-audio-bytes").unwrap();

    assemble(std::slice::from_ref(&src), &out).await.unwrap();

    assert_eq!(std::fs::read(&out).unwrap(), b"fake-audio-bytes");
}
