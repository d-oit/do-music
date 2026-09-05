//! Cinematic motion recipes for scene stills.
//!
//! The fallback animation used to be one breathing `zoompan` for every
//! scene of every video, so a ten-minute render was ten near-identical
//! slow zooms. This module builds a *vocabulary* of camera moves and
//! atmospheric layers and assigns them per scene, driven by the music.
//!
//! Everything here is a pure filter-graph builder: no I/O, no spawning, so
//! the whole vocabulary is offline-testable. `render.rs` executes it.
//!
//! Design constraints that shaped these recipes:
//! * meditation footage must stay *calm* — moves are slow and eased, never
//!   `Ken Burns`;
//! * every move reads from a supersampled source and must never sample
//!   outside the frame (see `MAX_PAN`);
//! * filters are limited to the widely-available set (scale, zoompan, crop,
//!   rotate, vignette, eq, noise, gblur, colorbalance) so no ffmpeg build
//!   flags are required.

use crate::quality::Quality;
use crate::video::{OUTPUT_HEIGHT, OUTPUT_WIDTH, VIDEO_FPS};

/// Pan amplitude as a fraction of the available crop margin.
///
/// The pan is centered at `margin/2` and valid over `[0, margin]`, so any
/// amplitude above 0.5 is silently clamped by zoompan and the move visibly
/// stalls at the extremes of every cycle.
pub const MAX_PAN: f64 = 0.45;

/// One camera move in the vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Move {
    /// Slow breathing zoom, eased with a cosine. The calm default.
    Breathe,
    /// Continuous slow push toward the subject (dolly in).
    PushIn,
    /// Continuous slow pull away (dolly out), revealing context.
    PullOut,
    /// Lateral drift across the frame with almost no zoom change.
    Drift,
    /// Very slow rotation plus a light zoom; dreamlike, used sparingly.
    Orbit,
    /// Static frame; atmosphere layers supply all the motion.
    Hold,
}

impl Move {
    /// Human-readable name (also used in logs and the autotune record).
    pub fn name(self) -> &'static str {
        match self {
            Self::Breathe => "breathe",
            Self::PushIn => "push-in",
            Self::PullOut => "pull-out",
            Self::Drift => "drift",
            Self::Orbit => "orbit",
            Self::Hold => "hold",
        }
    }

    /// All moves, in vocabulary order.
    pub fn all() -> [Move; 6] {
        [
            Self::Breathe,
            Self::PushIn,
            Self::PullOut,
            Self::Drift,
            Self::Orbit,
            Self::Hold,
        ]
    }
}

/// How lively a scene should be, derived from the music under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Energy(pub f64);

impl Energy {
    /// Clamp to 0..1; anything outside is a bug upstream but must not
    /// produce an invalid filter graph.
    pub fn new(value: f64) -> Self {
        Self(if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.35
        })
    }

    /// Calm scenes get slower, smaller moves; energetic scenes get more.
    pub fn scale(self, calm: f64, lively: f64) -> f64 {
        calm + (lively - calm) * self.0
    }
}

/// A fully-specified shot: camera move plus atmosphere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub camera: Move,
    /// Cycle length of the move, in seconds.
    pub period: f64,
    /// Zoom travel over the shot (0.03 = 3%).
    pub amplitude: f64,
    /// Pan amplitude as a fraction of the crop margin (<= MAX_PAN).
    pub pan: f64,
    /// Drift direction in degrees.
    pub angle: f64,
    /// Musical energy under this scene.
    pub energy: Energy,
}

/// Choose a shot for `scene_index`, varying the vocabulary deterministically.
///
/// Determinism matters: the same track and scene list must always produce
/// the same video, so this is a fixed rotation seeded by the scene index
/// rather than a random draw. The rotation is offset by musical energy, so
/// a livelier passage biases toward moves with more travel.
pub fn plan_shot(scene_index: usize, energy: Energy, total_scenes: usize) -> Shot {
    // A fixed, non-repeating rotation reads as intentional variety; a
    // random pick tends to clump the same move two or three times running.
    const ROTATION: [Move; 6] = [
        Move::Breathe,
        Move::PushIn,
        Move::Drift,
        Move::PullOut,
        Move::Orbit,
        Move::Breathe,
    ];
    // Open and close on the calmest move so the video settles at both ends.
    let camera = if scene_index == 0 || (total_scenes > 1 && scene_index + 1 == total_scenes) {
        Move::Breathe
    } else {
        ROTATION[scene_index % ROTATION.len()]
    };
    // Energetic passages move a little faster and travel a little further.
    let period = energy.scale(60.0, 34.0);
    let amplitude = match camera {
        Move::Breathe => energy.scale(0.028, 0.045),
        Move::PushIn | Move::PullOut => energy.scale(0.05, 0.09),
        Move::Drift => energy.scale(0.02, 0.03),
        Move::Orbit => energy.scale(0.04, 0.06),
        Move::Hold => 0.0,
    };
    let pan = match camera {
        Move::Drift => MAX_PAN,
        Move::Hold => 0.0,
        _ => MAX_PAN * 0.6,
    };
    Shot {
        camera,
        period,
        amplitude,
        pan,
        // 47 degrees per scene walks the circle without repeating quickly.
        angle: (scene_index as f64 * 47.0) % 360.0,
        energy,
    }
}

/// The zoom expression for a shot, in zoompan's `on` (output frame) domain.
fn zoom_expr(shot: &Shot, frames_per_cycle: f64) -> String {
    let a = shot.amplitude;
    match shot.camera {
        // Cosine easing: velocity is zero at both ends, so the move never
        // visibly starts or stops — essential for the meditative feel.
        Move::Breathe => format!("1.0+{a:.4}*(1.0-cos(2*PI*on/{frames_per_cycle:.0}))/2"),
        // Smoothstep-ish ease over the whole shot rather than a cycle.
        Move::PushIn => format!("1.0+{a:.4}*(1.0-cos(PI*min(on/{frames_per_cycle:.0},1)))/2"),
        Move::PullOut => {
            format!(
                "{:.4}-{a:.4}*(1.0-cos(PI*min(on/{frames_per_cycle:.0},1)))/2",
                1.0 + a
            )
        }
        Move::Drift => format!("1.0+{a:.4}"),
        Move::Orbit => format!("1.0+{a:.4}*(1.0-cos(2*PI*on/{frames_per_cycle:.0}))/2"),
        Move::Hold => "1.0".to_string(),
    }
}

/// Pan expressions `(x, y)` for a shot.
fn pan_exprs(shot: &Shot, frames_per_cycle: f64) -> (String, String) {
    if shot.pan <= 0.0 {
        return ("(iw-iw/zoom)/2".to_string(), "(ih-ih/zoom)/2".to_string());
    }
    let rad = shot.angle.to_radians();
    let (mx, my) = (rad.cos() * shot.pan, rad.sin() * shot.pan);
    // A full pan cycle runs at half the zoom rate so the two never beat
    // against each other in a way the eye can lock onto.
    let p = frames_per_cycle * 2.0;
    (
        format!("(iw-iw/zoom)/2+{mx:.4}*(iw-iw/zoom)*sin(2*PI*on/{p:.0})"),
        format!("(ih-ih/zoom)/2+{my:.4}*(ih-ih/zoom)*cos(2*PI*on/{p:.0})"),
    )
}

/// Atmosphere layers appended after the camera move.
///
/// These are what actually make a still photograph read as a *shot*:
/// a graded, vignetted frame with a hint of bloom and grain. Kept cheap —
/// all are per-pixel or small-kernel filters.
fn atmosphere(shot: &Shot, quality: Quality) -> Vec<String> {
    let mut layers = Vec::new();
    // Slight rotation for orbit shots, applied after zoom so the crop hides
    // the corners the rotation would otherwise expose.
    if shot.camera == Move::Orbit {
        let deg = shot.energy.scale(0.4, 0.9);
        let rad = deg.to_radians();
        layers.push(format!(
            "rotate={rad:.5}*sin(2*PI*t/{:.0}):ow=iw:oh=ih:c=none",
            shot.period * 2.0
        ));
    }
    layers.push("vignette=PI/6".to_string());
    // Film grade: gentle S-curve contrast, gently lifted saturation, and a
    // gentle cool/warm balance keyed to energy (calm scenes cooler).
    let contrast = 1.02 + shot.energy.0 * 0.04;
    let saturation = 1.03 + shot.energy.0 * 0.07;
    layers.push(format!(
        "eq=contrast={contrast:.3}:saturation={saturation:.3}:gamma=1.02"
    ));
    let warmth = shot.energy.scale(-0.03, 0.04);
    layers.push(format!("colorbalance=rs={warmth:.3}:bs={:.3}", -warmth));
    if quality.grain() > 0 {
        layers.push(format!("noise=alls={}:allf=t+u", quality.grain()));
    }
    layers
}

/// Build the complete `-vf` chain for one scene.
///
/// `supersample` comes from the quality tier; the graph scales up, performs
/// the camera move at that resolution (which is what removes zoompan's
/// pixel-snap jitter), then renders down to the output canvas.
pub fn filter_chain(shot: &Shot, quality: Quality, duration_secs: f64) -> String {
    let ss = quality.supersample();
    let (sw, sh) = (OUTPUT_WIDTH * ss, OUTPUT_HEIGHT * ss);
    let flags = quality.scale_flags();
    let fps = VIDEO_FPS as f64;

    // A push/pull eases over the whole shot; cyclic moves use their period.
    let frames = match shot.camera {
        Move::PushIn | Move::PullOut => (duration_secs * fps).max(1.0),
        _ => (shot.period * fps).max(1.0),
    };
    let z = zoom_expr(shot, frames);
    let (x, y) = pan_exprs(shot, frames);

    let mut parts = vec![format!("scale={sw}:{sh}:flags={flags}")];
    if shot.camera == Move::Hold {
        parts.push(format!(
            "scale={OUTPUT_WIDTH}:{OUTPUT_HEIGHT}:flags={flags},fps={VIDEO_FPS}"
        ));
    } else {
        parts.push(format!(
            "zoompan=z='{z}':x='{x}':y='{y}':d=1:s={OUTPUT_WIDTH}x{OUTPUT_HEIGHT}:fps={VIDEO_FPS}"
        ));
    }
    parts.extend(atmosphere(shot, quality));
    parts.push("format=yuv420p".to_string());
    parts.join(",")
}

/// Transition vocabulary: pick a crossfade that suits the cut.
///
/// A hard musical accent wants a brief light flash (`fadewhite`); a gentle
/// section change wants a plain dissolve. `strength` is the highlight
/// strength of the cut in 0..1.
pub fn transition_for(strength: f64, index: usize) -> &'static str {
    if !strength.is_finite() {
        return "fade";
    }
    if strength >= 0.85 {
        "fadewhite"
    } else if strength >= 0.6 {
        // Alternate two soft geometric wipes so strong-but-not-peak cuts
        // don't all look identical.
        if index.is_multiple_of(2) {
            "circleopen"
        } else {
            "smoothleft"
        }
    } else {
        "fade"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(camera: Move) -> Shot {
        Shot {
            camera,
            period: 40.0,
            amplitude: 0.05,
            pan: MAX_PAN,
            angle: 30.0,
            energy: Energy::new(0.5),
        }
    }

    #[test]
    fn energy_clamps_and_interpolates() {
        assert_eq!(Energy::new(-1.0).0, 0.0);
        assert_eq!(Energy::new(4.0).0, 1.0);
        assert_eq!(Energy::new(f64::NAN).0, 0.35);
        assert!((Energy::new(0.5).scale(10.0, 20.0) - 15.0).abs() < 1e-9);
    }

    #[test]
    fn shots_vary_across_scenes_but_settle_at_both_ends() {
        let e = Energy::new(0.4);
        let shots: Vec<Move> = (0..8).map(|i| plan_shot(i, e, 8).camera).collect();
        assert_eq!(shots[0], Move::Breathe, "opening shot must be calm");
        assert_eq!(shots[7], Move::Breathe, "closing shot must be calm");
        let distinct: std::collections::HashSet<_> = shots.iter().collect();
        assert!(distinct.len() >= 4, "not enough variety: {shots:?}");
    }

    #[test]
    fn planning_is_deterministic() {
        let e = Energy::new(0.6);
        for i in 0..20 {
            assert_eq!(plan_shot(i, e, 20), plan_shot(i, e, 20));
        }
    }

    #[test]
    fn pan_amplitude_never_exceeds_the_crop_margin() {
        for i in 0..64 {
            for level in [0.0, 0.5, 1.0] {
                let s = plan_shot(i, Energy::new(level), 64);
                assert!(s.pan <= MAX_PAN + 1e-9, "scene {i} pan {}", s.pan);
                let rad = s.angle.to_radians();
                assert!(rad.cos().abs() * s.pan <= 0.5 + 1e-9);
                assert!(rad.sin().abs() * s.pan <= 0.5 + 1e-9);
            }
        }
    }

    #[test]
    fn every_move_builds_a_valid_looking_chain() {
        for m in Move::all() {
            let chain = filter_chain(&shot(m), Quality::Balanced, 60.0);
            assert!(
                chain.starts_with("scale=3840:2160:flags=lanczos"),
                "{chain}"
            );
            assert!(chain.ends_with("format=yuv420p"), "{chain}");
            assert!(chain.contains("vignette=PI/6"));
            assert!(
                chain.contains(&format!("s={OUTPUT_WIDTH}x{OUTPUT_HEIGHT}")) || m == Move::Hold
            );
            // No empty filter slots, which ffmpeg rejects outright.
            assert!(!chain.contains(",,"), "empty filter in {chain}");
            assert!(!chain.contains("NaN"), "NaN leaked into {chain}");
        }
    }

    #[test]
    fn hold_shots_skip_zoompan_entirely() {
        let chain = filter_chain(&shot(Move::Hold), Quality::Balanced, 60.0);
        assert!(!chain.contains("zoompan"), "{chain}");
        assert!(chain.contains("fps=30"));
    }

    #[test]
    fn push_and_pull_travel_in_opposite_directions() {
        let push = filter_chain(&shot(Move::PushIn), Quality::Balanced, 60.0);
        let pull = filter_chain(&shot(Move::PullOut), Quality::Balanced, 60.0);
        // Push starts at 1.0 and grows; pull starts above 1.0 and shrinks.
        assert!(push.contains("z='1.0+"), "{push}");
        assert!(pull.contains("z='1.0500-"), "{pull}");
    }

    #[test]
    fn orbit_rotates_but_other_moves_do_not() {
        assert!(filter_chain(&shot(Move::Orbit), Quality::Balanced, 60.0).contains("rotate="));
        assert!(!filter_chain(&shot(Move::Breathe), Quality::Balanced, 60.0).contains("rotate="));
    }

    #[test]
    fn fast_tier_drops_grain_from_the_chain() {
        let chain = filter_chain(&shot(Move::Breathe), Quality::Fast, 60.0);
        assert!(!chain.contains("noise="), "{chain}");
        let high = filter_chain(&shot(Move::Breathe), Quality::High, 60.0);
        assert!(high.contains("noise=alls=6"), "{high}");
    }

    #[test]
    fn energy_drives_pace_and_grade() {
        let calm = plan_shot(1, Energy::new(0.0), 10);
        let lively = plan_shot(1, Energy::new(1.0), 10);
        assert!(lively.period < calm.period, "livelier scenes move faster");
        assert!(
            lively.amplitude > calm.amplitude,
            "livelier scenes travel more"
        );
    }

    #[test]
    fn transitions_escalate_with_cut_strength() {
        assert_eq!(transition_for(0.1, 0), "fade");
        assert_eq!(transition_for(0.7, 0), "circleopen");
        assert_eq!(transition_for(0.7, 1), "smoothleft");
        assert_eq!(transition_for(0.95, 0), "fadewhite");
        assert_eq!(transition_for(f64::NAN, 0), "fade");
    }
}
