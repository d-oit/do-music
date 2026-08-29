//! Serialization and defensive-parsing tests for the provider contracts — no
//! network required.
//!
//! See `.agents/skills/gmi-music`.

use do_music::api::{
    ChatMessage, ChatRequest, LlmResponse, PROVIDER_TABLE, ResponseFormat, find_string, h3_request,
    music_request, parse_spec,
};
use serde_json::json;

#[test]
fn music_request_serializes_expected_shape() {
    let req = music_request("minimax-music-3.0", "[Inst]", "warm ambient piano");
    let value = serde_json::to_value(&req).unwrap();
    assert_eq!(
        value,
        json!({
            "model": "minimax-music-3.0",
            "payload": {
                "lyrics": "[Inst]",
                "prompt": "warm ambient piano",
                "sample_rate": 44100,
                "bitrate": 256000,
                "format": "mp3"
            }
        })
    );
}

#[test]
fn music_request_never_carries_a_duration() {
    let value = serde_json::to_value(music_request("m", "lyrics", "prompt")).unwrap();
    assert!(value.get("duration").is_none());
    assert!(value["payload"].get("duration").is_none());
}

#[test]
fn chat_request_uses_json_object_response_format() {
    let value = serde_json::to_value(&ChatRequest {
        model: "m",
        messages: vec![ChatMessage {
            role: "user",
            content: "hi",
        }],
        temperature: 0.2,
        response_format: ResponseFormat::json_object(),
    })
    .unwrap();
    assert_eq!(value["response_format"]["type"], "json_object");
    // temperature is an f32; JSON stores it widened, so compare with tolerance.
    let t = value["temperature"].as_f64().unwrap();
    assert!((t - 0.2).abs() < 1e-6, "temperature was {t}");
    assert_eq!(value["messages"][0]["content"], "hi");
}

#[test]
fn parse_spec_accepts_valid_spec() {
    let content = r#"{"intent":"calm study music","genre":["ambient"],"mood":["calm"],"tempo":"slow","instruments":["piano"],"vocals":"none","energy":"low","arrangement":"minimal","style_prompt":"warm piano","lyrics":"[Inst]","score":8.0}"#;
    let spec = parse_spec(content).unwrap();
    assert_eq!(spec.intent, "calm study music");
    assert_eq!(spec.genre, vec!["ambient"]);
    assert_eq!(spec.score, 8.0);
}

#[test]
fn parse_spec_rejects_invalid_or_incomplete_json() {
    assert!(parse_spec("not json").is_err());
    assert!(parse_spec(r#"{"intent":"only intent"}"#).is_err());
}

#[test]
fn parse_spec_accepts_markdown_fenced_json() {
    let content = "```json\n{\"intent\":\"calm study music\",\"genre\":[\"ambient\"],\"mood\":[\"calm\"],\"tempo\":\"slow\",\"instruments\":[\"piano\"],\"vocals\":\"none\",\"energy\":\"low\",\"arrangement\":\"minimal\",\"style_prompt\":\"warm piano\",\"lyrics\":\"[Inst]\",\"score\":9}\n```";
    let spec = parse_spec(content).unwrap();
    assert_eq!(spec.intent, "calm study music");
    assert_eq!(spec.score, 9.0);
}

#[test]
fn find_string_finds_direct_nested_and_array_keys() {
    let cases = [
        (json!({"audio_url": "a"}), Some("a")),
        (json!({"data": {"audioUrl": "b"}}), Some("b")),
        (json!({"items": [{"url": "c"}]}), Some("c")),
        // Key order wins over insertion order: audio_url outranks download_url.
        (json!({"download_url": "d", "audio_url": "e"}), Some("e")),
    ];
    let keys = ["audio_url", "audioUrl", "url", "download_url"];
    for (value, expected) in cases {
        assert_eq!(find_string(&value, &keys).as_deref(), expected);
    }
}

#[test]
fn find_string_ignores_non_strings_and_missing() {
    assert_eq!(find_string(&json!({"url": 123}), &["url"]), None);
    assert_eq!(find_string(&json!({"data": {}}), &["url"]), None);
    assert_eq!(find_string(&json!(42), &["url"]), None);
}

#[test]
fn llm_response_deserializes_chat_shape() {
    let body = json!({
        "choices": [{"message": {"content": "{\"intent\":\"x\",\"genre\":[],\"mood\":[],\"tempo\":\"\",\"instruments\":[],\"vocals\":\"\",\"energy\":\"\",\"arrangement\":\"\",\"style_prompt\":\"\",\"lyrics\":\"\",\"score\":0}"}}]
    });
    let parsed: LlmResponse = serde_json::from_value(body).unwrap();
    assert!(
        parsed.choices[0]
            .message
            .content
            .contains("\"intent\":\"x\"")
    );
}

#[test]
fn h3_request_matches_documented_quickstart_shape() {
    let req = h3_request("MiniMax-H3", "mist drifts", "https://example.com/frame.png");
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["model"], "MiniMax-H3");
    assert_eq!(json["payload"]["prompt"], "mist drifts");
    assert_eq!(json["payload"]["resolution"], "2K");
    assert_eq!(json["payload"]["duration"], 5);
    assert_eq!(json["payload"]["ratio"], "16:9");
    assert_eq!(
        json["payload"]["first_frame_image"],
        "https://example.com/frame.png"
    );
}

#[test]
fn provider_table_has_the_six_documented_rows() {
    assert_eq!(PROVIDER_TABLE.len(), 6);
    assert!(
        PROVIDER_TABLE
            .iter()
            .any(|(m, k, _, f)| *m == "MiniMaxAI/MiniMax-M2.7" && *k == "llm" && *f)
    );
    assert!(
        PROVIDER_TABLE
            .iter()
            .any(|(m, k, _, f)| *m == "MiniMaxAI/MiniMax-M3" && *k == "llm" && *f)
    );
    assert!(
        PROVIDER_TABLE
            .iter()
            .any(|(m, k, _, f)| *m == "minimax-music-3.0" && *k == "music" && *f)
    );
    assert!(
        PROVIDER_TABLE
            .iter()
            .any(|(m, k, _, f)| *m == "minimax-music-2.5" && *k == "music" && !*f)
    );
    assert!(
        PROVIDER_TABLE
            .iter()
            .any(|(m, k, _, f)| *m == "MiniMax-H3" && *k == "video" && !*f)
    );
    assert!(
        PROVIDER_TABLE
            .iter()
            .any(|(m, k, _, f)| *m == "Pollinations flux" && *k == "image" && *f)
    );
}
