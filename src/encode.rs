//! Final-chain video codec selection: pure arg builders for the YouTube-safe
//! H.264 default plus HEVC and AV1 alternatives, shared by the xfade chain,
//! single-segment path, and transcode path. No I/O; fully offline-testable.
//!
//! `h264` follows YouTube's upload guidance (High profile, two B frames,
//! closed GOP, BT.709, MP4 fast-start). `hevc` and `av1` trade some
//! compatibility for markedly smaller files at comparable quality; callers
//! targeting YouTube should stick to the default.

use crate::quality::Quality;

/// Supported final-chain codecs; H.264 is the YouTube-compatible default.
pub const FINAL_CODEC_CHOICES: &[&str] = &["h264", "hevc", "av1"];

/// Encoding flags shared by the final video chain and single-segment path.
pub fn final_encode_args(codec: &str, quality: Quality) -> anyhow::Result<Vec<String>> {
    let preset = quality.final_preset();
    let mut args: Vec<String> = ["-pix_fmt", "yuv420p"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    match codec {
        "h264" => args.extend([
            "-c:v".into(),
            "libx264".into(),
            "-preset".into(),
            preset.into(),
            "-crf".into(),
            "18".into(),
            "-profile:v".into(),
            "high".into(),
            "-level:v".into(),
            "4.2".into(),
            "-bf".into(),
            "2".into(),
            "-g".into(),
            "60".into(),
            "-keyint_min".into(),
            "60".into(),
            "-flags".into(),
            "+cgop".into(),
        ]),
        "hevc" => args.extend([
            "-c:v".into(),
            "libx265".into(),
            "-preset".into(),
            preset.into(),
            "-crf".into(),
            "20".into(),
            "-tag:v".into(),
            "hvc1".into(),
            "-bf".into(),
            "2".into(),
            "-g".into(),
            "60".into(),
            "-keyint_min".into(),
            "60".into(),
            "-flags".into(),
            "+cgop".into(),
        ]),
        "av1" => args.extend([
            "-c:v".into(),
            "libsvtav1".into(),
            "-preset".into(),
            match quality {
                Quality::Fast => "10".into(),
                Quality::Balanced => "8".into(),
                Quality::High => "6".into(),
            },
            "-crf".into(),
            "30".into(),
            // tune=0 (VBR-friendly) without synthetic grain; grain estimation
            // costs encoder time and meditation visuals carry no real grain.
            "-svtav1-params".into(),
            "tune=0:film-grain=0".into(),
            "-g".into(),
            "60".into(),
            "-bf".into(),
            "2".into(),
        ]),
        other => {
            anyhow::bail!(
                "unsupported video codec {other:?} (choose from {FINAL_CODEC_CHOICES:?})"
            );
        }
    }
    args.extend([
        "-colorspace".into(),
        "bt709".into(),
        "-color_primaries".into(),
        "bt709".into(),
        "-color_trc".into(),
        "bt709".into(),
        "-movflags".into(),
        "+faststart".into(),
    ]);
    Ok(args)
}

/// H.264 variant used by the single-segment and transcode paths.
pub fn youtube_video_encode_args(quality: Quality) -> Vec<String> {
    final_encode_args("h264", quality).expect("h264 args are always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_encode_args_h264_follows_youtube_guidance() {
        let args = final_encode_args("h264", Quality::default()).unwrap();
        assert!(args.contains(&"libx264".to_string()));
        let crf = args.iter().position(|a| a == "-crf").unwrap();
        assert_eq!(args[crf + 1], "18");
        assert!(args.contains(&"high".to_string()));
        assert!(args.contains(&"+cgop".to_string()));
        assert!(args.contains(&"bt709".to_string()));
        assert!(args.contains(&"+faststart".to_string()));
    }

    #[test]
    fn final_encode_args_hevc_tags_hvc1() {
        let args = final_encode_args("hevc", Quality::default()).unwrap();
        assert!(args.contains(&"libx265".to_string()));
        assert!(args.contains(&"hvc1".to_string()));
        let crf = args.iter().position(|a| a == "-crf").unwrap();
        assert_eq!(args[crf + 1], "20");
        assert!(args.contains(&"+faststart".to_string()));
    }

    #[test]
    fn final_encode_args_av1_uses_svtav1_tuned_without_grain() {
        let args = final_encode_args("av1", Quality::default()).unwrap();
        assert!(args.contains(&"libsvtav1".to_string()));
        let preset = args.iter().position(|a| a == "-preset").unwrap();
        assert_eq!(args[preset + 1], "8");
        let crf = args.iter().position(|a| a == "-crf").unwrap();
        assert_eq!(args[crf + 1], "30");
        let params = args.iter().position(|a| a == "-svtav1-params").unwrap();
        assert_eq!(args[params + 1], "tune=0:film-grain=0");
    }

    #[test]
    fn quality_tier_drives_the_final_preset() {
        let fast = final_encode_args("h264", Quality::Fast).unwrap();
        let i = fast.iter().position(|a| a == "-preset").unwrap();
        assert_eq!(fast[i + 1], "veryfast");
        let high = final_encode_args("h264", Quality::High).unwrap();
        let i = high.iter().position(|a| a == "-preset").unwrap();
        assert_eq!(high[i + 1], "slow");
        // AV1 speeds up via its numeric preset ladder instead.
        let av1 = final_encode_args("av1", Quality::Fast).unwrap();
        let i = av1.iter().position(|a| a == "-preset").unwrap();
        assert_eq!(av1[i + 1], "10");
    }

    #[test]
    fn final_encode_args_reject_unknown_codecs() {
        let err = final_encode_args("mpeg2", Quality::default()).unwrap_err();
        assert!(err.to_string().contains("mpeg2"));
    }

    #[test]
    fn final_codec_choices_match_implemented_branches() {
        for codec in FINAL_CODEC_CHOICES {
            assert!(
                final_encode_args(codec, Quality::default()).is_ok(),
                "{codec} not implemented"
            );
        }
    }
}
