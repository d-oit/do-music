//! Offline tests for the generative visual engine. No live API calls;
//! python-dependent checks skip (pass with a printed note) when
//! python3/numpy is absent.

use do_music::visual;

fn python_numpy_available() -> Option<String> {
    match std::process::Command::new("python3")
        .args(["-c", "import numpy; print(numpy.__version__)"])
        .output()
    {
        Ok(out) if out.status.success() => {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        _ => None,
    }
}

/// Run a snippet with `visualizer/` importable, return trimmed stdout.
fn run_python(snippet: &str) -> Result<String, String> {
    let package_parent = visual::ensure_package().map_err(|e| e.to_string())?;
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(snippet)
        .env("PYTHONPATH", &package_parent)
        .output()
        .map_err(|e| format!("spawn python3: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[test]
fn python_args_contains_every_flag_and_omits_mirror_when_false() {
    let args = visual::python_args(
        std::path::Path::new("/tmp/a.wav"),
        "flow",
        "zen",
        false,
        42,
        do_music::visual::EncodeOptions {
            codec: "h264",
            preset: "medium",
            sensitivity: 1.0,
            gain: 1.0,
        },
        std::path::Path::new("/tmp/v.mp4"),
    );
    let text = args.join(" ");
    for flag in [
        "-m visualizer",
        "/tmp/a.wav",
        "--style flow",
        "--palette zen",
        "--seed 42",
        "--preset medium",
        "--codec h264",
        "-o /tmp/v.mp4",
    ] {
        assert!(text.contains(flag), "missing `{flag}` in `{text}`");
    }
    assert!(!text.contains("--mirror"));
    let with_mirror = visual::python_args(
        std::path::Path::new("/tmp/a.wav"),
        "bloom",
        "ink",
        true,
        7,
        do_music::visual::EncodeOptions {
            codec: "hevc",
            preset: "fast",
            sensitivity: 1.5,
            gain: 0.8,
        },
        std::path::Path::new("/tmp/v.mp4"),
    );
    assert!(with_mirror.contains(&"--mirror".to_string()));
    let mirror_text = with_mirror.join(" ");
    assert!(mirror_text.contains("--preset fast"));
    assert!(mirror_text.contains("--codec hevc"));
}

#[test]
fn value_enum_choices_match_python() {
    // Rust side: the CLI's accepted values (kept in `visual.rs`),
    // sorted to match python's `sorted()` output.
    let mut rust_styles = vec![
        "flow", "bloom", "plasma", "waves", "rings", "spectrum", "kaleido",
    ];
    rust_styles.sort_unstable();
    let mut rust_palettes = vec!["zen", "ink", "abyss", "ember", "aurora", "neon"];
    rust_palettes.sort_unstable();
    // Python side: read from the embedded package sources.
    let py = run_python(
        "from visualizer.styles import STYLES; from visualizer.palette import PALETTES; \
         print(','.join(sorted(STYLES))); print(','.join(sorted(PALETTES)))",
    );
    match py {
        Ok(out) => {
            let mut lines = out.lines();
            let styles: Vec<&str> = lines.next().unwrap().split(',').collect();
            let palettes: Vec<&str> = lines.next().unwrap().split(',').collect();
            assert_eq!(styles, rust_styles);
            assert_eq!(palettes, rust_palettes);
        }
        Err(e) => {
            if python_numpy_available().is_none() {
                println!("SKIP: python3/numpy unavailable ({e})");
            } else {
                panic!("python ran but snippet failed: {e}");
            }
        }
    }
}

#[test]
fn palette_anchors_land_on_expected_stops() {
    match run_python(
        "import numpy as np
from visualizer.palette import palette_lut
for name, anchors in {
    'zen': [(8,16,24),(32,72,60),(110,150,120),(238,226,180)],
    'ink': [(12,12,14),(56,58,62),(140,140,138),(244,242,236)],
}.items():
    lut = palette_lut(name)
    for stop, want in zip((0.0, 0.35, 0.7, 1.0), anchors):
        got = lut[min(255, int(round(stop*255)))]
        assert max(abs(int(g)-int(w)) for g, w in zip(got, want)) <= 1, (name, stop, got, want)
print('ok')",
    ) {
        Ok(s) => assert_eq!(s, "ok"),
        Err(e) => {
            if python_numpy_available().is_none() {
                println!("SKIP: python3/numpy unavailable");
            } else {
                panic!("palette anchor check failed: {e}");
            }
        }
    }
}

#[test]
fn band_dominance_on_synthetic_tones() {
    if python_numpy_available().is_none() {
        println!("SKIP: python3/numpy unavailable");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let low = dir.path().join("low.wav");
    let high = dir.path().join("high.wav");
    for (path, freq) in [(&low, 55.0), (&high, 5000.0)] {
        let snippet = format!(
            "import numpy as np, wave\n\
             rate=22050; n=rate*2\n\
             sig=(0.8*np.sin(2*np.pi*{freq}*np.arange(n)/rate)*32767).astype('<i2')\n\
             w=wave.open(r'''{}''','wb'); w.setnchannels(1); w.setsampwidth(2); w.setframerate(rate)\n\
             w.writeframes(sig.tobytes()); w.close()",
            path.display()
        );
        run_python(&snippet).expect("tone generation");
    }
    let out = run_python(&format!(
        "import json\nfrom visualizer.validate import check_band_separation\n\
         low = check_band_separation(r'''{}''')\n\
         high = check_band_separation(r'''{}''')\n\
         assert low['bass_mean'] > low['treble_mean'] * 5, low\n\
         assert high['treble_mean'] > high['bass_mean'] * 5, high\n\
         print(json.dumps({{'low': low, 'high': high}}))",
        low.display(),
        high.display()
    ))
    .expect("band separation");
    assert!(out.contains("\"bass_mean\""));
}

#[test]
fn main_end_to_end_frame_count_matches_fps() {
    // Runs the real visualizer main() with a fake ffmpeg: asserts the
    // stream written is exactly (audio duration at fps) frames of
    // sim-sized RGB bytes matching the declared rawvideo size.
    if python_numpy_available().is_none() {
        println!("SKIP: python3/numpy unavailable");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let wav = dir.path().join("tone.wav");
    let out = dir.path().join("tiny.mp4");
    let snippet = format!(
        "import types, wave\n\
         import numpy as np\n\
         import visualizer.cli as cli\n\
         from visualizer import analysis\n\
         rate=22050; n=rate*2\n\
         sig=(0.8*np.sin(2*np.pi*440*np.arange(n)/rate)*32767).astype('<i2')\n\
         w=wave.open(r'''{wav}''','wb'); w.setnchannels(1); w.setsampwidth(2); w.setframerate(rate)\n\
         w.writeframes(sig.tobytes()); w.close()\n\
         written=[]\n\
         stdin=types.SimpleNamespace(write=lambda b: written.append(len(b)), close=lambda: None)\n\
         proc=types.SimpleNamespace(stdin=stdin, wait=lambda: 0)\n\
         cmds=[]\n\
         cli.subprocess.Popen=lambda cmd, **kw: (cmds.append(cmd), proc)[1]\n\
         rc=cli.main([r'''{wav}''','--style','flow','--palette','zen','--sim-width','64','--sim-height','36','-o',r'''{out}'''])\n\
         assert rc==0, rc\n\
         assert '-preset' in cmds[0] and 'medium' in cmds[0], cmds[0]\n\
         assert cmds[0][cmds[0].index('-s')+1]=='64x36', cmds[0]\n\
         n_feat=1+(n-analysis._WINDOW)//analysis._HOP\n\
         duration=((n_feat-1)*analysis._HOP+analysis._WINDOW)/22050.0\n\
         n_frames=int(round(duration*30))\n\
         total=sum(written)\n\
         assert total==n_frames*36*64*3,(total,n_frames)\n\
         assert n_frames==60, n_frames\n\
         print('ok',n_frames)",
        wav = wav.display(),
        out = out.display()
    );
    match run_python(&snippet) {
        Ok(s) => assert!(s.starts_with("ok"), "unexpected output: {s}"),
        Err(e) => panic!("visualizer main smoke failed: {e}"),
    }
}

#[test]
fn plasma_deterministic_and_well_shaped() {
    match run_python(
        "import numpy as np
from visualizer.styles import Plasma
s1 = Plasma(64, 36, 7); f1 = s1.step(np.zeros(3), 0.0)
s2 = Plasma(64, 36, 7); f2 = s2.step(np.zeros(3), 0.0)
assert f1.shape == (36, 64, 3) and f1.dtype == np.uint8, (f1.shape, f1.dtype)
assert np.array_equal(f1, f2), 'same seed diverged'
print('ok')",
    ) {
        Ok(s) => assert_eq!(s, "ok"),
        Err(e) => {
            if python_numpy_available().is_none() {
                println!("SKIP: python3/numpy unavailable");
            } else {
                panic!("plasma determinism failed: {e}");
            }
        }
    }
}

#[test]
fn waves_and_rings_deterministic_and_band_reactive() {
    match run_python(
        "import numpy as np
from visualizer.styles import Waves, Rings
for cls in (Waves, Rings):
    a = cls(64, 36, 7); fa = a.step(np.zeros(3), 1.0)
    b = cls(64, 36, 7); fb = b.step(np.zeros(3), 1.0)
    assert fa.shape == (36, 64, 3) and fa.dtype == np.uint8, (cls.__name__, fa.shape)
    assert np.array_equal(fa, fb), f'{cls.__name__} same seed diverged'
    bass = cls(64, 36, 7).step(np.array([1.0, 0.0, 0.0]), 1.0)
    treble = cls(64, 36, 7).step(np.array([0.0, 0.0, 1.0]), 1.0)
    assert not np.array_equal(bass, treble), f'{cls.__name__} not band-reactive'
print('ok')",
    ) {
        Ok(s) => assert_eq!(s, "ok"),
        Err(e) => {
            if python_numpy_available().is_none() {
                println!("SKIP: python3/numpy unavailable");
            } else {
                panic!("waves/rings determinism failed: {e}");
            }
        }
    }
}

#[test]
fn embedded_package_files_are_complete() {
    // The in-tree package exists in this repo, so ensure_package reuses
    // it; the materialization fallback is exercised by run_python's
    // ensure_package call in every python test. Verify the embedded
    // manifest itself is complete.
    let files = visual::package_files();
    assert!(files.len() >= 5);
    for (name, body) in files {
        assert!(!body.trim().is_empty(), "{name} empty");
    }
}

/// Synthesize a 60 s ambient WAV with known events at 20 s and 40 s, run the
/// highlight detector on it, and return the detected mark times.
#[test]
fn highlight_detector_finds_known_musical_events() {
    if python_numpy_available().is_none() {
        println!("skipping: python3/numpy not available");
        return;
    }
    let snippet = r#"
import json, tempfile, os, wave
import numpy as np
from visualizer import highlights

rate = 22050
t = np.arange(60 * rate) / rate
sig = 0.15*np.sin(2*np.pi*110*t) + 0.1*np.sin(2*np.pi*220*t)
sig[int(20*rate):] += 0.35*np.sin(2*np.pi*165*t[int(20*rate):])
sig[int(40*rate):] += 0.25*np.sin(2*np.pi*3000*t[int(40*rate):])
sig = np.clip(sig, -1, 1)
path = os.path.join(tempfile.mkdtemp(), "t.wav")
w = wave.open(path, "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(rate)
w.writeframes((sig*32767).astype("<i2").tobytes()); w.close()

doc = highlights.analyze(path)
print(json.dumps([m["time"] for m in doc["marks"]]))
"#;
    let out = match run_python(snippet) {
        Ok(o) => o,
        Err(e) => panic!("highlight detector failed: {e}"),
    };
    let times: Vec<f64> = serde_json::from_str(&out).expect("mark times are JSON");
    // A swell at 20 s and a bright entry at 40 s must both be found, each
    // within a second of the truth (the detector compensates group delay).
    for expected in [20.0_f64, 40.0] {
        assert!(
            times.iter().any(|t| (t - expected).abs() < 1.0),
            "no mark near {expected}s in {times:?}"
        );
    }
}

#[test]
fn highlight_detector_is_deterministic_and_silence_yields_no_marks() {
    if python_numpy_available().is_none() {
        println!("skipping: python3/numpy not available");
        return;
    }
    let snippet = r#"
import json, tempfile, os, wave
import numpy as np
from visualizer import highlights

def render(sig, rate=22050):
    path = os.path.join(tempfile.mkdtemp(), "t.wav")
    w = wave.open(path, "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(rate)
    w.writeframes((np.clip(sig, -1, 1)*32767).astype("<i2").tobytes()); w.close()
    return path

rate = 22050
t = np.arange(30 * rate) / rate
tone = 0.2*np.sin(2*np.pi*220*t)
a = highlights.analyze(render(tone))
b = highlights.analyze(render(tone))
silence = highlights.analyze(render(np.zeros(30*rate)))
print(json.dumps({
    "deterministic": a == b,
    "steady_marks": len(a["marks"]),
    "silent_marks": len(silence["marks"]),
}))
"#;
    let out = match run_python(snippet) {
        Ok(o) => o,
        Err(e) => panic!("highlight detector failed: {e}"),
    };
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    assert_eq!(v["deterministic"], serde_json::json!(true));
    // A steady tone and pure silence have no musical events to cut on:
    // scores are absolute, so featureless audio must not be stretched into
    // spurious highlights.
    assert_eq!(
        v["steady_marks"],
        serde_json::json!(0),
        "steady tone: {out}"
    );
    assert_eq!(v["silent_marks"], serde_json::json!(0), "silence: {out}");
}
