//! Request validation coverage for the standalone Codex image gateway.

use llm_access_codex_image::request::{
    normalize_image_gateway_path, parse_image_request, CodexImageEndpoint,
};
use serde_json::json;

#[test]
fn codex_image_path_normalization_accepts_public_aliases() {
    assert_eq!(
        normalize_image_gateway_path("/v1/images/generations"),
        Some(CodexImageEndpoint::Generations)
    );
    assert_eq!(
        normalize_image_gateway_path("/api/codex-gateway/images/edits"),
        Some(CodexImageEndpoint::Edits)
    );
    assert_eq!(
        normalize_image_gateway_path("/api/llm-gateway/v1/images/generations"),
        Some(CodexImageEndpoint::Generations)
    );
    assert_eq!(normalize_image_gateway_path("/v1/responses"), None);
}

#[test]
fn codex_image_generation_defaults_model_and_rejects_unknown_fields() {
    let request = parse_image_request(
        CodexImageEndpoint::Generations,
        json!({
            "prompt": "draw a lake",
            "size": "1024x1024",
            "quality": "high",
            "n": 2
        }),
    )
    .expect("valid generation request");

    assert_eq!(request.model, "gpt-image-2");
    assert_eq!(request.n, 2);

    let err = parse_image_request(
        CodexImageEndpoint::Generations,
        json!({
            "prompt": "draw a lake",
            "style": "watercolor"
        }),
    )
    .expect_err("unknown field must be rejected");
    assert_eq!(err.status.as_u16(), 400);
    assert!(err.message.contains("unknown field"));
}

#[test]
fn codex_image_generation_rejects_wrong_model_empty_prompt_and_bad_n() {
    for payload in [
        json!({"model": "gpt-image-1", "prompt": "x"}),
        json!({"model": "gpt-image-2", "prompt": "   "}),
        json!({"model": "gpt-image-2", "prompt": "x", "n": 0}),
        json!({"model": "gpt-image-2", "prompt": "x", "n": 5}),
    ] {
        let err = parse_image_request(CodexImageEndpoint::Generations, payload)
            .expect_err("invalid generation payload must be rejected");
        assert_eq!(err.status.as_u16(), 400);
    }
}

#[test]
fn codex_image_edits_validate_image_sources_and_count() {
    parse_image_request(
        CodexImageEndpoint::Edits,
        json!({
            "prompt": "change the sky",
            "images": [
                "data:image/png;base64,aGVsbG8=",
                "https://example.com/input.webp"
            ]
        }),
    )
    .expect("valid edit request");

    for payload in [
        json!({"prompt": "x", "images": []}),
        json!({"prompt": "x", "images": [
            "https://example.com/1.png",
            "https://example.com/2.png",
            "https://example.com/3.png",
            "https://example.com/4.png",
            "https://example.com/5.png",
            "https://example.com/6.png"
        ]}),
        json!({"prompt": "x", "images": ["file:///tmp/a.png"]}),
        json!({"prompt": "x", "images": ["./a.png"]}),
        json!({"prompt": "x", "images": ["data:text/plain;base64,aGVsbG8="]}),
    ] {
        let err = parse_image_request(CodexImageEndpoint::Edits, payload)
            .expect_err("invalid edit payload must be rejected");
        assert_eq!(err.status.as_u16(), 400);
    }
}
