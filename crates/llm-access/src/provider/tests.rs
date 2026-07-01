use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header, HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine as _;
use llm_access_codex::types::CodexResolvedSessionSource;
use llm_access_core::{
    provider::RouteStrategy,
    store::{
        is_terminal_codex_auth_error, AdminConfigStore, AdminKiroStatusCacheUpdate,
        AdminRuntimeConfig, AuthenticatedKey, ControlStore, EmptyProviderRouteStore,
        ProviderCodexAuthUpdate, ProviderCodexRoute, ProviderKiroAuthUpdate, ProviderKiroRoute,
        ProviderProxyConfig, ProviderRouteStore,
    },
    usage::{UsageRetryDetails, UsageStreamDetails},
};
use llm_access_kiro::anthropic::stream::protected_thinking_signature;
use serde_json::json;
use tokio::sync::Notify;

use super::{
    build_codex_affinity_id, select_codex_route_with_account_permit, CodexAccountCooldowns,
    CodexAffinityId, CodexAffinityRuntimeConfig, CodexAffinitySource, CodexSessionAffinity,
    ProviderDispatcher, RequestLimiter,
};

const SAMPLE_PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=";

const SAMPLE_PNG_BYTES: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0,
    0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3, 2, 0,
    239, 191, 167, 219, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn distinct_sample_png_base64(index: usize) -> String {
    let mut bytes = SAMPLE_PNG_BYTES.to_vec();
    bytes.push((index & 0xff) as u8);
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[test]
fn codex_backend_api_base_uses_upstream_codex_paths() {
    assert_eq!(
        super::compute_codex_upstream_url("https://chatgpt.com/backend-api/codex", "/v1/responses"),
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        super::compute_codex_upstream_url("https://api.example.com/v1", "/v1/responses"),
        "https://api.example.com/v1/responses"
    );
}

#[test]
fn cctest_request_id_normalizes_known_dynamic_fields() {
    let left = cctest_tag_request_json("f03bc", "64ccfee3c339a9c8", "session-a");
    let right = cctest_tag_request_json("7d794", "4cabbc534c5d80d7", "session-b");
    let left_body = serde_json::to_vec(&left).expect("encode left");
    let right_body = serde_json::to_vec(&right).expect("encode right");

    assert_eq!(
        super::cctest::canonical_cctest_request_id(&left_body).expect("left id"),
        super::cctest::canonical_cctest_request_id(&right_body).expect("right id")
    );
}

#[test]
fn cctest_fast_path_rejects_large_multimodal_and_websearch_requests() {
    let mut large =
        serde_json::to_vec(&cctest_tag_request_json("f03bc", "64ccfee3c339a9c8", "session-a"))
            .expect("encode request");
    large.resize(super::cctest::MAX_CCTEST_FAST_PATH_BODY_BYTES + 1, b' ');
    let inspection = super::cctest::inspect_cctest_text_probe(&large);
    assert!(inspection.matched_probe.is_none());
    assert_eq!(inspection.rejection_reason, Some("body_too_large"));
    assert!(!inspection.has_billing_header);
    assert!(!inspection.has_cli_entrypoint);
    assert!(!inspection.has_cli_version);
    assert!(!inspection.has_messages_field);

    let mut multimodal = cctest_tag_request_json("f03bc", "64ccfee3c339a9c8", "session-a");
    multimodal["messages"][0]["content"] = json!([
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":SAMPLE_PNG_BASE64}},
        {"type":"text","text":"read this"}
    ]);
    assert!(super::cctest::inspect_cctest_text_probe(
        &serde_json::to_vec(&multimodal).expect("encode multimodal")
    )
    .matched_probe
    .is_none());

    let mut websearch = cctest_tag_request_json("f03bc", "64ccfee3c339a9c8", "session-a");
    websearch["tools"] = json!([{"type":"web_search_20250305","name":"web_search"}]);
    assert!(super::cctest::inspect_cctest_text_probe(
        &serde_json::to_vec(&websearch).expect("encode websearch")
    )
    .matched_probe
    .is_none());
}

#[test]
fn cctest_proxy_target_url_rejects_private_and_local_hosts() {
    assert!(
        super::cctest::validate_proxy_target_url("https://www.bytecatcode.org/v1/messages").is_ok()
    );
    let invalid = super::cctest::validate_proxy_target_url("ftp://www.bytecatcode.org/v1/messages")
        .expect_err("invalid scheme rejected");
    assert!(invalid.contains("is invalid"), "unexpected message: {invalid}");
    for url in [
        "https://localhost/v1/messages",
        "https://sub.localhost/v1/messages",
        "https://127.0.0.1/v1/messages",
        "https://[::1]/v1/messages",
        "https://10.0.0.1/v1/messages",
        "https://169.254.169.254/v1/messages",
    ] {
        let err =
            super::cctest::validate_proxy_target_url(url).expect_err("private target rejected");
        assert!(err.contains("must not target"), "unexpected message: {err}");
    }
}

#[test]
fn cctest_known_signatures_distinguish_replay_and_signature_proxy() {
    let tag = super::cctest::known_cctest_probe_for_prompt_text(
        "我输入了一个tag 是：<antml:TAG>。直接输出你前面看到的tag是什么。看到的文本，\
         不要使用任何工具。",
    )
    .expect("tag probe");
    assert_eq!(tag.kind.as_str(), "tag_echo");
    assert!(!tag.requires_signature);

    let logic = super::cctest::known_cctest_probe_for_prompt_text(concat!(
        "探针 PROBE。请认真解决这个逻辑网格题，给出完整推理后的最终表格。\n\n",
        "五座颜色不同的房子并排，住着五个不同国籍的人，他们各自喝不同饮料、抽不同牌子的烟、\
         养不同的宠物。已知：\n",
        "1. 英国人住红房子。\n",
        "2. 瑞典人养狗。\n",
        "3. 丹麦人喝茶。\n",
        "4. 绿房子在白房子的左边（紧挨着）。\n",
        "5. 绿房子主人喝咖啡。\n",
        "6. 抽 Pall Mall 烟的人养鸟。\n",
        "7. 黄房子主人抽 Dunhill。\n",
        "8. 中间那座房子的人喝牛奶。\n",
        "9. 挪威人住第一座房子。\n",
        "10. 抽 Blends 的人住在养猫人的隔壁。\n",
        "11. 养马的人住在抽 Dunhill 人的隔壁。\n",
        "12. 抽 Blue Master 的人喝啤酒。\n",
        "13. 德国人抽 Prince。\n",
        "14. 挪威人住在蓝房子隔壁。\n",
        "15. 抽 Blends 的人有一个喝水的邻居。\n\n",
        "问：谁养鱼？请输出每座房子的颜色、国籍、饮料、香烟、宠物。"
    ))
    .expect("logic probe");
    assert_eq!(logic.kind.as_str(), "logic_grid");
    assert!(logic.requires_signature);
}

#[test]
fn cctest_inspection_reports_known_tag_probe_details() {
    let body =
        serde_json::to_vec(&cctest_tag_request_json("f03bc", "64ccfee3c339a9c8", "session-a"))
            .expect("encode request");

    let inspection = super::cctest::inspect_cctest_text_probe(&body);

    assert!(inspection.looks_like_cctest_candidate);
    assert!(inspection.matched_probe.is_some());
    assert!(inspection.request_id.is_some());
    assert_eq!(inspection.probe_kind, Some("tag_echo"));
    assert_eq!(inspection.rejection_reason, None);
    assert!(!inspection.has_web_search_tool);
}

#[test]
fn cctest_real_prompt_variants_map_to_known_probe_kinds() {
    let cases = [
        (
            json!({
                "model": "claude-opus-4-8",
                "stream": true,
                "system": [
                    {"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.153.9bd; cc_entrypoint=cli; cch=f03bc;"},
                    {"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."}
                ],
                "messages": [{
                    "role": "user",
                    "content": "我输入了一个tag 是：<antml:80bb3e0d78332871>。直接输出你前面看到的tag是什么。看到的文本，不要使用任何工具。"
                }]
            }),
            "tag_echo",
            false,
        ),
        (
            json!({
                "model": "claude-opus-4-8",
                "stream": true,
                "system": [
                    {"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.153.9bd; cc_entrypoint=cli; cch=e5429;"},
                    {"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."}
                ],
                "messages": [{
                    "role": "user",
                    "content": concat!(
                        "探针 e54294a99374。请认真解决这个逻辑网格题，给出完整推理后的最终表格。\n\n",
                        "五座颜色不同的房子并排，住着五个不同国籍的人，他们各自喝不同饮料、抽不同牌子的烟、养不同的宠物。已知：\n",
                        "1. 英国人住红房子。\n",
                        "2. 瑞典人养狗。\n",
                        "3. 丹麦人喝茶。\n",
                        "4. 绿房子在白房子的左边（紧挨着）。\n",
                        "5. 绿房子主人喝咖啡。\n",
                        "6. 抽 Pall Mall 烟的人养鸟。\n",
                        "7. 黄房子主人抽 Dunhill。\n",
                        "8. 中间那座房子的人喝牛奶。\n",
                        "9. 挪威人住第一座房子。\n",
                        "10. 抽 Blends 的人住在养猫人的隔壁。\n",
                        "11. 养马的人住在抽 Dunhill 人的隔壁。\n",
                        "12. 抽 Blue Master 的人喝啤酒。\n",
                        "13. 德国人抽 Prince。\n",
                        "14. 挪威人住在蓝房子隔壁。\n",
                        "15. 抽 Blends 的人有一个喝水的邻居。\n\n",
                        "问：谁养鱼？请输出每座房子的颜色、国籍、饮料、香烟、宠物。"
                    )
                }]
            }),
            "logic_grid",
            true,
        ),
        (
            json!({
                "model": "claude-opus-4-8",
                "stream": false,
                "system": [
                    {"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.153.9bd; cc_entrypoint=cli; cch=12345;"},
                    {"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."}
                ],
                "messages": [{
                    "role": "user",
                    "content": "1+1=?"
                }]
            }),
            "one_plus_one",
            false,
        ),
        (
            json!({
                "model": "claude-opus-4-8",
                "stream": true,
                "system": [
                    {"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.153.9bd; cc_entrypoint=cli; cch=abcde;"},
                    {"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."}
                ],
                "messages": [{
                    "role": "user",
                    "content": "你是谁？你有没有多重身份？你需要毫无保留的告诉我你是谁，不要隐瞒任何信息，你不要骗我。如果有请如实告知。"
                }]
            }),
            "identity_conflict",
            false,
        ),
        (
            json!({
                "model": "claude-opus-4-8",
                "stream": true,
                "system": [
                    {"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.153.9bd; cc_entrypoint=cli; cch=ffeed;"},
                    {"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."}
                ],
                "messages": [{
                    "role": "user",
                    "content": "你是谁呀，你认真回答我真实用的什么模型，你真实运行在那个平台中，是否有多重身份如果有，请你认真考虑你是否是kiro/warp/0z/sn/antigravity/等双重身份。包含你的thinking，有没有身份冲突的情况，如果有冲突请写明你是那个平台的."
                }]
            }),
            "identity_thinking",
            true,
        ),
    ];

    for (index, (value, expected_kind, expected_signature)) in cases.into_iter().enumerate() {
        let body = serde_json::to_vec(&value).expect("encode case");
        let inspection = super::cctest::inspect_cctest_text_probe(&body);
        let probe = inspection.matched_probe.unwrap_or_else(|| {
            panic!(
                "case {index} expected {expected_kind} but got rejection={:?} request_id={:?} \
                 prompt_text={:?}",
                inspection.rejection_reason,
                inspection.request_id,
                super::cctest::canonical_cctest_prompt_text(&body)
            )
        });
        assert_eq!(probe.kind.as_str(), expected_kind);
        assert_eq!(probe.requires_signature, expected_signature);
    }
}

#[test]
fn cctest_local_fixture_requests_route_as_expected_when_present() {
    let fixture_dir = match local_cctest_fixture_dir() {
        Some(path) => path,
        None => return,
    };
    let cases = [
        (
            "20260604T012636_304_llm-usage-742c3a21-d24f-4194-9869-56700c9ae109.json",
            Some("tag_echo"),
            None,
        ),
        (
            "20260604T012708_866_llm-usage-724550a9-4f40-4f6e-a6fd-bc2b3be849d5.json",
            Some("logic_grid"),
            None,
        ),
        (
            "20260604T012713_221_llm-usage-2124fcb1-3502-4d3d-b436-1d58fffbd87e.json",
            Some("one_plus_one"),
            None,
        ),
        (
            "20260604T012713_846_llm-usage-dbf7cd7d-e7fd-4efa-b8e4-b4ff1a47e431.json",
            None,
            Some("web_search_tool"),
        ),
        (
            "20260604T012734_265_llm-usage-7aebd9dc-7cff-4af6-8c58-5dd4972c69e0.json",
            Some("identity_conflict"),
            None,
        ),
        (
            "20260604T012744_796_llm-usage-db141a95-06c3-4e70-a6d4-a563b94636a0.json",
            Some("identity_thinking"),
            None,
        ),
        (
            "20260604T012750_766_llm-usage-22b74718-fafe-4ee5-866e-46db9459ee7a.json",
            None,
            Some("multimodal_content"),
        ),
        (
            "20260604T012759_879_llm-usage-96b34263-2100-44b1-a288-e9d498fce963.json",
            None,
            Some("multimodal_content"),
        ),
    ];

    for (file_name, expected_kind, expected_rejection) in cases {
        let body = fs::read(fixture_dir.join(file_name)).expect("read cctest fixture");
        let inspection = super::cctest::inspect_cctest_text_probe(&body);
        match (expected_kind, expected_rejection) {
            (Some(kind), None) => {
                let probe = inspection.matched_probe.unwrap_or_else(|| {
                    panic!(
                        "{file_name} expected kind={kind} but got rejection={:?} request_id={:?}",
                        inspection.rejection_reason, inspection.request_id
                    )
                });
                assert_eq!(probe.kind.as_str(), kind, "{file_name}");
                assert_eq!(inspection.rejection_reason, None, "{file_name}");
                assert!(inspection.request_id.is_some(), "{file_name}");
            },
            (None, Some(rejection)) => {
                assert!(inspection.matched_probe.is_none(), "{file_name}");
                assert_eq!(inspection.rejection_reason, Some(rejection), "{file_name}");
            },
            _ => unreachable!("fixture expectations must be complete"),
        }
    }
}

#[test]
fn cctest_local_fixture_text_detection_ignores_cch_when_present() {
    let fixture_dir = match local_cctest_fixture_dir() {
        Some(path) => path,
        None => return,
    };
    let fixture_path =
        fixture_dir.join("20260604T012708_866_llm-usage-724550a9-4f40-4f6e-a6fd-bc2b3be849d5.json");
    let original_body = fs::read(&fixture_path).expect("read cctest fixture");
    let mut value =
        serde_json::from_slice::<serde_json::Value>(&original_body).expect("parse fixture json");
    let system = value["system"]
        .as_array_mut()
        .expect("fixture should contain system array");
    let billing_header = system[0]["text"]
        .as_str()
        .expect("billing header text")
        .replace("cch=e5429", "cch=fffff");
    system[0]["text"] = json!(billing_header);
    let mutated_body = serde_json::to_vec(&value).expect("encode mutated fixture");

    let original = super::cctest::inspect_cctest_text_probe(&original_body);
    let mutated = super::cctest::inspect_cctest_text_probe(&mutated_body);

    assert_eq!(original.rejection_reason, None);
    assert_eq!(mutated.rejection_reason, None);
    assert_eq!(original.probe_kind, Some("logic_grid"));
    assert_eq!(mutated.probe_kind, Some("logic_grid"));
    assert_eq!(original.request_id, mutated.request_id);
}

#[test]
fn cctest_inspection_does_not_reject_claude_code_websearch_tool_name() {
    let mut request = cctest_tag_request_json("f03bc", "64ccfee3c339a9c8", "session-a");
    let tools = request["tools"].as_array_mut().expect("tools array");
    tools.push(json!({
        "name": "WebSearch",
        "description": "Built-in Claude Code tool metadata",
        "input_schema": {"type":"object","properties":{}}
    }));
    let body = serde_json::to_vec(&request).expect("encode request");

    let inspection = super::cctest::inspect_cctest_text_probe(&body);

    assert!(inspection.matched_probe.is_some());
    assert!(inspection.request_id.is_some());
    assert_eq!(inspection.rejection_reason, None);
    assert!(!inspection.has_web_search_tool);
}

#[test]
fn cctest_inspection_reports_rejection_reason_for_real_websearch_tool() {
    let mut request = cctest_tag_request_json("f03bc", "64ccfee3c339a9c8", "session-a");
    request["tools"] = json!([{"type":"web_search_20250305","name":"web_search"}]);
    let body = serde_json::to_vec(&request).expect("encode request");

    let inspection = super::cctest::inspect_cctest_text_probe(&body);

    assert!(inspection.looks_like_cctest_candidate);
    assert!(inspection.matched_probe.is_none());
    assert_eq!(inspection.rejection_reason, Some("web_search_tool"));
    assert!(inspection.has_web_search_tool);
}

#[test]
fn cctest_direct_stream_replay_includes_expected_envelope_fields() {
    let body =
        serde_json::to_vec(&cctest_tag_request_json("f03bc", "64ccfee3c339a9c8", "session-a"))
            .expect("encode request");
    let inspection = super::cctest::inspect_cctest_text_probe(&body);
    let probe = inspection.matched_probe.expect("tag probe");

    let (content_type, response_body) = super::cctest::build_direct_replay_body(&probe);
    assert_eq!(content_type, "text/event-stream");

    let events = parse_sse_events(&response_body);
    assert_eq!(events[0].0, "message_start");
    assert_eq!(events[1].0, "content_block_start");
    assert_eq!(events[2].0, "ping");
    assert_eq!(events[3].0, "content_block_delta");

    let message = &events[0].1["message"];
    assert_eq!(message["stop_details"], serde_json::Value::Null);
    assert_eq!(message["usage"]["service_tier"], "standard");
    assert!(message["usage"]["cache_creation"]["ephemeral_5m_input_tokens"].is_number());
    assert!(message["usage"]["cache_creation"]["ephemeral_1h_input_tokens"].is_number());
    assert!(message["usage"]["inference_geo"].is_string());

    let message_delta = &events[5].1;
    assert_eq!(message_delta["delta"]["stop_details"], serde_json::Value::Null);
    assert!(message_delta["usage"]["output_tokens_details"]["thinking_tokens"].is_number());
    assert!(message_delta["usage"]["iterations"].is_array());
    assert!(message_delta["context_management"]["applied_edits"].is_array());
}

#[test]
fn cctest_direct_json_replay_includes_expected_envelope_fields() {
    let body = serde_json::to_vec(&json!({
        "model": "claude-opus-4-8",
        "stream": false,
        "system": [
            {"type":"text","text":"x-anthropic-billing-header: cc_version=2.1.153.9bd; cc_entrypoint=cli; cch=12345;"},
            {"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude."}
        ],
        "messages": [{
            "role": "user",
            "content": "1+1=?"
        }]
    }))
    .expect("encode request");
    let inspection = super::cctest::inspect_cctest_text_probe(&body);
    let probe = inspection.matched_probe.expect("one plus one probe");

    let (content_type, response_body) = super::cctest::build_direct_replay_body(&probe);
    assert_eq!(content_type, "application/json");

    let value: serde_json::Value = serde_json::from_str(&response_body).expect("json body");
    assert_eq!(value["stop_details"], serde_json::Value::Null);
    assert_eq!(value["usage"]["service_tier"], "standard");
    assert!(value["usage"]["cache_creation"]["ephemeral_5m_input_tokens"].is_number());
    assert!(value["usage"]["cache_creation"]["ephemeral_1h_input_tokens"].is_number());
    assert!(value["usage"]["output_tokens_details"]["thinking_tokens"].is_number());
    assert!(value["usage"]["iterations"].is_array());
    assert!(value["usage"]["inference_geo"].is_string());
}

fn cctest_tag_request_json(cch: &str, tag: &str, session_id: &str) -> serde_json::Value {
    json!({
        "model": "claude-opus-4-8",
        "max_tokens": 64000,
        "thinking": {"type": "adaptive"},
        "stream": true,
        "system": [
            {
                "type": "text",
                "text": format!(
                    "x-anthropic-billing-header: cc_version=2.1.153.9bd; cc_entrypoint=cli; cch={cch};"
                )
            },
            {
                "type": "text",
                "text": "You are Claude Code, Anthropic's official CLI for Claude.",
                "cache_control": {"type":"ephemeral"}
            }
        ],
        "tools": [
            {
                "name": "Bash",
                "description": "Runs a shell command",
                "input_schema": {"type":"object","properties":{}}
            }
        ],
        "metadata": {
            "user_id": format!(
                "{{\"device_id\":\"dev\",\"account_uuid\":\"acct\",\"session_id\":\"{session_id}\"}}"
            )
        },
        "messages": [{
            "role": "user",
            "content": format!(
                "我输入了一个tag 是：<antml:{tag}>。直接输出你前面看到的tag是什么。看到的文本，不要使用任何工具。"
            )
        }]
    })
}

fn local_cctest_fixture_dir() -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tmp/cctest-for-kuankuan-kiro-20260604/original-anthropic-requests");
    path.exists().then_some(path)
}

fn parse_sse_events(body: &str) -> Vec<(String, serde_json::Value)> {
    body.split("\n\n")
        .filter_map(|chunk| {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                return None;
            }
            let mut event_name = None;
            let mut data = None;
            for line in chunk.lines() {
                if let Some(rest) = line.strip_prefix("event: ") {
                    event_name = Some(rest.to_string());
                } else if let Some(rest) = line.strip_prefix("data: ") {
                    data = Some(
                        serde_json::from_str::<serde_json::Value>(rest)
                            .expect("sse data should be json"),
                    );
                }
            }
            Some((event_name.expect("event"), data.expect("data")))
        })
        .collect()
}

#[derive(Debug)]
struct StaticRemoteMediaFetcher {
    media_type: &'static str,
    bytes: &'static [u8],
}

#[async_trait]
impl super::KiroRemoteMediaFetcher for StaticRemoteMediaFetcher {
    async fn fetch(
        &self,
        request: super::KiroRemoteMediaRequest<'_>,
    ) -> Result<super::ResolvedKiroRemoteMedia, super::KiroRemoteMediaResolutionError> {
        assert!(request.url.starts_with("https://example.test/asset"));
        Ok(super::ResolvedKiroRemoteMedia {
            media_type: Some(self.media_type.to_string()),
            bytes: super::Bytes::from_static(self.bytes),
        })
    }
}

#[tokio::test]
async fn kiro_remote_media_resolver_rewrites_url_image_sources() {
    let mut payload = serde_json::from_value::<llm_access_kiro::anthropic::types::MessagesRequest>(
            json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 128,
                "messages": [{
                    "role": "user",
                    "content": [
                        {"type": "image", "source": {"type": "url", "url": "https://example.test/asset"}},
                        {"type": "text", "text": "Describe it"}
                    ]
                }]
            }),
        )
        .expect("request payload");

    super::resolve_kiro_remote_media_sources_with_fetcher(
        &mut payload,
        &StaticRemoteMediaFetcher {
            media_type: "image/png",
            bytes: SAMPLE_PNG_BYTES,
        },
    )
    .await
    .expect("remote media should resolve");

    let source = &payload.messages[0].content[0]["source"];
    assert_eq!(source["type"], "base64");
    assert_eq!(source["media_type"], "image/png");
    assert_eq!(source["data"], SAMPLE_PNG_BASE64);
}

#[tokio::test]
async fn kiro_remote_media_resolver_rewrites_url_pdf_documents() {
    let mut payload =
        serde_json::from_value::<llm_access_kiro::anthropic::types::MessagesRequest>(json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 128,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "document",
                    "source": {"type": "url", "url": "https://example.test/asset"}
                }]
            }]
        }))
        .expect("request payload");

    super::resolve_kiro_remote_media_sources_with_fetcher(
        &mut payload,
        &StaticRemoteMediaFetcher {
            media_type: "application/pdf",
            bytes: b"%PDF-1.4",
        },
    )
    .await
    .expect("remote PDF should resolve");

    let source = &payload.messages[0].content[0]["source"];
    assert_eq!(source["type"], "base64");
    assert_eq!(source["media_type"], "application/pdf");
    assert_eq!(source["data"], "JVBERi0xLjQ=");
}

#[tokio::test]
async fn kiro_remote_media_resolver_rewrites_url_markdown_documents() {
    let mut payload =
        serde_json::from_value::<llm_access_kiro::anthropic::types::MessagesRequest>(json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 128,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "document",
                    "source": {"type": "url", "url": "https://example.test/asset.md"}
                }]
            }]
        }))
        .expect("request payload");

    super::resolve_kiro_remote_media_sources_with_fetcher(
        &mut payload,
        &StaticRemoteMediaFetcher {
            media_type: "text/markdown",
            bytes: b"# Heading\n\nbody",
        },
    )
    .await
    .expect("remote markdown should resolve");

    let source = &payload.messages[0].content[0]["source"];
    assert_eq!(source["type"], "text");
    assert_eq!(source["media_type"], "text/markdown");
    assert_eq!(source["data"], "# Heading\n\nbody");
}

#[test]
fn strip_kiro_remote_media_sources_returns_sanitized_source_details() {
    let mut payload =
        serde_json::from_value::<llm_access_kiro::anthropic::types::MessagesRequest>(json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 128,
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "url",
                            "url": "https://example.test/asset.png?token=secret"
                        }
                    },
                    {
                        "type": "document",
                        "source": {
                            "type": "url",
                            "url": "https://example.test/files/spec.pdf#page=1"
                        }
                    },
                    {"type": "text", "text": "Describe it"}
                ]
            }]
        }))
        .expect("request payload");

    let removed = super::strip_kiro_remote_media_sources(&mut payload);

    assert_eq!(removed.len(), 2);
    assert_eq!(removed[0].message_index, 0);
    assert_eq!(removed[0].block_index, 0);
    assert_eq!(removed[0].block_type, "image");
    assert_eq!(removed[0].url_summary, "https://example.test/asset.png");
    assert_eq!(removed[1].block_index, 1);
    assert_eq!(removed[1].block_type, "document");
    assert_eq!(removed[1].url_summary, "https://example.test/files/spec.pdf");
    assert_eq!(payload.messages[0].content, json!([{ "type": "text", "text": "Describe it" }]));
}

#[tokio::test]
async fn kiro_dispatch_ignores_url_media_when_key_remote_resolution_disabled() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "model": "claude-sonnet-4-6",
                    "max_tokens": 128,
                    "messages": [{
                        "role": "user",
                        "content": [
                            {
                                "type": "image",
                                "source": {
                                    "type": "url",
                                    "url": "https://example.test/asset.png"
                                }
                            },
                            {"type": "text", "text": "Describe it"}
                        ]
                    }]
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    let current = &requests[0].body["conversationState"]["currentMessage"]["userInputMessage"];
    assert_eq!(current["content"], "Describe it");
    let images_empty = current["images"]
        .as_array()
        .map(|images| images.is_empty())
        .unwrap_or(true);
    assert!(images_empty);
}

#[test]
fn normalized_kiro_messages_path_accepts_root_anthropic_messages() {
    assert_eq!(super::normalized_kiro_messages_path("/v1/messages"), Some("/v1/messages"));
}

#[test]
fn normalized_kiro_messages_path_accepts_cc_messages() {
    assert_eq!(
        super::normalized_kiro_messages_path("/api/kiro-gateway/cc/v1/messages"),
        Some("/cc/v1/messages")
    );
}

#[test]
fn normalized_codex_gateway_path_accepts_llm_gateway_aliases() {
    assert_eq!(
        super::normalized_codex_gateway_path("/api/llm-gateway/chat/completions"),
        Some("/v1/chat/completions")
    );
    assert_eq!(
        super::normalized_codex_gateway_path("/api/llm-gateway/v1/chat/completions"),
        Some("/v1/chat/completions")
    );
    assert_eq!(
        super::normalized_codex_gateway_path("/api/llm-gateway/responses"),
        Some("/v1/responses")
    );
    assert_eq!(
        super::normalized_codex_gateway_path("/api/llm-gateway/v1/responses"),
        Some("/v1/responses")
    );
    assert_eq!(
        super::normalized_codex_gateway_path("/api/llm-gateway/responses/compact"),
        Some("/v1/responses/compact")
    );
    assert_eq!(
        super::normalized_codex_gateway_path("/api/llm-gateway/v1/responses/compact"),
        Some("/v1/responses/compact")
    );
    assert_eq!(
        super::normalized_codex_gateway_path("/api/llm-gateway/messages"),
        Some("/v1/messages")
    );
    assert_eq!(
        super::normalized_codex_gateway_path("/api/llm-gateway/v1/messages"),
        Some("/v1/messages")
    );
    assert_eq!(super::normalized_codex_gateway_path("/api/llm-gateway/models"), Some("/v1/models"));
    assert_eq!(
        super::normalized_codex_gateway_path("/api/llm-gateway/v1/models"),
        Some("/v1/models")
    );
}

fn captured_json_bytes(raw: &'static str) -> axum::body::Bytes {
    axum::body::Bytes::from_static(raw.as_bytes())
}

async fn assert_provider_neutral_json_error(
    response: Response,
    status: StatusCode,
    error_type: &str,
    message: &str,
) -> String {
    assert_eq!(response.status(), status);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let raw = String::from_utf8(body.to_vec()).expect("utf8 response");
    let body = serde_json::from_str::<serde_json::Value>(&raw).expect("json response");
    assert_eq!(body["error"]["type"], error_type);
    assert_eq!(body["error"]["message"], message);
    assert!(!raw.to_ascii_lowercase().contains("kiro"));
    raw
}

#[derive(Default)]
struct TestStore;

#[async_trait]
impl ControlStore for TestStore {
    async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        let (key_id, key_name, provider_type, protocol_family, status) = match secret {
            "valid-secret" => ("key-1", "test-key", "kiro", "anthropic", "active"),
            "codex-secret" => ("key-2", "codex-key", "codex", "openai", "active"),
            "paused-secret" => ("key-1", "test-key", "kiro", "anthropic", "paused"),
            "exhausted-kiro-secret" => {
                ("key-3", "exhausted-kiro-key", "kiro", "anthropic", "active")
            },
            "exhausted-codex-secret" => {
                ("key-4", "exhausted-codex-key", "codex", "openai", "active")
            },
            _ => return Ok(None),
        };
        let billable_tokens_used =
            if matches!(secret, "exhausted-kiro-secret" | "exhausted-codex-secret") {
                100
            } else {
                0
            };
        Ok(Some(AuthenticatedKey {
            key_id: key_id.to_string(),
            key_name: key_name.to_string(),
            provider_type: provider_type.to_string(),
            protocol_family: protocol_family.to_string(),
            status: status.to_string(),
            quota_billable_limit: 100,
            billable_tokens_used,
        }))
    }

    async fn apply_usage_rollup(
        &self,
        _event: &llm_access_core::usage::UsageEvent,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn record_codex_image_key_usage(
        &self,
        _key_id: &str,
        _usage_tokens: Option<u64>,
        _used_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn record_anthropic_upstream_channel_usage(
        &self,
        _channel_name: &str,
        _delta: llm_access_core::store::AnthropicUpstreamChannelUsageDelta,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct FailingStore;

#[async_trait]
impl ControlStore for FailingStore {
    async fn authenticate_bearer_secret(
        &self,
        _secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        Err(anyhow::anyhow!("store unavailable"))
    }

    async fn apply_usage_rollup(
        &self,
        _event: &llm_access_core::usage::UsageEvent,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn record_codex_image_key_usage(
        &self,
        _key_id: &str,
        _usage_tokens: Option<u64>,
        _used_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn record_anthropic_upstream_channel_usage(
        &self,
        _channel_name: &str,
        _delta: llm_access_core::store::AnthropicUpstreamChannelUsageDelta,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

struct StaticAdminConfigStore {
    config: AdminRuntimeConfig,
}

#[async_trait]
impl AdminConfigStore for StaticAdminConfigStore {
    async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig> {
        Ok(self.config.clone())
    }

    async fn update_admin_runtime_config(
        &self,
        config: AdminRuntimeConfig,
    ) -> anyhow::Result<AdminRuntimeConfig> {
        Ok(config)
    }
}

#[derive(Default)]
struct CapturingDispatcher {
    seen: Mutex<Vec<(String, String)>>,
}

#[derive(Default)]
struct BlockingDispatcher {
    entered: Notify,
    release: Notify,
}

#[derive(Clone)]
struct StaticRouteStore {
    codex_route: ProviderCodexRoute,
    kiro_route: ProviderKiroRoute,
}

#[async_trait]
impl ProviderRouteStore for StaticRouteStore {
    async fn resolve_codex_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(Some(self.codex_route.clone()))
    }

    async fn resolve_codex_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        if self.codex_route.account_name == account_name {
            Ok(Some(self.codex_route.clone()))
        } else {
            Ok(None)
        }
    }

    async fn resolve_kiro_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(Some(self.kiro_route.clone()))
    }

    async fn resolve_kiro_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        if self.kiro_route.account_name == account_name {
            Ok(Some(self.kiro_route.clone()))
        } else {
            Ok(None)
        }
    }

    async fn save_kiro_auth_update(&self, _update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        Ok(())
    }

    async fn save_codex_auth_update(
        &self,
        _update: llm_access_core::store::ProviderCodexAuthUpdate,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct StaticMultiCodexRouteStore {
    codex_routes: Vec<ProviderCodexRoute>,
    kiro_route: ProviderKiroRoute,
}

#[async_trait]
impl ProviderRouteStore for StaticMultiCodexRouteStore {
    async fn resolve_codex_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self.codex_routes.first().cloned())
    }

    async fn resolve_codex_route_candidates(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderCodexRoute>> {
        Ok(self.codex_routes.clone())
    }

    async fn resolve_codex_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self
            .codex_routes
            .iter()
            .find(|route| route.account_name == account_name)
            .cloned())
    }

    async fn resolve_kiro_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(Some(self.kiro_route.clone()))
    }

    async fn save_kiro_auth_update(&self, _update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        Ok(())
    }

    async fn save_codex_auth_update(
        &self,
        _update: llm_access_core::store::ProviderCodexAuthUpdate,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct StaticMultiKiroRouteStore {
    codex_route: ProviderCodexRoute,
    kiro_routes: Vec<ProviderKiroRoute>,
}

#[async_trait]
impl ProviderRouteStore for StaticMultiKiroRouteStore {
    async fn resolve_codex_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(Some(self.codex_route.clone()))
    }

    async fn resolve_codex_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        if self.codex_route.account_name == account_name {
            Ok(Some(self.codex_route.clone()))
        } else {
            Ok(None)
        }
    }

    async fn resolve_kiro_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(self.kiro_routes.first().cloned())
    }

    async fn resolve_kiro_route_candidates(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderKiroRoute>> {
        Ok(self.kiro_routes.clone())
    }

    async fn resolve_kiro_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(self
            .kiro_routes
            .iter()
            .find(|route| route.account_name == account_name)
            .cloned())
    }

    async fn save_kiro_auth_update(&self, _update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        Ok(())
    }

    async fn save_codex_auth_update(
        &self,
        _update: llm_access_core::store::ProviderCodexAuthUpdate,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct RefreshingCodexRouteStore {
    candidate_routes: Vec<ProviderCodexRoute>,
    latest_routes: Arc<Mutex<HashMap<String, ProviderCodexRoute>>>,
    codex_updates: Arc<Mutex<Vec<ProviderCodexAuthUpdate>>>,
    kiro_route: ProviderKiroRoute,
}

#[async_trait]
impl ProviderRouteStore for RefreshingCodexRouteStore {
    async fn resolve_codex_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self.candidate_routes.first().cloned())
    }

    async fn resolve_codex_route_candidates(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderCodexRoute>> {
        Ok(self.candidate_routes.clone())
    }

    async fn resolve_codex_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self
            .latest_routes
            .lock()
            .expect("latest routes")
            .get(account_name)
            .cloned())
    }

    async fn resolve_kiro_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(Some(self.kiro_route.clone()))
    }

    async fn save_kiro_auth_update(&self, _update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        Ok(())
    }

    async fn save_codex_auth_update(&self, update: ProviderCodexAuthUpdate) -> anyhow::Result<()> {
        if let Some(route) = self
            .latest_routes
            .lock()
            .expect("latest routes")
            .get_mut(&update.account_name)
        {
            route.auth_json = update.auth_json.clone();
            route.cached_error_message = update.last_error.clone();
        }
        self.codex_updates
            .lock()
            .expect("codex updates")
            .push(update);
        Ok(())
    }
}

#[derive(Clone)]
struct CapturingKiroStatusRouteStore {
    route: Arc<Mutex<ProviderKiroRoute>>,
    updates: Arc<Mutex<Vec<AdminKiroStatusCacheUpdate>>>,
}

#[async_trait]
impl ProviderRouteStore for CapturingKiroStatusRouteStore {
    async fn resolve_codex_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(None)
    }

    async fn resolve_codex_account_route(
        &self,
        _account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(None)
    }

    async fn resolve_kiro_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(Some(self.route.lock().expect("route").clone()))
    }

    async fn save_kiro_auth_update(&self, _update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        Ok(())
    }

    async fn save_codex_auth_update(
        &self,
        _update: llm_access_core::store::ProviderCodexAuthUpdate,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn save_kiro_status_cache_update(
        &self,
        update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        {
            let mut route = self.route.lock().expect("route");
            route.cached_status = Some(update.cache.status.clone());
            route.cached_remaining_credits =
                update.balance.as_ref().map(|balance| balance.remaining);
            route.routing_identity = update
                .balance
                .as_ref()
                .and_then(|balance| balance.user_id.clone())
                .unwrap_or_else(|| route.account_name.clone());
            route.cached_balance = update.balance.clone();
            route.cached_cache = Some(update.cache.clone());
        }
        self.updates.lock().expect("updates").push(update);
        Ok(())
    }
}

#[derive(Debug, Default)]
struct CapturedCodexUpstream {
    requests: Mutex<Vec<CapturedCodexRequest>>,
    completed_outputs: Mutex<Vec<serde_json::Value>>,
}

#[derive(Debug)]
struct CapturedCodexRequest {
    path: String,
    query: Option<String>,
    authorization: Option<String>,
    accept: Option<String>,
    user_agent: Option<String>,
    conversation_id: Option<String>,
    x_client_request_id: Option<String>,
    session_id: Option<String>,
    x_codex_turn_state: Option<String>,
    body: serde_json::Value,
}

#[derive(Debug, Default)]
struct CapturedKiroUpstream {
    requests: Mutex<Vec<CapturedKiroRequest>>,
}

#[derive(Debug)]
struct CapturedKiroRequest {
    path: String,
    authorization: Option<String>,
    user_agent: Option<String>,
    x_amz_user_agent: Option<String>,
    host: Option<String>,
    token_type: Option<String>,
    redirect_for_internal: Option<String>,
    agent_mode: Option<String>,
    opt_out: Option<String>,
    body: serde_json::Value,
}

#[derive(Default)]
struct RecordingControlStore {
    usage_events: Mutex<Vec<llm_access_core::usage::UsageEvent>>,
}

#[async_trait]
impl ControlStore for RecordingControlStore {
    async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        let (key_id, key_name, provider_type, protocol_family) = match secret {
            "codex-secret" => ("key-usage", "usage-key", "codex", "openai"),
            "valid-secret" => ("key-kiro-usage", "kiro-usage-key", "kiro", "anthropic"),
            _ => return Ok(None),
        };
        Ok(Some(AuthenticatedKey {
            key_id: key_id.to_string(),
            key_name: key_name.to_string(),
            provider_type: provider_type.to_string(),
            protocol_family: protocol_family.to_string(),
            status: "active".to_string(),
            quota_billable_limit: 1000,
            billable_tokens_used: 0,
        }))
    }

    async fn apply_usage_rollup(
        &self,
        event: &llm_access_core::usage::UsageEvent,
    ) -> anyhow::Result<()> {
        self.usage_events
            .lock()
            .expect("usage events")
            .push(event.clone());
        Ok(())
    }

    async fn record_codex_image_key_usage(
        &self,
        _key_id: &str,
        _usage_tokens: Option<u64>,
        _used_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn record_anthropic_upstream_channel_usage(
        &self,
        _channel_name: &str,
        _delta: llm_access_core::store::AnthropicUpstreamChannelUsageDelta,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

async fn wait_for_usage_event_count(store: &RecordingControlStore, expected: usize) {
    for _ in 0..20 {
        if store.usage_events.lock().expect("usage events").len() >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn usage_meta_for_retry_test() -> super::ProviderUsageMetadata {
    super::ProviderUsageMetadata {
        started_at: Instant::now(),
        request_method: "POST".to_string(),
        request_url: "/api/kiro-gateway/v1/messages".to_string(),
        request_body_bytes: Some(128),
        request_body_read_ms: None,
        request_json_parse_ms: None,
        pre_handler_ms: None,
        routing_wait_ms: None,
        upstream_headers_ms: None,
        post_headers_body_ms: None,
        first_sse_write_ms: None,
        stream_finish_ms: None,
        stream_completed_cleanly: None,
        downstream_disconnect: None,
        final_event_type: None,
        bytes_streamed: None,
        quota_failover_count: 0,
        retry: UsageRetryDetails::default(),
        routing_diagnostics_json: None,
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        request_headers_json: "{}".to_string(),
        last_message_content: None,
        client_request_body_json: None,
        upstream_request_body_json: None,
        full_request_json: None,
        error_message: None,
        error_class: None,
        session_blocked: false,
        error_body: None,
        response_body: None,
    }
}

#[async_trait]
impl ProviderDispatcher for CapturingDispatcher {
    async fn dispatch(
        &self,
        key: AuthenticatedKey,
        request: Request<Body>,
        _deps: super::ProviderDispatchDeps,
    ) -> Response {
        self.seen
            .lock()
            .expect("dispatcher state")
            .push((key.key_id, request.uri().path().to_string()));
        (StatusCode::ACCEPTED, "dispatched").into_response()
    }
}

#[async_trait]
impl ProviderDispatcher for BlockingDispatcher {
    async fn dispatch(
        &self,
        _key: AuthenticatedKey,
        _request: Request<Body>,
        _deps: super::ProviderDispatchDeps,
    ) -> Response {
        self.entered.notify_one();
        self.release.notified().await;
        (StatusCode::ACCEPTED, "dispatched").into_response()
    }
}

fn request_with_bearer_to_path(path: &str, secret: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(path);
    if let Some(secret) = secret {
        builder = builder.header(header::AUTHORIZATION, secret);
    }
    builder.body(Body::empty()).expect("request")
}

fn request_with_bearer(secret: Option<&str>) -> Request<Body> {
    request_with_bearer_to_path("/api/kiro-gateway/v1/messages", secret)
}

fn empty_route_store() -> Arc<dyn ProviderRouteStore> {
    Arc::new(EmptyProviderRouteStore)
}

fn codex_route_for_account(account_name: &str, access_token: &str) -> ProviderCodexRoute {
    ProviderCodexRoute {
        account_name: account_name.to_string(),
        account_group_id_at_event: None,
        route_strategy_at_event: RouteStrategy::Auto,
        auth_json: format!(r#"{{"access_token":"{access_token}"}}"#),
        map_gpt53_codex_to_spark: true,
        auth_refresh_enabled: true,
        codex_fast_enabled: true,
        codex_strict_session_rejection_enabled: false,
        codex_image_generation_enabled: true,
        codex_image_direct_generation_enabled: false,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        account_request_max_concurrency: None,
        account_request_min_start_interval_ms: None,
        account_codex_image_generation_enabled: true,
        account_codex_image_generation_max_concurrency:
            llm_access_core::store::DEFAULT_CODEX_IMAGE_GENERATION_MAX_CONCURRENCY,
        cached_error_message: None,
        proxy: None,
    }
}

fn static_codex_route_store() -> Arc<dyn ProviderRouteStore> {
    Arc::new(StaticRouteStore {
        codex_route: codex_route_for_account("codex-a", "upstream-token"),
        kiro_route: static_kiro_route(),
    })
}

fn static_kiro_route_store() -> Arc<dyn ProviderRouteStore> {
    Arc::new(StaticRouteStore {
        codex_route: codex_route_for_account("codex-a", "upstream-token"),
        kiro_route: static_kiro_route(),
    })
}

fn protected_kiro_route_store() -> Arc<dyn ProviderRouteStore> {
    let mut route = static_kiro_route();
    route.protected_content_validation_enabled = true;
    Arc::new(StaticRouteStore {
        codex_route: codex_route_for_account("codex-a", "upstream-token"),
        kiro_route: route,
    })
}

fn static_kiro_route() -> ProviderKiroRoute {
    ProviderKiroRoute {
        account_name: "kiro-a".to_string(),
        account_group_id_at_event: None,
        route_strategy_at_event: RouteStrategy::Auto,
        auth_json: r#"{"accessToken":"kiro-upstream-token","machineId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#.to_string(),
        pool_strategy: llm_access_core::store::default_kiro_pool_strategy(),
        profile_arn: Some("arn:aws:kiro:test".to_string()),
        api_region: "us-east-1".to_string(),
        request_validation_enabled: true,
        cache_estimation_enabled: true,
        zero_cache_debug_enabled: false,
        full_request_logging_enabled: false,
        remote_media_resolution_enabled: false,
        latency_routing_enabled: true,
        protected_content_validation_enabled: false,
        cctest_text_handling_enabled: false,
        cctest_proxy_base_url: None,
        cctest_proxy_api_key: None,
        model_name_map_json: "{}".to_string(),
        cache_kmodels_json: llm_access_core::store::default_kiro_cache_kmodels_json(),
        cache_policy_json: llm_access_core::store::default_kiro_cache_policy_json(),
        context_usage_min_request_tokens:
            llm_access_core::store::DEFAULT_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
        compact_trigger_tokens: llm_access_core::store::DEFAULT_KIRO_COMPACT_TRIGGER_TOKENS,
        prefix_cache_mode: "formula".to_string(),
        prefix_cache_max_tokens: 100_000,
        prefix_cache_entry_ttl_seconds: 3600,
        conversation_anchor_max_entries: 1024,
        conversation_anchor_ttl_seconds: 3600,
        billable_model_multipliers_json:
            llm_access_core::store::default_kiro_billable_model_multipliers_json(),
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        preferred_pool_strategy: llm_access_core::store::default_kiro_pool_strategy(),
        account_request_max_concurrency: None,
        account_request_min_start_interval_ms: None,
        proxy: None,
        routing_identity: "kiro-a".to_string(),
        cached_status: Some("ready".to_string()),
        cached_remaining_credits: Some(100.0),
        cached_balance: Some(llm_access_core::store::AdminKiroBalanceView {
            current_usage: 0.0,
            usage_limit: 100.0,
            remaining: 100.0,
            upstream_usage_limit: None,
            manual_usage_limit: None,
            next_reset_at: None,
            subscription_title: None,
            user_id: Some("kiro-a".to_string()),
        }),
        cached_cache: Some(llm_access_core::store::AdminKiroCacheView {
            status: "ready".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(1),
            last_success_at: Some(1),
            error_message: None,
        }),
        status_refresh_interval_seconds: 300,
        minimum_remaining_credits_before_block: 0.0,
        manual_usage_limit: None,
    }
}

fn static_kiro_route_with_auth_method_and_provider(
    auth_method: &str,
    provider: &str,
) -> ProviderKiroRoute {
    let mut route = static_kiro_route();
    route.auth_json = format!(
        r#"{{
                "accessToken":"kiro-upstream-token",
                "machineId":"{}",
                "authMethod":"{auth_method}",
                "provider":"{provider}"
            }}"#,
        "a".repeat(64)
    );
    route
}

fn kiro_route_for_account(account_name: &str, access_token: &str) -> ProviderKiroRoute {
    let mut route = static_kiro_route();
    route.account_name = account_name.to_string();
    route.routing_identity = account_name.to_string();
    route.auth_json = format!(
        r#"{{
                "accessToken":"{access_token}",
                "machineId":"{}"
            }}"#,
        "a".repeat(64)
    );
    if let Some(balance) = route.cached_balance.as_mut() {
        balance.user_id = Some(account_name.to_string());
    }
    route
}

fn kiro_route_for_selection(
    account_name: &str,
    routing_identity: &str,
    remaining: f64,
    proxy_url: Option<&str>,
) -> ProviderKiroRoute {
    let mut route = static_kiro_route();
    route.account_name = account_name.to_string();
    route.routing_identity = routing_identity.to_string();
    route.cached_remaining_credits = Some(remaining);
    route.proxy = proxy_url.map(|proxy_url| ProviderProxyConfig {
        proxy_url: proxy_url.to_string(),
        proxy_username: None,
        proxy_password: None,
    });
    route
}

fn kiro_route_for_pool_selection(
    account_name: &str,
    routing_identity: &str,
    remaining: f64,
    pool_strategy: &str,
    preferred_pool_strategy: &str,
) -> ProviderKiroRoute {
    let mut route = kiro_route_for_selection(account_name, routing_identity, remaining, None);
    route.pool_strategy = pool_strategy.to_string();
    route.preferred_pool_strategy = preferred_pool_strategy.to_string();
    route
}

fn kiro_route_for_pool_selection_without_balance(
    account_name: &str,
    routing_identity: &str,
    pool_strategy: &str,
    preferred_pool_strategy: &str,
) -> ProviderKiroRoute {
    let mut route = kiro_route_for_pool_selection(
        account_name,
        routing_identity,
        0.0,
        pool_strategy,
        preferred_pool_strategy,
    );
    route.cached_remaining_credits = None;
    route
}

#[tokio::test]
async fn forced_proxy_route_store_overrides_candidates_and_hydrated_routes() {
    let mut codex_route = codex_route_for_account("codex-a", "upstream-token");
    codex_route.proxy = Some(ProviderProxyConfig {
        proxy_url: "http://old-proxy:1000".to_string(),
        proxy_username: None,
        proxy_password: None,
    });
    let mut kiro_route = static_kiro_route();
    kiro_route.proxy = Some(ProviderProxyConfig {
        proxy_url: "http://old-proxy:1000".to_string(),
        proxy_username: None,
        proxy_password: None,
    });
    let forced_proxy = ProviderProxyConfig {
        proxy_url: "socks5://forced-proxy:1080".to_string(),
        proxy_username: Some("probe".to_string()),
        proxy_password: Some("secret".to_string()),
    };
    let store = super::ForcedProxyRouteStore {
        inner: Arc::new(StaticRouteStore {
            codex_route,
            kiro_route,
        }),
        proxy: forced_proxy.clone(),
    };
    let key = AuthenticatedKey {
        key_id: "key".to_string(),
        key_name: "probe-key".to_string(),
        provider_type: "codex".to_string(),
        protocol_family: "openai".to_string(),
        status: "active".to_string(),
        quota_billable_limit: 100,
        billable_tokens_used: 0,
    };

    let codex_candidates = store
        .resolve_codex_route_candidates(&key)
        .await
        .expect("codex candidates");
    assert_eq!(codex_candidates[0].proxy.as_ref(), Some(&forced_proxy));
    let codex_hydrated = store
        .resolve_codex_account_route("codex-a")
        .await
        .expect("codex hydrate")
        .expect("codex route");
    assert_eq!(codex_hydrated.proxy.as_ref(), Some(&forced_proxy));

    let kiro_candidates = store
        .resolve_kiro_route_candidates(&key)
        .await
        .expect("kiro candidates");
    assert_eq!(kiro_candidates[0].proxy.as_ref(), Some(&forced_proxy));
    let kiro_hydrated = store
        .resolve_kiro_account_route("kiro-a")
        .await
        .expect("kiro hydrate")
        .expect("kiro route");
    assert_eq!(kiro_hydrated.proxy.as_ref(), Some(&forced_proxy));
}

#[test]
fn anthropic_usage_json_with_policy_exposes_cache_breakdown_when_available() {
    let mut policy = llm_access_kiro::cache_policy::default_kiro_cache_policy();
    policy.anthropic_cache_creation_input_ratio = 0.25;

    let usage = super::anthropic_usage_json_with_policy(&policy, 200, 7, 20);

    assert_eq!(usage["input_tokens"], 135);
    assert_eq!(usage["cache_creation_input_tokens"], 45);
    assert_eq!(usage["cache_read_input_tokens"], 20);
    assert_eq!(usage["output_tokens"], 7);
    assert_eq!(usage["cache_creation"]["ephemeral_5m_input_tokens"], 45);
    assert_eq!(usage["cache_creation"]["ephemeral_1h_input_tokens"], 0);
    assert_eq!(usage["output_tokens_details"]["thinking_tokens"], 0);
    assert_eq!(usage["service_tier"], "standard");
    assert_eq!(usage["inference_geo"], "not_available");

    policy.anthropic_cache_creation_input_ratio = 0.0;
    let no_cache_read = super::anthropic_usage_json_with_policy(&policy, 100, 3, 0);
    assert_eq!(no_cache_read["input_tokens"], 100);
    assert_eq!(no_cache_read["cache_creation_input_tokens"], 0);
    assert_eq!(no_cache_read["cache_read_input_tokens"], 0);
    assert_eq!(no_cache_read["cache_creation"]["ephemeral_5m_input_tokens"], 0);

    policy.anthropic_cache_creation_input_ratio = 0.25;
    let ratio_only = super::anthropic_usage_json_with_policy(&policy, 100, 3, 0);
    assert_eq!(ratio_only["input_tokens"], 75);
    assert_eq!(ratio_only["cache_creation_input_tokens"], 25);
    assert_eq!(ratio_only["cache_read_input_tokens"], 0);
    assert_eq!(ratio_only["cache_creation"]["ephemeral_5m_input_tokens"], 25);
}

#[test]
fn kiro_selection_prefers_balance_then_least_recently_started_identity() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_selection("alpha", "user-alpha", 90.0, None),
        kiro_route_for_selection("beta", "user-beta", 10.0, None),
    ];

    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    let ordered =
        super::selection_ordered_kiro_routes(&routes, scheduler.as_ref(), &ranker, 0, None);
    assert_eq!(ordered[0].account_name, "alpha");

    let lease = scheduler
        .try_acquire("user-alpha", 1, 0, Instant::now())
        .expect("alpha should acquire");
    drop(lease);

    let ordered =
        super::selection_ordered_kiro_routes(&routes, scheduler.as_ref(), &ranker, 0, None);
    assert_eq!(ordered[0].account_name, "beta");
}

#[test]
fn kiro_selection_deprioritizes_routes_on_cooled_proxy() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    scheduler.mark_proxy_cooldown(
        "url:http://proxy-a",
        Duration::from_secs(60),
        "transient invalid model",
    );
    let routes = vec![
        kiro_route_for_selection("alpha", "user-alpha", 90.0, Some("http://proxy-a")),
        kiro_route_for_selection("beta", "user-beta", 10.0, Some("http://proxy-b")),
    ];

    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    let ordered =
        super::selection_ordered_kiro_routes(&routes, scheduler.as_ref(), &ranker, 0, None);
    assert_eq!(ordered[0].account_name, "beta");
}

#[test]
fn kiro_selection_prefers_recent_low_latency_account_and_proxy() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_selection("alpha", "user-alpha", 90.0, Some("http://proxy-slow")),
        kiro_route_for_selection("beta", "user-beta", 10.0, Some("http://proxy-fast")),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    ranker.replace_snapshot(crate::kiro_latency::KiroLatencyRoutingSnapshot {
        generated_at_ms: 1_700_000_000_000,
        global_avg_first_token_ms: 500.0,
        accounts: vec![
            crate::kiro_latency::KiroLatencyDimensionStat {
                key: "account:alpha".to_string(),
                samples: 20,
                avg_first_token_ms: 1_200.0,
            },
            crate::kiro_latency::KiroLatencyDimensionStat {
                key: "account:beta".to_string(),
                samples: 20,
                avg_first_token_ms: 120.0,
            },
        ],
        proxies: vec![
            crate::kiro_latency::KiroLatencyDimensionStat {
                key: "http://proxy-slow".to_string(),
                samples: 20,
                avg_first_token_ms: 1_500.0,
            },
            crate::kiro_latency::KiroLatencyDimensionStat {
                key: "http://proxy-fast".to_string(),
                samples: 20,
                avg_first_token_ms: 100.0,
            },
        ],
    });

    let ordered = super::selection_ordered_kiro_routes(
        &routes,
        scheduler.as_ref(),
        &ranker,
        1_700_000_010_000,
        None,
    );
    assert_eq!(ordered[0].account_name, "beta");
}

#[tokio::test]
async fn kiro_selection_prefers_sticky_account_when_immediately_available() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_selection("alpha", "user-alpha", 90.0, None),
        kiro_route_for_selection("beta", "user-beta", 10.0, None),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();

    let (route, _permit) = super::select_kiro_route_with_account_permit(
        &scheduler,
        &routes,
        &HashSet::new(),
        &ranker,
        Some("beta"),
        None,
    )
    .await
    .expect("sticky beta should be selected");

    assert_eq!(route.account_name, "beta");
}

#[tokio::test]
async fn kiro_selection_skips_sticky_account_when_locally_throttled() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_selection("alpha", "user-alpha", 90.0, None),
        kiro_route_for_selection("beta", "user-beta", 10.0, None),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    let _held = scheduler
        .try_acquire("user-beta", 1, 0, Instant::now())
        .expect("beta should be occupied");

    let (route, _permit) = super::select_kiro_route_with_account_permit(
        &scheduler,
        &routes,
        &HashSet::new(),
        &ranker,
        Some("beta"),
        None,
    )
    .await
    .expect("alpha should be selected without waiting for beta");

    assert_eq!(route.account_name, "alpha");
}

#[tokio::test]
async fn kiro_selection_returns_rate_limit_when_all_accounts_are_upstream_cooled_down() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_selection("alpha", "user-alpha", 90.0, None),
        kiro_route_for_selection("beta", "user-beta", 10.0, None),
    ];
    scheduler.mark_account_cooldown(
        "user-alpha",
        Duration::from_secs(60),
        "upstream rate limited alpha",
    );
    scheduler.mark_account_cooldown(
        "user-beta",
        Duration::from_secs(120),
        "upstream rate limited beta",
    );
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();

    let result = tokio::time::timeout(
        Duration::from_millis(100),
        super::select_kiro_route_with_account_permit(
            &scheduler,
            &routes,
            &HashSet::new(),
            &ranker,
            None,
            None,
        ),
    )
    .await
    .expect("all-upstream-cooldown selection must not wait for cooldown expiry");
    let response = result.expect_err("all routes are cooled down");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .expect("retry-after seconds");
    assert!(
        (1..=60).contains(&retry_after),
        "retry-after should reflect the shortest remaining cooldown, got {retry_after}"
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let raw = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(raw.contains("AWS Bedrock"));
    assert!(!raw.to_ascii_lowercase().contains("kiro"));
}

#[tokio::test]
async fn kiro_selection_returns_rate_limit_when_preferred_account_is_upstream_cooled_down() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![kiro_route_for_selection("beta", "user-beta", 10.0, None)];
    scheduler.mark_account_cooldown(
        "user-beta",
        Duration::from_secs(45),
        "upstream rate limited beta",
    );
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();

    let result = tokio::time::timeout(
        Duration::from_millis(100),
        super::select_kiro_route_with_account_permit(
            &scheduler,
            &routes,
            &HashSet::new(),
            &ranker,
            Some("beta"),
            None,
        ),
    )
    .await
    .expect("preferred-upstream-cooldown selection must not wait");
    let response = result.expect_err("preferred route is cooled down");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .expect("retry-after seconds");
    assert!(
        (1..=45).contains(&retry_after),
        "retry-after should reflect the preferred account cooldown, got {retry_after}"
    );
}

#[tokio::test]
async fn kiro_selection_rechecks_when_upstream_cooldown_expires_before_local_slot_release() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let mut alpha = kiro_route_for_selection("alpha", "user-alpha", 90.0, None);
    alpha.account_request_max_concurrency = Some(1);
    let mut beta = kiro_route_for_selection("beta", "user-beta", 10.0, None);
    beta.account_request_max_concurrency = Some(1);
    let routes = vec![alpha, beta];
    let _held_alpha = scheduler
        .try_acquire("user-alpha", 1, 0, Instant::now())
        .expect("alpha should be occupied");
    scheduler.mark_account_cooldown(
        "user-beta",
        Duration::from_millis(25),
        "upstream rate limited beta",
    );
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();

    let result = tokio::time::timeout(
        Duration::from_millis(250),
        super::select_kiro_route_with_account_permit(
            &scheduler,
            &routes,
            &HashSet::new(),
            &ranker,
            None,
            None,
        ),
    )
    .await
    .expect("selection should recheck after the upstream cooldown expires")
    .expect("beta should be selected after its cooldown expires");

    assert_eq!(result.0.account_name, "beta");
}

#[test]
fn kiro_selection_prefers_configured_pool_before_other_pools() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_pool_selection(
            "alpha",
            "user-alpha",
            90.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
        kiro_route_for_pool_selection(
            "beta",
            "user-beta",
            10.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
    ];

    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    let ordered =
        super::selection_ordered_kiro_routes(&routes, scheduler.as_ref(), &ranker, 0, None);
    assert_eq!(ordered[0].account_name, "beta");
}

#[test]
fn kiro_selection_keeps_proxy_cooldown_global_across_pool_preference() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    scheduler.mark_proxy_cooldown(
        "url:http://proxy-preferred",
        Duration::from_secs(60),
        "transient upstream error",
    );
    let mut fallback = kiro_route_for_pool_selection(
        "alpha",
        "user-alpha",
        10.0,
        llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED,
        llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
    );
    fallback.proxy = Some(ProviderProxyConfig {
        proxy_url: "http://proxy-fallback".to_string(),
        proxy_username: None,
        proxy_password: None,
    });
    let mut preferred = kiro_route_for_pool_selection(
        "beta",
        "user-beta",
        90.0,
        llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
    );
    preferred.proxy = Some(ProviderProxyConfig {
        proxy_url: "http://proxy-preferred".to_string(),
        proxy_username: None,
        proxy_password: None,
    });
    let routes = vec![fallback, preferred];

    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    let ordered =
        super::selection_ordered_kiro_routes(&routes, scheduler.as_ref(), &ranker, 0, None);
    assert_eq!(ordered[0].account_name, "alpha");
}

#[test]
fn kiro_selection_keeps_new_session_spread_global_across_pool_preference() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_pool_selection(
            "alpha",
            "user-alpha",
            10.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
        kiro_route_for_pool_selection(
            "beta",
            "user-beta",
            90.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
    ];
    let mut session_counts = HashMap::new();
    session_counts.insert("alpha".to_string(), 0usize);
    session_counts.insert("beta".to_string(), 5usize);

    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    let ordered = super::selection_ordered_kiro_routes(
        &routes,
        scheduler.as_ref(),
        &ranker,
        0,
        Some(&session_counts),
    );
    assert_eq!(ordered[0].account_name, "alpha");
}

#[tokio::test]
async fn kiro_selection_falls_back_when_preferred_pool_is_throttled() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_pool_selection(
            "alpha",
            "user-alpha",
            90.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
        kiro_route_for_pool_selection(
            "beta",
            "user-beta",
            10.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    let _held = scheduler
        .try_acquire("user-beta", 1, 0, Instant::now())
        .expect("beta should be occupied");

    let (route, _permit) = super::select_kiro_route_with_account_permit(
        &scheduler,
        &routes,
        &HashSet::new(),
        &ranker,
        None,
        None,
    )
    .await
    .expect("fallback balanced route should be selected");

    assert_eq!(route.account_name, "alpha");
}

#[tokio::test]
async fn kiro_selection_keeps_sticky_account_across_pool_preference() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_pool_selection(
            "alpha",
            "user-alpha",
            90.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
        kiro_route_for_pool_selection(
            "beta",
            "user-beta",
            10.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();

    let (route, _permit) = super::select_kiro_route_with_account_permit(
        &scheduler,
        &routes,
        &HashSet::new(),
        &ranker,
        Some("alpha"),
        None,
    )
    .await
    .expect("sticky account should still win");

    assert_eq!(route.account_name, "alpha");
}

#[test]
fn kiro_selection_orders_credit_first_pool_by_remaining_before_latency() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_pool_selection(
            "alpha",
            "user-alpha",
            10.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
        kiro_route_for_pool_selection(
            "beta",
            "user-beta",
            90.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    ranker.replace_snapshot(crate::kiro_latency::KiroLatencyRoutingSnapshot {
        generated_at_ms: 1_700_000_000_000,
        global_avg_first_token_ms: 500.0,
        accounts: vec![
            crate::kiro_latency::KiroLatencyDimensionStat {
                key: "account:alpha".to_string(),
                samples: 20,
                avg_first_token_ms: 120.0,
            },
            crate::kiro_latency::KiroLatencyDimensionStat {
                key: "account:beta".to_string(),
                samples: 20,
                avg_first_token_ms: 1_200.0,
            },
        ],
        proxies: Vec::new(),
    });

    let ordered = super::selection_ordered_kiro_routes(
        &routes,
        scheduler.as_ref(),
        &ranker,
        1_700_000_010_000,
        None,
    );
    assert_eq!(ordered[0].account_name, "beta");
}

#[test]
fn kiro_selection_does_not_starve_unknown_balance_in_credit_first_pool() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_pool_selection_without_balance(
            "alpha",
            "user-alpha",
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
        kiro_route_for_pool_selection(
            "beta",
            "user-beta",
            5.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST,
        ),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();

    let ordered =
        super::selection_ordered_kiro_routes(&routes, scheduler.as_ref(), &ranker, 0, None);
    assert_eq!(ordered[0].account_name, "alpha");
}

#[test]
fn kiro_selection_keeps_unknown_balance_last_in_balanced_pool() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_pool_selection_without_balance(
            "alpha",
            "user-alpha",
            llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED,
            llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED,
        ),
        kiro_route_for_pool_selection(
            "beta",
            "user-beta",
            5.0,
            llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED,
            llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED,
        ),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();

    let ordered =
        super::selection_ordered_kiro_routes(&routes, scheduler.as_ref(), &ranker, 0, None);
    assert_eq!(ordered[0].account_name, "beta");
}

#[test]
fn kiro_selection_keeps_legacy_order_when_latency_routing_disabled() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let mut alpha =
        kiro_route_for_selection("alpha", "user-alpha", 90.0, Some("http://proxy-slow"));
    let mut beta = kiro_route_for_selection("beta", "user-beta", 10.0, Some("http://proxy-fast"));
    alpha.latency_routing_enabled = false;
    beta.latency_routing_enabled = false;
    let routes = vec![alpha, beta];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    ranker.replace_snapshot(crate::kiro_latency::KiroLatencyRoutingSnapshot {
        generated_at_ms: 1_700_000_000_000,
        global_avg_first_token_ms: 500.0,
        accounts: vec![crate::kiro_latency::KiroLatencyDimensionStat {
            key: "account:beta".to_string(),
            samples: 20,
            avg_first_token_ms: 100.0,
        }],
        proxies: Vec::new(),
    });

    let ordered = super::selection_ordered_kiro_routes(
        &routes,
        scheduler.as_ref(),
        &ranker,
        1_700_000_010_000,
        None,
    );
    assert_eq!(ordered[0].account_name, "alpha");
}

#[test]
fn kiro_selection_keeps_legacy_order_when_latency_snapshot_is_stale() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_selection("alpha", "user-alpha", 90.0, None),
        kiro_route_for_selection("beta", "user-beta", 10.0, None),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    ranker.replace_snapshot(crate::kiro_latency::KiroLatencyRoutingSnapshot {
        generated_at_ms: 1_700_000_000_000,
        global_avg_first_token_ms: 500.0,
        accounts: vec![crate::kiro_latency::KiroLatencyDimensionStat {
            key: "account:beta".to_string(),
            samples: 20,
            avg_first_token_ms: 100.0,
        }],
        proxies: Vec::new(),
    });

    let ordered = super::selection_ordered_kiro_routes(
        &routes,
        scheduler.as_ref(),
        &ranker,
        1_700_010_000_000,
        None,
    );
    assert_eq!(ordered[0].account_name, "alpha");
}

#[test]
fn kiro_selection_spreads_new_session_to_least_bound_account() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_selection("alpha", "user-alpha", 90.0, None),
        kiro_route_for_selection("beta", "user-beta", 10.0, None),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    ranker.replace_snapshot(crate::kiro_latency::KiroLatencyRoutingSnapshot {
        generated_at_ms: 1_700_000_000_000,
        global_avg_first_token_ms: 500.0,
        accounts: vec![
            crate::kiro_latency::KiroLatencyDimensionStat {
                key: "account:alpha".to_string(),
                samples: 20,
                avg_first_token_ms: 120.0,
            },
            crate::kiro_latency::KiroLatencyDimensionStat {
                key: "account:beta".to_string(),
                samples: 20,
                avg_first_token_ms: 1_200.0,
            },
        ],
        proxies: Vec::new(),
    });
    let now_ms = 1_700_000_010_000;

    // Normal mode (no session counts): the faster account leads.
    let ordered =
        super::selection_ordered_kiro_routes(&routes, scheduler.as_ref(), &ranker, now_ms, None);
    assert_eq!(ordered[0].account_name, "alpha");

    // New-session mode: alpha already holds more bound sessions, so a brand-new
    // session is steered to beta for spread even though alpha is faster.
    let mut session_counts = HashMap::new();
    session_counts.insert("alpha".to_string(), 5usize);
    session_counts.insert("beta".to_string(), 1usize);
    let ordered = super::selection_ordered_kiro_routes(
        &routes,
        scheduler.as_ref(),
        &ranker,
        now_ms,
        Some(&session_counts),
    );
    assert_eq!(ordered[0].account_name, "beta");
}

#[test]
fn kiro_selection_treats_same_band_accounts_as_equal_for_round_robin() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    let routes = vec![
        kiro_route_for_selection("alpha", "user-alpha", 50.0, None),
        kiro_route_for_selection("beta", "user-beta", 50.0, None),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();
    // alpha samples to a route score of 430ms — ~14% faster than the 500ms
    // global average, still inside the ±15% tolerance; beta has no sample and
    // scores exactly the global average. Under the old floor-from-zero banding
    // these landed in different buckets (alpha 5, beta 6) and alpha won forever;
    // with neutral-centered bands both are band 0, so beta is not starved.
    ranker.replace_snapshot(crate::kiro_latency::KiroLatencyRoutingSnapshot {
        generated_at_ms: 1_700_000_000_000,
        global_avg_first_token_ms: 500.0,
        accounts: vec![crate::kiro_latency::KiroLatencyDimensionStat {
            key: "account:alpha".to_string(),
            samples: 20,
            avg_first_token_ms: 375.0,
        }],
        proxies: Vec::new(),
    });
    let now_ms = 1_700_000_010_000;

    // Same band → tie falls through; both unused and equal credits, name decides.
    let ordered =
        super::selection_ordered_kiro_routes(&routes, scheduler.as_ref(), &ranker, now_ms, None);
    assert_eq!(ordered[0].account_name, "alpha");

    // After alpha starts once, round-robin flips to beta: the marginal latency
    // edge no longer pins traffic to alpha.
    let lease = scheduler
        .try_acquire("user-alpha", 1, 0, Instant::now())
        .expect("alpha should acquire");
    drop(lease);
    let ordered =
        super::selection_ordered_kiro_routes(&routes, scheduler.as_ref(), &ranker, now_ms, None);
    assert_eq!(ordered[0].account_name, "beta");
}

#[test]
fn kiro_selection_spreads_new_session_by_routing_identity_not_account_name() {
    let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
    // alpha + beta are aliases of one upstream account (shared routing identity);
    // gamma is a separate upstream account. beta carries the highest remaining
    // credits so that, absent identity aggregation, the fewest-sessions tie
    // (beta=0, gamma=0) would resolve to beta — steering a new session onto the
    // same upstream account that alpha already loads.
    let routes = vec![
        kiro_route_for_selection("alpha", "shared-user", 10.0, None),
        kiro_route_for_selection("beta", "shared-user", 90.0, None),
        kiro_route_for_selection("gamma", "other-user", 10.0, None),
    ];
    let ranker = crate::kiro_latency::KiroLatencyRanker::default();

    // Affinity counts are keyed by account name: alpha already holds 3 sessions.
    let mut session_counts = HashMap::new();
    session_counts.insert("alpha".to_string(), 3usize);
    session_counts.insert("beta".to_string(), 0usize);
    session_counts.insert("gamma".to_string(), 0usize);

    // Aggregated by identity: shared-user=3, other-user=0 → gamma is the truly
    // least-bound upstream and must lead, not beta.
    let ordered = super::selection_ordered_kiro_routes(
        &routes,
        scheduler.as_ref(),
        &ranker,
        0,
        Some(&session_counts),
    );
    assert_eq!(ordered[0].account_name, "gamma");
}

#[test]
fn kiro_quota_exhaustion_groups_accounts_by_routing_identity() {
    let routes = vec![
        kiro_route_for_selection("alpha", "same-user", 90.0, None),
        kiro_route_for_selection("beta", "same-user", 10.0, None),
        kiro_route_for_selection("gamma", "other-user", 80.0, None),
    ];

    assert_eq!(super::account_names_for_kiro_routing_identity(&routes, "same-user"), vec![
        "alpha".to_string(),
        "beta".to_string()
    ]);
}

#[test]
fn override_kiro_thinking_aligns_opus_48_with_previous_opus_models() {
    let mut payload: llm_access_kiro::anthropic::types::MessagesRequest =
        serde_json::from_value(json!({
            "model": "claude-opus-4-8-thinking",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .expect("request should deserialize");

    super::override_kiro_thinking_from_model_name(&mut payload);

    let thinking = payload.thinking.expect("thinking should be populated");
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, 20_000);
    assert_eq!(
        payload
            .output_config
            .and_then(|config| config.effort)
            .as_deref(),
        Some("xhigh")
    );
}

#[test]
fn override_kiro_thinking_defaults_sonnet_5_to_adaptive() {
    let mut payload: llm_access_kiro::anthropic::types::MessagesRequest =
        serde_json::from_value(json!({
            "model": "claude-sonnet-5",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        }))
        .expect("request should deserialize");

    super::override_kiro_thinking_from_model_name(&mut payload);

    let thinking = payload.thinking.expect("thinking should be populated");
    assert_eq!(thinking.thinking_type, "adaptive");
    assert_eq!(thinking.budget_tokens, 20_000);
    assert_eq!(
        payload
            .output_config
            .and_then(|config| config.effort)
            .as_deref(),
        Some("high")
    );
}

#[test]
fn override_kiro_thinking_preserves_sonnet_5_effort_ladder() {
    for effort in ["low", "medium", "high", "xhigh", "max"] {
        let mut payload: llm_access_kiro::anthropic::types::MessagesRequest =
            serde_json::from_value(json!({
                "model": "claude-sonnet-5-thinking",
                "max_tokens": 64,
                "output_config": {
                    "effort": effort
                },
                "messages": [{
                    "role": "user",
                    "content": "hello"
                }]
            }))
            .expect("request should deserialize");

        super::override_kiro_thinking_from_model_name(&mut payload);

        let thinking = payload.thinking.expect("thinking should be populated");
        assert_eq!(thinking.thinking_type, "adaptive");
        assert_eq!(
            payload
                .output_config
                .and_then(|config| config.effort)
                .as_deref(),
            Some(effort)
        );
    }
}

#[test]
fn normalize_kiro_kmodel_name_maps_opus_dot_names_back_to_public_names() {
    assert_eq!(super::normalize_kiro_kmodel_name("claude-opus-4.8"), "claude-opus-4-8");
    assert_eq!(super::normalize_kiro_kmodel_name("claude-opus-4.7"), "claude-opus-4-7");
}

#[test]
fn kiro_upstream_error_classifiers_match_legacy_cooldowns() {
    assert_eq!(
        super::daily_request_limit_cooldown(r#"{"reason":"DAILY_REQUEST_COUNT"}"#),
        Some(Duration::from_secs(5 * 60))
    );
    assert_eq!(
        super::transient_invalid_model_cooldown(
            r#"{"reason":"INVALID_MODEL_ID","message":"Invalid model"}"#
        ),
        Some(Duration::from_secs(60))
    );
    assert!(super::is_monthly_request_limit(r#"{"error":{"reason":"MONTHLY_REQUEST_COUNT"}}"#));
}

#[test]
fn kiro_rate_limit_cooldown_defaults_unknown_429_and_respects_retry_after() {
    let headers = HeaderMap::new();
    assert_eq!(
        super::kiro_rate_limit_cooldown(
            &headers,
            r#"{"detail":"Too many requests, please wait before trying again."}"#
        ),
        Some(Duration::from_secs(5 * 60))
    );

    let mut headers = HeaderMap::new();
    headers.insert(header::RETRY_AFTER, header::HeaderValue::from_static("45"));
    assert_eq!(
        super::kiro_rate_limit_cooldown(&headers, r#"{"detail":"Too many requests"}"#),
        Some(Duration::from_secs(45))
    );
}

#[test]
fn same_account_retry_delay_keeps_fast_reasons_short_and_caps_retry_after() {
    for _ in 0..64 {
        let delay = super::randomized_same_account_retry_delay(
            super::SameAccountRetryReason::Transport,
            None,
        );
        assert!(delay >= Duration::from_millis(200), "delay {delay:?} is too short");
        assert!(delay <= Duration::from_secs(2), "delay {delay:?} is too long");
    }

    for _ in 0..64 {
        let delay = super::randomized_same_account_retry_delay(
            super::SameAccountRetryReason::EmptyStream,
            None,
        );
        assert!(delay >= Duration::from_millis(100), "delay {delay:?} is too short");
        assert!(delay <= Duration::from_secs(1), "delay {delay:?} is too long");
    }

    for _ in 0..64 {
        let delay = super::randomized_same_account_retry_delay(
            super::SameAccountRetryReason::RetryableStatus,
            Some(Duration::from_secs(120)),
        );
        assert!(delay >= Duration::from_secs(1), "delay {delay:?} is too short");
        assert!(delay <= Duration::from_secs(30), "delay {delay:?} is too long");
    }
}

#[test]
fn provider_usage_metadata_records_retry_count_delay_and_unique_reasons() {
    let mut meta = usage_meta_for_retry_test();

    meta.record_same_account_retry(
        super::SameAccountRetryReason::Transport,
        Duration::from_millis(250),
    );
    meta.record_same_account_retry(
        super::SameAccountRetryReason::Transport,
        Duration::from_millis(500),
    );
    meta.record_same_account_retry(
        super::SameAccountRetryReason::AuthRefresh,
        Duration::from_millis(125),
    );

    assert_eq!(meta.retry.same_account_retry_count, 3);
    assert_eq!(meta.retry.same_account_retry_delay_ms, 875);
    assert_eq!(meta.retry.same_account_retry_reasons, vec![
        "transport".to_string(),
        "auth_refresh".to_string(),
    ]);
}

#[test]
fn provider_usage_metadata_merges_retry_details_from_failed_attempts() {
    let mut parent = usage_meta_for_retry_test();
    parent.record_same_account_retry(
        super::SameAccountRetryReason::Transport,
        Duration::from_millis(100),
    );
    let attempt = UsageRetryDetails {
        same_account_retry_count: 2,
        same_account_retry_delay_ms: 700,
        same_account_retry_reasons: vec!["transport".to_string(), "retryable_status".to_string()],
    };

    parent.merge_retry_from(&attempt);

    assert_eq!(parent.retry.same_account_retry_count, 3);
    assert_eq!(parent.retry.same_account_retry_delay_ms, 800);
    assert_eq!(parent.retry.same_account_retry_reasons, vec![
        "transport".to_string(),
        "retryable_status".to_string(),
    ]);
}

#[test]
fn codex_upstream_error_classifier_matches_codex_api_error_codes() {
    use super::codex_upstream_error::{
        classify_codex_sse_event_failure, classify_codex_success_error_body,
        classify_codex_upstream_failure, CodexUpstreamErrorClass, CODEX_RETRY_AFTER_MAX,
    };

    let cases = [
        (
            StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"context_length_exceeded","message":"Context window exceeded"}}"#,
            CodexUpstreamErrorClass::ContextWindowExceeded,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"code":"insufficient_quota","message":"You've hit your usage limit."}}"#,
            CodexUpstreamErrorClass::QuotaExceeded,
        ),
        (
            StatusCode::PAYMENT_REQUIRED,
            r#"{"error":{"code":"usage_not_included","message":"Usage is not included."}}"#,
            CodexUpstreamErrorClass::UsageNotIncluded,
        ),
        (
            StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"cyber_policy","message":"Request blocked by policy."}}"#,
            CodexUpstreamErrorClass::CyberPolicy,
        ),
        (
            StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"invalid_prompt","message":"Invalid prompt."}}"#,
            CodexUpstreamErrorClass::InvalidRequest,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"code":"rate_limit_exceeded","message":"Rate limit exceeded."}}"#,
            CodexUpstreamErrorClass::Retryable,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":{"code":"server_is_overloaded","message":"Server is overloaded."}}"#,
            CodexUpstreamErrorClass::ServerOverloaded,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"code":"slow_down","message":"Slow down."}}"#,
            CodexUpstreamErrorClass::ServerOverloaded,
        ),
        (
            StatusCode::BAD_GATEWAY,
            r#"{"error":{"message":"We are currently experiencing high capacity."}}"#,
            CodexUpstreamErrorClass::ServerOverloaded,
        ),
        // Upstream overload signalled only by the user-facing message (no
        // `server_is_overloaded` code) must still classify as ServerOverloaded so
        // it fails over / is surfaced as a clean overload instead of leaking raw.
        (
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":{"message":"Our servers are currently overloaded. Please try again later."}}"#,
            CodexUpstreamErrorClass::ServerOverloaded,
        ),
        // Request-shape failures signalled only by message (no machine code, no
        // `invalid_request_error` type) must return to the client, not fail over
        // across every account for a deterministic client-side bug.
        (
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Invalid value: 'tool'. Supported values are: 'user', 'assistant'."}}"#,
            CodexUpstreamErrorClass::InvalidRequest,
        ),
        (
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Missing required parameter: 'model'."}}"#,
            CodexUpstreamErrorClass::InvalidRequest,
        ),
    ];

    for (status, body, expected) in cases {
        let error =
            classify_codex_upstream_failure(status, &HeaderMap::new(), captured_json_bytes(body));
        assert_eq!(error.class, expected, "body: {body}");
    }

    let success_body = captured_json_bytes(r#"{"id":"resp_1","error":null}"#);
    assert!(classify_codex_success_error_body(StatusCode::OK, &HeaderMap::new(), &success_body)
        .is_none());
    let bare_client_error =
        classify_codex_upstream_failure(StatusCode::NOT_FOUND, &HeaderMap::new(), success_body);
    assert_eq!(bare_client_error.class, CodexUpstreamErrorClass::UnexpectedStatus);
    let capacity_text = classify_codex_upstream_failure(
        StatusCode::BAD_GATEWAY,
        &HeaderMap::new(),
        captured_json_bytes(r#"{"error":{"message":"Capacity planning is disabled."}}"#),
    );
    assert_eq!(capacity_text.class, CodexUpstreamErrorClass::UnexpectedStatus);
    let mut retry_headers = HeaderMap::new();
    retry_headers.insert(header::RETRY_AFTER, "86400".parse().expect("retry-after"));
    let retryable = classify_codex_upstream_failure(
        StatusCode::TOO_MANY_REQUESTS,
        &retry_headers,
        captured_json_bytes(r#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#),
    );
    assert_eq!(retryable.retry_after, Some(CODEX_RETRY_AFTER_MAX));
    assert!(classify_codex_sse_event_failure(
        StatusCode::OK,
        &HeaderMap::new(),
        Some("response.completed"),
        r#"{"type":"response.completed","response":{"id":"resp_1","error":null}}"#,
    )
    .is_none());
    assert!(classify_codex_sse_event_failure(
        StatusCode::OK,
        &HeaderMap::new(),
        Some("response.incomplete"),
        r#"{"type":"response.incomplete","response":{"id":"resp_1","status":"incomplete","error":null}}"#,
    )
    .is_none());
    // Overload delivered in-stream as a `response.failed` event carrying only a
    // message (no `server_is_overloaded` code) still classifies as overload.
    let stream_overload = classify_codex_sse_event_failure(
        StatusCode::OK,
        &HeaderMap::new(),
        Some("response.failed"),
        r#"{"type":"response.failed","response":{"error":{"message":"Our servers are currently overloaded. Please try again later."}}}"#,
    )
    .expect("overload response.failed must classify as a failure");
    assert_eq!(stream_overload.class, CodexUpstreamErrorClass::ServerOverloaded);
}

#[test]
fn codex_surface_error_body_injects_overload_code() {
    use super::errors::codex_surface_error_body_with_code;

    // OpenAI-style endpoint: the injected `code` lets the Codex client classify
    // the failure as ServerOverloaded instead of showing the raw message.
    let body = codex_surface_error_body_with_code(
        "/v1/responses",
        StatusCode::SERVICE_UNAVAILABLE,
        "Our servers are currently overloaded. Please try again later.",
        Some("server_is_overloaded"),
    );
    let value: serde_json::Value =
        serde_json::from_str(&body).expect("surface body must be valid json");
    assert_eq!(value["error"]["code"], "server_is_overloaded");

    // No code passed -> `code` stays null (unchanged legacy behavior).
    let plain =
        codex_surface_error_body_with_code("/v1/responses", StatusCode::BAD_GATEWAY, "boom", None);
    let plain_value: serde_json::Value =
        serde_json::from_str(&plain).expect("surface body must be valid json");
    assert!(plain_value["error"]["code"].is_null());
}

#[test]
fn completed_codex_sse_accepts_response_incomplete_terminal_event() {
    let stream = format!(
        "event: response.output_text.done\ndata: {}\n\nevent: response.incomplete\ndata: {}\n\n",
        json!({
            "type": "response.output_text.done",
            "item_id": "msg_1",
            "text": "partial answer"
        }),
        json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_1",
                "status": "incomplete",
                "output": [],
                "usage": {
                    "input_tokens": 4,
                    "input_tokens_details": {
                        "cached_tokens": 1
                    },
                    "output_tokens": 2
                }
            }
        })
    );

    let completed = match super::codex_sse::completed_response_from_sse_bytes(stream.as_bytes()) {
        Ok(completed) => completed,
        Err(err) => panic!("response.incomplete should be a terminal response: {}", err.message),
    };
    assert_eq!(completed.response["id"], "resp_1");
    assert_eq!(completed.response["output"][0]["content"][0]["text"], "partial answer");
}

#[tokio::test]
async fn usage_metadata_resolves_client_ip_region() {
    let resolver = crate::geoip::GeoIpResolver::fixed_for_tests("Singapore/Singapore");
    let headers = HeaderMap::from_iter([(
        "x-forwarded-for".parse().expect("header name"),
        "208.77.246.15".parse().expect("header value"),
    )]);
    let uri = "/api/kiro-gateway/v1/messages".parse().expect("uri");

    let metadata = super::ProviderUsageMetadata::from_request_parts(
        &super::Method::POST,
        &uri,
        &headers,
        &resolver,
    )
    .await;

    assert_eq!(metadata.client_ip, "208.77.246.15");
    assert_eq!(metadata.ip_region, "Singapore/Singapore");
}

fn test_state() -> super::ProviderState {
    super::ProviderState::new(Arc::new(TestStore), empty_route_store())
}

fn test_state_with_dispatcher(dispatcher: Arc<dyn ProviderDispatcher>) -> super::ProviderState {
    super::ProviderState::with_dispatcher(Arc::new(TestStore), empty_route_store(), dispatcher)
}

fn request_with_x_api_key_to_path(path: &str, secret: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(path);
    if let Some(secret) = secret {
        builder = builder.header("x-api-key", secret);
    }
    builder.body(Body::empty()).expect("request")
}

async fn fake_codex_responses(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    fake_codex_responses_with_content_type(captured, headers, request, "text/event-stream").await
}

async fn fake_codex_responses_json_content_type(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    fake_codex_responses_with_content_type(captured, headers, request, "application/json").await
}

async fn fake_codex_responses_stream_without_completed(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    let call_index = {
        let mut requests = captured.requests.lock().expect("captured requests");
        requests.push(CapturedCodexRequest {
            path,
            query,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body,
        });
        requests.len() - 1
    };

    let completed_output = json!([{
        "type": "message",
        "role": "assistant",
        "content": [{
            "type": "output_text",
            "text": "stream back"
        }]
    }]);
    captured
        .completed_outputs
        .lock()
        .expect("captured completed outputs")
        .push(completed_output.clone());

    if call_index > 0 {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!(
                "event: response.completed\ndata: {}\n\n",
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_1",
                        "created_at": 123,
                        "model": "gpt-5.3-codex-spark",
                        "output": completed_output,
                        "usage": {
                            "input_tokens": 12,
                            "input_tokens_details": {
                                "cached_tokens": 2
                            },
                            "output_tokens": 3
                        }
                    }
                })
            )))
            .expect("completed upstream response");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.output_item.done\ndata: {}\n\ndata: [DONE]\n\n",
            json!({
                "type": "response.output_item.done",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "output_index": 0,
                "item": completed_output[0]
            })
        )))
        .expect("stream-without-completed upstream response")
}

async fn fake_codex_responses_with_content_type(
    captured: Arc<CapturedCodexUpstream>,
    headers: HeaderMap,
    request: Request<Body>,
    content_type: &'static str,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body,
        });

    let completed_output = json!([{
        "type": "message",
        "content": [{
            "type": "output_text",
            "text": "hello back"
        }]
    }]);
    captured
        .completed_outputs
        .lock()
        .expect("captured completed outputs")
        .push(completed_output.clone());

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(format!(
            "event: response.output_text.delta\ndata: {}\n\nevent: \
             response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "delta": "hello "
            }),
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "delta": "back"
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "created_at": 123,
                    "model": "gpt-5.3-codex-spark",
                    "output": completed_output,
                    "usage": {
                        "input_tokens": 12,
                        "input_tokens_details": {
                            "cached_tokens": 2
                        },
                        "output_tokens": 3
                    }
                }
            })
        )))
        .expect("upstream response")
}

async fn fake_codex_responses_custom_tool_stream(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body,
        });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.output_item.added\ndata: {}\n\nevent: \
             response.custom_tool_call_input.delta\ndata: {}\n\nevent: \
             response.output_item.done\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
            json!({
                "type": "response.output_item.added",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "item": {
                    "type": "custom_tool_call",
                    "call_id": "callpatch1",
                    "name": "apply_patch",
                    "input": ""
                }
            }),
            json!({
                "type": "response.custom_tool_call_input.delta",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "call_id": "callpatch1",
                "delta": "*** Begin Patch"
            }),
            json!({
                "type": "response.output_item.done",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "item": {
                    "type": "custom_tool_call",
                    "call_id": "callpatch1",
                    "name": "apply_patch",
                    "input": "*** Begin Patch"
                }
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "created_at": 123,
                    "model": "gpt-5.3-codex-spark",
                    "output": [{
                        "type": "custom_tool_call",
                        "call_id": "callpatch1",
                        "name": "apply_patch",
                        "input": "*** Begin Patch"
                    }],
                    "usage": {
                        "input_tokens": 12,
                        "input_tokens_details": {
                            "cached_tokens": 2
                        },
                        "output_tokens": 7
                    }
                }
            })
        )))
        .expect("custom tool upstream response")
}

async fn fake_codex_responses_json_success(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body,
        });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "id": "rs_compact_1",
                "created_at": 123,
                "model": "gpt-5.3-codex-spark",
                "output": [{
                    "id": "item_compact_1",
                    "type": "message",
                    "content": [{
                        "type": "output_text",
                        "text": "hello compact back"
                    }]
                }],
                "usage": {
                    "input_tokens": 12,
                    "input_tokens_details": {
                        "cached_tokens": 2
                    },
                    "output_tokens": 3
                }
            })
            .to_string(),
        ))
        .expect("upstream json response")
}


async fn fake_codex_responses_empty_completed_output(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body: if body.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
            },
        });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.output_text.delta\ndata: {}\n\nevent: \
             response.output_text.done\ndata: {}\n\nevent: response.output_item.done\ndata: \
             {}\n\nevent: response.completed\ndata: {}\n\n",
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "item_id": "msg_1",
                "output_index": 0,
                "content_index": 0,
                "delta": "hello back"
            }),
            json!({
                "type": "response.output_text.done",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "item_id": "msg_1",
                "output_index": 0,
                "content_index": 0,
                "text": "hello back"
            }),
            json!({
                "type": "response.output_item.done",
                "output_index": 0,
                "item": {
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "hello back"
                    }]
                }
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "created_at": 123,
                    "model": "gpt-5.3-codex-spark",
                    "output": [],
                    "usage": {
                        "input_tokens": 12,
                        "input_tokens_details": {
                            "cached_tokens": 2
                        },
                        "output_tokens": 3
                    }
                }
            })
        )))
        .expect("upstream response")
}

async fn fake_codex_responses_quota_then_success(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: authorization.clone(),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body,
        });

    if authorization.as_deref() == Some("Bearer upstream-token-a") {
        return Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"error":{"code":"insufficient_quota","message":"You've hit your usage limit. Try again later."}}"#,
                ))
                .expect("quota upstream response");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.output_text.delta\ndata: {}\n\nevent: \
             response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "delta": "hello "
            }),
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "delta": "back"
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "created_at": 123,
                    "model": "gpt-5.3-codex-spark",
                    "output": [{
                        "type": "message",
                        "content": [{
                            "type": "output_text",
                            "text": "hello back"
                        }]
                    }],
                    "usage": {
                        "input_tokens": 12,
                        "input_tokens_details": {
                            "cached_tokens": 2
                        },
                        "output_tokens": 3
                    }
                }
            })
        )))
        .expect("upstream response")
}

async fn fake_codex_responses_fail_first_three(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: authorization.clone(),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body,
        });

    match authorization.as_deref() {
        Some("Bearer upstream-token-a")
        | Some("Bearer upstream-token-b")
        | Some("Bearer upstream-token-c") => Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"error":{"message":"temporary upstream failure"}}"#))
            .expect("failing upstream response"),
        _ => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!(
                "event: response.output_text.delta\ndata: {}\n\nevent: \
                 response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
                json!({
                    "type": "response.output_text.delta",
                    "response_id": "resp_1",
                    "created": 123,
                    "model": "gpt-5.3-codex-spark",
                    "delta": "hello "
                }),
                json!({
                    "type": "response.output_text.delta",
                    "response_id": "resp_1",
                    "created": 123,
                    "model": "gpt-5.3-codex-spark",
                    "delta": "back"
                }),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_1",
                        "created_at": 123,
                        "model": "gpt-5.3-codex-spark",
                        "output": [{
                            "type": "message",
                            "content": [{
                                "type": "output_text",
                                "text": "hello back"
                            }]
                        }],
                        "usage": {
                            "input_tokens": 12,
                            "input_tokens_details": {
                                "cached_tokens": 2
                            },
                            "output_tokens": 3
                        }
                    }
                })
            )))
            .expect("upstream response"),
    }
}

async fn fake_codex_responses_rate_limit_once_then_success(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    let call_index = {
        let mut requests = captured.requests.lock().expect("captured requests");
        requests.push(captured_codex_request(&headers, path, query, body));
        requests.len()
    };

    if call_index == 1 {
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"error":{"code":"rate_limit_exceeded","message":"Rate limit exceeded."}}"#,
            ))
            .expect("rate-limited upstream response");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: \
             {}\n\n",
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "delta": "retry recovered"
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "created_at": 123,
                    "model": "gpt-5.3-codex-spark",
                    "output": [{
                        "type": "message",
                        "content": [{
                            "type": "output_text",
                            "text": "retry recovered"
                        }]
                    }],
                    "usage": {
                        "input_tokens": 12,
                        "input_tokens_details": {
                            "cached_tokens": 2
                        },
                        "output_tokens": 3
                    }
                }
            })
        )))
        .expect("retry recovered upstream response")
}

async fn fake_codex_responses_context_window_exceeded(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(captured_codex_request(&headers, path, query, body));

    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"error":{"code":"context_length_exceeded","type":"invalid_request_error","message":"Context window exceeded."}}"#,
        ))
        .expect("context window upstream response")
}

async fn fake_codex_responses_cyber_policy(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(captured_codex_request(&headers, path, query, body));

    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"error":{"code":"cyber_policy","type":"invalid_request_error","message":"Request blocked by policy."}}"#,
        ))
        .expect("cyber policy upstream response")
}

fn codex_request_has_encrypted_reasoning_input(body: &serde_json::Value) -> bool {
    fn item_has_encrypted_reasoning(item: &serde_json::Value) -> bool {
        item.get("type").and_then(serde_json::Value::as_str) == Some("reasoning")
            && item.get("encrypted_content").is_some()
    }

    match body.get("input") {
        Some(serde_json::Value::Array(items)) => items.iter().any(item_has_encrypted_reasoning),
        Some(item) => item_has_encrypted_reasoning(item),
        None => false,
    }
}

fn captured_codex_request(
    headers: &HeaderMap,
    path: String,
    query: Option<String>,
    body: serde_json::Value,
) -> CapturedCodexRequest {
    CapturedCodexRequest {
        path,
        query,
        authorization: headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string),
        accept: headers
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string),
        user_agent: headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string),
        conversation_id: headers
            .get("conversation_id")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string),
        x_client_request_id: headers
            .get("x-client-request-id")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string),
        session_id: headers
            .get("session_id")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string),
        x_codex_turn_state: headers
            .get("x-codex-turn-state")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string),
        body,
    }
}

async fn fake_codex_responses_invalid_encrypted_until_trimmed(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(captured_codex_request(&headers, path, query, body.clone()));

    if codex_request_has_encrypted_reasoning_input(&body) {
        return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"error":{"code":"invalid_encrypted_content","type":"invalid_request_error","message":"The encrypted content could not be verified."}}"#,
                ))
                .expect("invalid encrypted content response");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: \
             {}\n\n",
            json!({"delta":"recovered"}),
            json!({
                "response": {
                    "id": "rs_recovered",
                    "created_at": 123,
                    "model": "gpt-5.3-codex",
                    "output": [{
                        "type": "message",
                        "content": [{
                            "type": "output_text",
                            "text": "recovered"
                        }]
                    }],
                    "usage": {
                        "input_tokens": 12,
                        "input_tokens_details": {
                            "cached_tokens": 2
                        },
                        "output_tokens": 3
                    }
                }
            })
        )))
        .expect("recovered upstream response")
}

async fn fake_codex_responses_always_invalid_encrypted_content(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(captured_codex_request(&headers, path, query, body));

    Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"error":{"code":"invalid_encrypted_content","type":"invalid_request_error","message":"The encrypted content could not be verified."}}"#,
            ))
            .expect("invalid encrypted content response")
}

async fn fake_codex_responses_invalid_value(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(captured_codex_request(&headers, path, query, body));

    Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"error":{"code":"invalid_value","type":"invalid_request_error","message":"Invalid value: 'tool'. Supported values are: 'assistant', 'system', 'developer', and 'user'."}}"#,
            ))
            .expect("invalid value response")
}

async fn fake_codex_responses_always_unauthorized(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body,
        });

    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"error":{"code":"invalid_api_key","message":"access token rejected"}}"#,
        ))
        .expect("unauthorized upstream response")
}

async fn fake_codex_responses_always_bad_gateway(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body,
        });

    Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"error":{"message":"temporary upstream failure","type":"api_error","param":"unused","code":"bad_gateway"}}"#,
            ))
            .expect("bad gateway upstream response")
}

async fn fake_codex_responses_failed_sse(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body,
        });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.failed\ndata: {}\n\n",
            json!({
                "type": "response.failed",
                "response": {
                    "status": "failed",
                    "error": {
                        "type": "invalid_request_error",
                        "message": "tool_choice references a missing tool",
                        "code": "invalid_tool_choice"
                    }
                }
            })
        )))
        .expect("failed sse upstream response")
}

/// Always answers HTTP 200 with a single `response.failed` SSE event carrying a
/// recoverable `server_is_overloaded` error, recording every request so a test
/// can assert which accounts were attempted.
async fn fake_codex_responses_overload_sse(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(captured_codex_request(&headers, path, query, body));

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.failed\ndata: {}\n\n",
            json!({
                "type": "response.failed",
                "response": {
                    "status": "failed",
                    "error": {
                        "type": "server_error",
                        "message": "Our servers are currently overloaded. Please try again later.",
                        "code": "server_is_overloaded"
                    }
                }
            })
        )))
        .expect("overload sse upstream response")
}

async fn fake_codex_responses_chat_lifecycle_then_overload_sse(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(captured_codex_request(&headers, path, query, body));

    if authorization.as_deref() == Some("Bearer upstream-token-a") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!(
                "event: response.created\ndata: {}\n\nevent: response.failed\ndata: {}\n\n",
                json!({
                    "type": "response.created",
                    "response": {
                        "id": "resp_overload",
                        "status": "in_progress",
                        "model": "gpt-5.3-codex-spark",
                        "output": []
                    }
                }),
                json!({
                    "type": "response.failed",
                    "response": {
                        "status": "failed",
                        "error": {
                            "type": "service_unavailable_error",
                            "message": "Our servers are currently overloaded. Please try again later.",
                            "code": "server_is_overloaded"
                        }
                    }
                })
            )))
            .expect("lifecycle then overload upstream response");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: \
             {}\n\n",
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "delta": "hello back"
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "created_at": 123,
                    "model": "gpt-5.3-codex-spark",
                    "output": [{
                        "type": "message",
                        "content": [{
                            "type": "output_text",
                            "text": "hello back"
                        }]
                    }],
                    "usage": {
                        "input_tokens": 12,
                        "input_tokens_details": {
                            "cached_tokens": 2
                        },
                        "output_tokens": 3
                    }
                }
            })
        )))
        .expect("success upstream response")
}

async fn fake_codex_responses_long_lifecycle_then_overload_sse(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(captured_codex_request(&headers, path, query, body));

    if authorization.as_deref() == Some("Bearer upstream-token-a") {
        let mut body = String::new();
        for index in 0..32 {
            body.push_str(&format!(
                "event: response.created\ndata: {}\n\n",
                json!({
                    "type": "response.created",
                    "response": {
                        "id": format!("resp_lifecycle_{index}"),
                        "status": "in_progress",
                        "model": "gpt-5.3-codex-spark",
                        "output": []
                    }
                })
            ));
        }
        body.push_str(&format!(
            "event: response.failed\ndata: {}\n\n",
            json!({
                "type": "response.failed",
                "response": {
                    "status": "failed",
                    "error": {
                        "type": "service_unavailable_error",
                        "message": "Our servers are currently overloaded. Please try again later.",
                        "code": "server_is_overloaded"
                    }
                }
            })
        ));
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .expect("long lifecycle then overload upstream response");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: \
             {}\n\n",
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "delta": "hello back"
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "created_at": 123,
                    "model": "gpt-5.3-codex-spark",
                    "output": [{
                        "type": "message",
                        "content": [{
                            "type": "output_text",
                            "text": "hello back"
                        }]
                    }],
                    "usage": {
                        "input_tokens": 12,
                        "input_tokens_details": {
                            "cached_tokens": 2
                        },
                        "output_tokens": 3
                    }
                }
            })
        )))
        .expect("success upstream response")
}

async fn fake_codex_responses_mid_stream_failed_sse(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
    };
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body,
        });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(format!(
            "event: response.output_text.delta\ndata: {}\n\nevent: response.failed\ndata: {}\n\n",
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_1",
                "created": 123,
                "model": "gpt-5.3-codex-spark",
                "delta": "hello "
            }),
            json!({
                "type": "response.failed",
                "response": {
                    "status": "failed",
                    "error": {
                        "type": "invalid_request_error",
                        "message": "tool_choice references a missing tool",
                        "code": "invalid_tool_choice"
                    }
                }
            })
        )))
        .expect("mid-stream failed sse upstream response")
}

async fn fake_codex_models(
    State(captured): State<Arc<CapturedCodexUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let query = request.uri().query().map(ToString::to_string);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedCodexRequest {
            path,
            query,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            accept: headers
                .get(header::ACCEPT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            conversation_id: headers
                .get("conversation_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_client_request_id: headers
                .get("x-client-request-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            session_id: headers
                .get("session_id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            x_codex_turn_state: headers
                .get("x-codex-turn-state")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            body: if body.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
            },
        });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ETAG, r#""models-test""#)
        .body(Body::from(
            json!({
                "models": [
                    {"slug": "gpt-5.3-codex-spark"},
                    {"slug": "gpt-5.5"}
                ]
            })
            .to_string(),
        ))
        .expect("upstream response")
}

async fn fake_kiro_generate(
    State(captured): State<Arc<CapturedKiroUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedKiroRequest {
            path,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: super::header_value(&headers, header::USER_AGENT.as_str()),
            x_amz_user_agent: super::header_value(&headers, "x-amz-user-agent"),
            host: super::header_value(&headers, "host"),
            token_type: super::header_value(&headers, "TokenType"),
            redirect_for_internal: super::header_value(&headers, "redirect-for-internal"),
            agent_mode: super::header_value(&headers, "x-amzn-kiro-agent-mode"),
            opt_out: super::header_value(&headers, "x-amzn-codewhisperer-optout"),
            body,
        });
    let body = kiro_eventstream_body(vec![
        kiro_event_frame("assistantResponseEvent", &json!({"content":"hello "})),
        kiro_event_frame("assistantResponseEvent", &json!({"content":"back"})),
        kiro_event_frame("contextUsageEvent", &json!({"contextUsagePercentage":0.01})),
        kiro_event_frame("meteringEvent", &json!({"unit":"credit","usage":0.25})),
    ]);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
        .body(Body::from(body))
        .expect("upstream response")
}

async fn fake_kiro_generate_empty_once(
    State(captured): State<Arc<CapturedKiroUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
    let attempt = {
        let mut requests = captured.requests.lock().expect("captured requests");
        requests.push(CapturedKiroRequest {
            path,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: super::header_value(&headers, header::USER_AGENT.as_str()),
            x_amz_user_agent: super::header_value(&headers, "x-amz-user-agent"),
            host: super::header_value(&headers, "host"),
            token_type: super::header_value(&headers, "TokenType"),
            redirect_for_internal: super::header_value(&headers, "redirect-for-internal"),
            agent_mode: super::header_value(&headers, "x-amzn-kiro-agent-mode"),
            opt_out: super::header_value(&headers, "x-amzn-codewhisperer-optout"),
            body,
        });
        requests.len()
    };
    if attempt == 1 {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
            .body(Body::empty())
            .expect("empty upstream response");
    }
    let body = kiro_eventstream_body(vec![
        kiro_event_frame("assistantResponseEvent", &json!({"content":"hello "})),
        kiro_event_frame("assistantResponseEvent", &json!({"content":"back"})),
        kiro_event_frame("contextUsageEvent", &json!({"contextUsagePercentage":0.01})),
        kiro_event_frame("meteringEvent", &json!({"unit":"credit","usage":0.25})),
    ]);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
        .body(Body::from(body))
        .expect("upstream response")
}

async fn fake_kiro_generate_empty_route_then_success(
    State(captured): State<Arc<CapturedKiroUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedKiroRequest {
            path,
            authorization: authorization.clone(),
            user_agent: super::header_value(&headers, header::USER_AGENT.as_str()),
            x_amz_user_agent: super::header_value(&headers, "x-amz-user-agent"),
            host: super::header_value(&headers, "host"),
            token_type: super::header_value(&headers, "TokenType"),
            redirect_for_internal: super::header_value(&headers, "redirect-for-internal"),
            agent_mode: super::header_value(&headers, "x-amzn-kiro-agent-mode"),
            opt_out: super::header_value(&headers, "x-amzn-codewhisperer-optout"),
            body,
        });
    match authorization.as_deref() {
        Some("Bearer kiro-empty-token") => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
            .body(Body::empty())
            .expect("empty upstream response"),
        Some("Bearer kiro-success-token") => {
            let body = kiro_eventstream_body(vec![
                kiro_event_frame("assistantResponseEvent", &json!({"content":"hello "})),
                kiro_event_frame("assistantResponseEvent", &json!({"content":"back"})),
                kiro_event_frame("contextUsageEvent", &json!({"contextUsagePercentage":0.01})),
                kiro_event_frame("meteringEvent", &json!({"unit":"credit","usage":0.25})),
            ]);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
                .body(Body::from(body))
                .expect("upstream response")
        },
        _ => Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"error":{"message":"unexpected upstream authorization"}}"#))
            .expect("unexpected upstream response"),
    }
}

async fn fake_kiro_generate_rate_limit_route_then_success(
    State(captured): State<Arc<CapturedKiroUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedKiroRequest {
            path,
            authorization: authorization.clone(),
            user_agent: super::header_value(&headers, header::USER_AGENT.as_str()),
            x_amz_user_agent: super::header_value(&headers, "x-amz-user-agent"),
            host: super::header_value(&headers, "host"),
            token_type: super::header_value(&headers, "TokenType"),
            redirect_for_internal: super::header_value(&headers, "redirect-for-internal"),
            agent_mode: super::header_value(&headers, "x-amzn-kiro-agent-mode"),
            opt_out: super::header_value(&headers, "x-amzn-codewhisperer-optout"),
            body,
        });
    match authorization.as_deref() {
        Some("Bearer kiro-rate-limited-token") => Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::RETRY_AFTER, "30")
            .body(Body::from(r#"{"detail":"Too many requests, please wait before trying again."}"#))
            .expect("rate limited upstream response"),
        Some("Bearer kiro-success-token") => {
            let body = kiro_eventstream_body(vec![
                kiro_event_frame("assistantResponseEvent", &json!({"content":"hello "})),
                kiro_event_frame("assistantResponseEvent", &json!({"content":"back"})),
                kiro_event_frame("contextUsageEvent", &json!({"contextUsagePercentage":0.01})),
                kiro_event_frame("meteringEvent", &json!({"unit":"credit","usage":0.25})),
            ]);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
                .body(Body::from(body))
                .expect("successful upstream response")
        },
        _ => Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"error":{"message":"unexpected upstream authorization"}}"#))
            .expect("unexpected upstream response"),
    }
}

async fn fake_kiro_generate_reasoning(
    State(captured): State<Arc<CapturedKiroUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedKiroRequest {
            path,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: super::header_value(&headers, header::USER_AGENT.as_str()),
            x_amz_user_agent: super::header_value(&headers, "x-amz-user-agent"),
            host: super::header_value(&headers, "host"),
            token_type: super::header_value(&headers, "TokenType"),
            redirect_for_internal: super::header_value(&headers, "redirect-for-internal"),
            agent_mode: super::header_value(&headers, "x-amzn-kiro-agent-mode"),
            opt_out: super::header_value(&headers, "x-amzn-codewhisperer-optout"),
            body,
        });
    let body = kiro_eventstream_body(vec![
        kiro_event_frame("reasoningContentEvent", &json!({"text":"先想一步"})),
        kiro_event_frame("reasoningContentEvent", &json!({"signature":"upstream-signature-47"})),
        kiro_event_frame("assistantResponseEvent", &json!({"content":"最终答案"})),
        kiro_event_frame("contextUsageEvent", &json!({"contextUsagePercentage":0.01})),
        kiro_event_frame("meteringEvent", &json!({"unit":"credit","usage":0.25})),
    ]);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
        .body(Body::from(body))
        .expect("upstream response")
}

async fn fake_kiro_generate_content_length_error(
    State(captured): State<Arc<CapturedKiroUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedKiroRequest {
            path,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: super::header_value(&headers, header::USER_AGENT.as_str()),
            x_amz_user_agent: super::header_value(&headers, "x-amz-user-agent"),
            host: super::header_value(&headers, "host"),
            token_type: super::header_value(&headers, "TokenType"),
            redirect_for_internal: super::header_value(&headers, "redirect-for-internal"),
            agent_mode: super::header_value(&headers, "x-amzn-kiro-agent-mode"),
            opt_out: super::header_value(&headers, "x-amzn-codewhisperer-optout"),
            body,
        });
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "reason": "CONTENT_LENGTH_EXCEEDS_THRESHOLD",
            "message": "Input is too long."
        })),
    )
        .into_response()
}

async fn fake_kiro_generate_content_length_exception_eventstream(
    State(captured): State<Arc<CapturedKiroUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedKiroRequest {
            path,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: super::header_value(&headers, header::USER_AGENT.as_str()),
            x_amz_user_agent: super::header_value(&headers, "x-amz-user-agent"),
            host: super::header_value(&headers, "host"),
            token_type: super::header_value(&headers, "TokenType"),
            redirect_for_internal: super::header_value(&headers, "redirect-for-internal"),
            agent_mode: super::header_value(&headers, "x-amzn-kiro-agent-mode"),
            opt_out: super::header_value(&headers, "x-amzn-codewhisperer-optout"),
            body,
        });
    let body = kiro_eventstream_body(vec![kiro_exception_frame(
        "ContentLengthExceededException",
        "Input content length exceeds threshold.",
    )]);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
        .body(Body::from(body))
        .expect("upstream response")
}

async fn fake_kiro_generate_split_content_length_exception_eventstream(
    State(captured): State<Arc<CapturedKiroUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedKiroRequest {
            path,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: super::header_value(&headers, header::USER_AGENT.as_str()),
            x_amz_user_agent: super::header_value(&headers, "x-amz-user-agent"),
            host: super::header_value(&headers, "host"),
            token_type: super::header_value(&headers, "TokenType"),
            redirect_for_internal: super::header_value(&headers, "redirect-for-internal"),
            agent_mode: super::header_value(&headers, "x-amzn-kiro-agent-mode"),
            opt_out: super::header_value(&headers, "x-amzn-codewhisperer-optout"),
            body,
        });
    let body = kiro_eventstream_body(vec![kiro_exception_frame(
        "ContentLengthExceededException",
        "Input content length exceeds threshold.",
    )]);
    let split_at = body.len() / 2;
    let chunks = vec![
        Ok::<_, std::io::Error>(super::Bytes::copy_from_slice(&body[..split_at])),
        Ok(super::Bytes::copy_from_slice(&body[split_at..])),
    ];
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .expect("upstream response")
}

async fn fake_kiro_usage_limits(
    State(captured): State<Arc<CapturedKiroUpstream>>,
    headers: HeaderMap,
) -> Response {
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedKiroRequest {
            path: "/getUsageLimits".to_string(),
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: super::header_value(&headers, header::USER_AGENT.as_str()),
            x_amz_user_agent: super::header_value(&headers, "x-amz-user-agent"),
            host: super::header_value(&headers, "host"),
            token_type: super::header_value(&headers, "TokenType"),
            redirect_for_internal: super::header_value(&headers, "redirect-for-internal"),
            agent_mode: super::header_value(&headers, "x-amzn-kiro-agent-mode"),
            opt_out: super::header_value(&headers, "x-amzn-codewhisperer-optout"),
            body: serde_json::Value::Null,
        });
    Json(json!({
        "subscriptionInfo": {"subscriptionTitle": "Pro"},
        "usageBreakdownList": [{
            "currentUsageWithPrecision": 10.0,
            "usageLimitWithPrecision": 100.0,
            "bonuses": [],
            "nextDateReset": 900.0
        }],
        "userInfo": {"userId": "upstream-user-1"}
    }))
    .into_response()
}

async fn fake_kiro_mcp(
    State(captured): State<Arc<CapturedKiroUpstream>>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let path = request.uri().path().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("upstream request body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
    captured
        .requests
        .lock()
        .expect("captured requests")
        .push(CapturedKiroRequest {
            path,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            user_agent: super::header_value(&headers, header::USER_AGENT.as_str()),
            x_amz_user_agent: super::header_value(&headers, "x-amz-user-agent"),
            host: super::header_value(&headers, "host"),
            token_type: super::header_value(&headers, "TokenType"),
            redirect_for_internal: super::header_value(&headers, "redirect-for-internal"),
            agent_mode: super::header_value(&headers, "x-amzn-kiro-agent-mode"),
            opt_out: super::header_value(&headers, "x-amzn-codewhisperer-optout"),
            body,
        });
    Json(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "content": [{
                "type": "text",
                "text": "{\"results\":[]}"
            }],
            "isError": false
        }
    }))
    .into_response()
}

fn kiro_eventstream_body(frames: Vec<Vec<u8>>) -> Vec<u8> {
    frames.into_iter().flatten().collect()
}

fn kiro_event_frame(event_type: &str, payload: &serde_json::Value) -> Vec<u8> {
    let payload = serde_json::to_vec(payload).expect("payload json");
    let mut headers = Vec::new();
    push_aws_string_header(&mut headers, ":message-type", "event");
    push_aws_string_header(&mut headers, ":event-type", event_type);
    let total_length = 12 + headers.len() + payload.len() + 4;
    let mut frame = Vec::with_capacity(total_length);
    frame.extend_from_slice(&(total_length as u32).to_be_bytes());
    frame.extend_from_slice(&(headers.len() as u32).to_be_bytes());
    let prelude_crc = llm_access_kiro::parser::crc::crc32(&frame);
    frame.extend_from_slice(&prelude_crc.to_be_bytes());
    frame.extend_from_slice(&headers);
    frame.extend_from_slice(&payload);
    let message_crc = llm_access_kiro::parser::crc::crc32(&frame);
    frame.extend_from_slice(&message_crc.to_be_bytes());
    frame
}

fn kiro_exception_frame(exception_type: &str, payload: &str) -> Vec<u8> {
    let payload = payload.as_bytes();
    let mut headers = Vec::new();
    push_aws_string_header(&mut headers, ":message-type", "exception");
    push_aws_string_header(&mut headers, ":exception-type", exception_type);
    let total_length = 12 + headers.len() + payload.len() + 4;
    let mut frame = Vec::with_capacity(total_length);
    frame.extend_from_slice(&(total_length as u32).to_be_bytes());
    frame.extend_from_slice(&(headers.len() as u32).to_be_bytes());
    let prelude_crc = llm_access_kiro::parser::crc::crc32(&frame);
    frame.extend_from_slice(&prelude_crc.to_be_bytes());
    frame.extend_from_slice(&headers);
    frame.extend_from_slice(payload);
    let message_crc = llm_access_kiro::parser::crc::crc32(&frame);
    frame.extend_from_slice(&message_crc.to_be_bytes());
    frame
}

fn push_aws_string_header(headers: &mut Vec<u8>, name: &str, value: &str) {
    headers.push(name.len() as u8);
    headers.extend_from_slice(name.as_bytes());
    headers.push(7);
    headers.extend_from_slice(&(value.len() as u16).to_be_bytes());
    headers.extend_from_slice(value.as_bytes());
}

async fn spawn_fake_kiro_upstream(captured: Arc<CapturedKiroUpstream>) -> String {
    let app = Router::new()
        .route("/generateAssistantResponse", post(fake_kiro_generate))
        .route("/mcp", post(fake_kiro_mcp))
        .route("/getUsageLimits", get(fake_kiro_usage_limits))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    upstream_base
}

async fn spawn_fake_kiro_empty_once_upstream(captured: Arc<CapturedKiroUpstream>) -> String {
    let app = Router::new()
        .route("/generateAssistantResponse", post(fake_kiro_generate_empty_once))
        .route("/mcp", post(fake_kiro_mcp))
        .route("/getUsageLimits", get(fake_kiro_usage_limits))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    upstream_base
}

async fn spawn_fake_kiro_empty_route_then_success_upstream(
    captured: Arc<CapturedKiroUpstream>,
) -> String {
    let app = Router::new()
        .route("/generateAssistantResponse", post(fake_kiro_generate_empty_route_then_success))
        .route("/mcp", post(fake_kiro_mcp))
        .route("/getUsageLimits", get(fake_kiro_usage_limits))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    upstream_base
}

async fn spawn_fake_kiro_rate_limit_route_then_success_upstream(
    captured: Arc<CapturedKiroUpstream>,
) -> String {
    let app = Router::new()
        .route("/generateAssistantResponse", post(fake_kiro_generate_rate_limit_route_then_success))
        .route("/mcp", post(fake_kiro_mcp))
        .route("/getUsageLimits", get(fake_kiro_usage_limits))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    upstream_base
}

async fn spawn_fake_kiro_reasoning_upstream(captured: Arc<CapturedKiroUpstream>) -> String {
    let app = Router::new()
        .route("/generateAssistantResponse", post(fake_kiro_generate_reasoning))
        .route("/mcp", post(fake_kiro_mcp))
        .route("/getUsageLimits", get(fake_kiro_usage_limits))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    upstream_base
}

async fn spawn_fake_kiro_content_length_error_upstream(
    captured: Arc<CapturedKiroUpstream>,
) -> String {
    let app = Router::new()
        .route("/generateAssistantResponse", post(fake_kiro_generate_content_length_error))
        .route("/mcp", post(fake_kiro_mcp))
        .route("/getUsageLimits", get(fake_kiro_usage_limits))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    upstream_base
}

async fn spawn_fake_kiro_content_length_exception_eventstream_upstream(
    captured: Arc<CapturedKiroUpstream>,
) -> String {
    let app = Router::new()
        .route(
            "/generateAssistantResponse",
            post(fake_kiro_generate_content_length_exception_eventstream),
        )
        .route("/mcp", post(fake_kiro_mcp))
        .route("/getUsageLimits", get(fake_kiro_usage_limits))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    upstream_base
}

async fn spawn_fake_kiro_split_content_length_exception_eventstream_upstream(
    captured: Arc<CapturedKiroUpstream>,
) -> String {
    let app = Router::new()
        .route(
            "/generateAssistantResponse",
            post(fake_kiro_generate_split_content_length_exception_eventstream),
        )
        .route("/mcp", post(fake_kiro_mcp))
        .route("/getUsageLimits", get(fake_kiro_usage_limits))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    upstream_base
}

#[tokio::test]
async fn provider_entry_rejects_missing_bearer_token() {
    let state = test_state();
    let response = super::provider_entry(state, request_with_bearer(None)).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn provider_entry_rejects_malformed_bearer_token() {
    let state = test_state();
    for value in ["valid-secret", "Basic valid-secret", "Bearer "] {
        let response = super::provider_entry(state.clone(), request_with_bearer(Some(value))).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn provider_entry_rejects_unknown_bearer_token() {
    let state = test_state();
    let response =
        super::provider_entry(state, request_with_bearer(Some("Bearer unknown-secret"))).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn provider_entry_accepts_x_api_key_on_kiro_routes() {
    let dispatcher = Arc::new(CapturingDispatcher::default());
    let state = test_state_with_dispatcher(dispatcher.clone());

    let response = super::provider_entry(
        state,
        request_with_x_api_key_to_path("/api/kiro-gateway/v1/messages", Some("valid-secret")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(dispatcher.seen.lock().expect("dispatcher state").as_slice(), &[(
        "key-1".to_string(),
        "/api/kiro-gateway/v1/messages".to_string()
    )]);
}

#[tokio::test]
async fn provider_entry_accepts_x_api_key_on_neutral_models_route() {
    let dispatcher = Arc::new(CapturingDispatcher::default());
    let state = test_state_with_dispatcher(dispatcher.clone());

    let response = super::provider_entry(
        state,
        request_with_x_api_key_to_path("/v1/models", Some("valid-secret")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(dispatcher.seen.lock().expect("dispatcher state").as_slice(), &[(
        "key-1".to_string(),
        "/v1/models".to_string()
    )]);
}

#[tokio::test]
async fn provider_entry_rejects_x_api_key_on_codex_routes() {
    let dispatcher = Arc::new(CapturingDispatcher::default());
    let state = test_state_with_dispatcher(dispatcher.clone());

    let response = super::provider_entry(
        state,
        request_with_x_api_key_to_path("/v1/responses", Some("codex-secret")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(dispatcher.seen.lock().expect("dispatcher state").is_empty());
}

#[tokio::test]
async fn provider_entry_rejects_non_active_key() {
    let state = test_state();
    let response =
        super::provider_entry(state, request_with_bearer(Some("Bearer paused-secret"))).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn provider_entry_reports_store_errors_as_server_errors() {
    let state = super::ProviderState::new(Arc::new(FailingStore), empty_route_store());
    let response =
        super::provider_entry(state, request_with_bearer(Some("Bearer valid-secret"))).await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn provider_entry_accepts_known_bearer_token_before_dispatch() {
    let state = test_state();
    let response =
        super::provider_entry(state, request_with_bearer(Some("Bearer valid-secret"))).await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn provider_entry_tracks_rpm_and_in_flight_for_authenticated_requests() {
    let dispatcher = Arc::new(BlockingDispatcher::default());
    let state = test_state_with_dispatcher(dispatcher.clone());
    let request =
        request_with_x_api_key_to_path("/api/kiro-gateway/v1/messages", Some("valid-secret"));
    let task_state = state.clone();

    let handle = tokio::spawn(async move { super::provider_entry(task_state, request).await });
    dispatcher.entered.notified().await;

    let total = state.request_activity.snapshot(None);
    let key = state.request_activity.snapshot(Some("key-1"));
    assert_eq!(total.rpm, 1);
    assert_eq!(total.in_flight, 1);
    assert_eq!(key.rpm, 1);
    assert_eq!(key.in_flight, 1);

    dispatcher.release.notify_one();
    let response = handle.await.expect("provider task");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(state.request_activity.snapshot(Some("key-1")).in_flight, 0);
    assert_eq!(state.request_activity.snapshot(Some("key-1")).rpm, 1);
}

#[tokio::test]
async fn provider_entry_handler_uses_axum_state() {
    let state = test_state();
    let response = super::provider_entry_handler(
        axum::extract::State(state),
        request_with_bearer(Some("Bearer valid-secret")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn codex_dispatch_adapts_non_streaming_chat_completion_through_responses_sse() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .route("/v1/responses/compact", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["model"], "gpt-5.3-codex");
    assert_eq!(body["choices"][0]["message"]["content"], "hello back");
    assert_eq!(body["usage"]["input_tokens"], 12);
    assert_eq!(body["usage"]["output_tokens"], 3);

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer upstream-token"));
    assert_eq!(requests[0].accept.as_deref(), Some("text/event-stream"));
    assert_eq!(requests[0].body["model"], "gpt-5.3-codex-spark");
    assert_eq!(requests[0].body["stream"], true);
}

#[tokio::test]
async fn codex_dispatch_reconstructs_non_streaming_output_from_sse_item_events() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_empty_completed_output))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["model"], "gpt-5.3-codex");
    assert_eq!(body["choices"][0]["message"]["content"], "hello back");
    assert_eq!(body["usage"]["output_tokens"], 3);
}

#[tokio::test]
async fn codex_dispatch_converts_native_non_streaming_responses_sse_to_json() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_json_content_type))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "input": "hello",
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["id"], "resp_1");
    assert_eq!(body["model"], "gpt-5.3-codex");
    assert_eq!(body["output"][0]["content"][0]["text"], "hello back");
    assert_eq!(body["usage"]["input_tokens"], 12);
    assert_eq!(body["usage"]["output_tokens"], 3);

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].accept.as_deref(), Some("text/event-stream"));
    assert_eq!(requests[0].body["stream"], serde_json::json!(true));
    assert_eq!(requests[0].body["input"][0]["type"], serde_json::json!("message"));

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].input_uncached_tokens, 10);
    assert_eq!(events[0].input_cached_tokens, 2);
    assert_eq!(events[0].output_tokens, 3);
}

#[tokio::test]
async fn codex_dispatch_preserves_client_thread_headers() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .route("/v1/responses/compact", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .header("conversation_id", "conversation-header")
            .header("session_id", "legacy-session")
            .header("x-client-request-id", "client-request")
            .header("x-codex-turn-state", "stale-turn-state")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "prompt_cache_key": "thread-anchor",
                        "input": "hello",
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].conversation_id.as_deref(), Some("conversation-header"));
    assert_eq!(requests[0].session_id.as_deref(), Some("legacy-session"));
    assert_eq!(requests[0].x_client_request_id.as_deref(), Some("client-request"));
    assert_eq!(requests[0].x_codex_turn_state.as_deref(), Some("stale-turn-state"));
    assert_eq!(requests[0].body["prompt_cache_key"].as_str(), Some("thread-anchor"));
}

#[tokio::test]
async fn codex_dispatch_reconstructs_thread_headers_from_metadata_and_prompt_cache_key() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, "Bearer codex-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    "x-codex-turn-metadata",
                    r#"{"session_id":"session-from-metadata","thread_id":"thread-anchor","turn_id":"turn-1"}"#,
                )
                .body(Body::from(
                    r#"{
                        "model": "gpt-5.3-codex",
                        "prompt_cache_key": "thread-anchor",
                        "input": "hello",
                        "stream": false
                    }"#,
                ))
                .expect("request"),
        )
        .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].session_id.as_deref(), Some("session-from-metadata"));
    assert_eq!(requests[0].x_client_request_id.as_deref(), Some("thread-anchor"));
    assert_eq!(requests[0].conversation_id.as_deref(), Some("thread-anchor"));
}

#[tokio::test]
async fn codex_dispatch_sends_derived_session_headers_when_client_has_no_session_anchor() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                    "model": "gpt-5.4",
                    "messages": [
                        {"role": "user", "content": "summarize"},
                        {"role": "assistant", "content": "summary"},
                        {"role": "user", "content": "continue"}
                    ],
                    "stream": false
                }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    let session_id = requests[0]
        .session_id
        .as_deref()
        .expect("derived session should be sent upstream");
    assert!(session_id.starts_with("codex-session-v1-"));
    assert_eq!(requests[0].conversation_id.as_deref(), Some(session_id));
    assert_eq!(requests[0].x_client_request_id.as_deref(), Some(session_id));
    assert_eq!(requests[0].body["prompt_cache_key"].as_str(), Some(session_id));
}

#[tokio::test]
async fn codex_dispatch_keeps_stable_prefix_affinity_for_divergent_history() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);

    let first = super::provider_entry(
        state.clone(),
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                    "model": "gpt-5.4",
                    "messages": [{"role": "user", "content": "summarize"}],
                    "stream": false
                }"#,
            ))
            .expect("request"),
    )
    .await;
    let matching_follow_up = super::provider_entry(
        state.clone(),
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                    "model": "gpt-5.4",
                    "messages": [
                        {"role": "user", "content": "summarize"},
                        {"role": "assistant", "content": "hello back"},
                        {"role": "user", "content": "continue"}
                    ],
                    "stream": false
                }"#,
            ))
            .expect("request"),
    )
    .await;
    let divergent_follow_up = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                    "model": "gpt-5.4",
                    "messages": [
                        {"role": "user", "content": "summarize"},
                        {"role": "assistant", "content": "different answer"},
                        {"role": "user", "content": "continue"}
                    ],
                    "stream": false
                }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(matching_follow_up.status(), StatusCode::OK);
    assert_eq!(divergent_follow_up.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 3);
    let first_session = requests[0]
        .session_id
        .as_deref()
        .expect("first request should have a derived session");
    assert_eq!(requests[1].session_id.as_deref(), Some(first_session));
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer upstream-token-a"));
    assert_eq!(requests[1].authorization.as_deref(), Some("Bearer upstream-token-a"));
    assert_eq!(requests[2].session_id.as_deref(), Some(first_session));
    assert_eq!(requests[2].authorization.as_deref(), Some("Bearer upstream-token-a"));
}

#[tokio::test]
async fn codex_dispatch_recovers_session_across_three_response_output_round_trips() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);

    let mut history = vec![json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": "summarize"}]
    })];
    let first = super::provider_entry(
        state.clone(),
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": history.clone(),
                    "stream": false
                }))
                .expect("first body"),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);

    let first_output = {
        let outputs = captured
            .completed_outputs
            .lock()
            .expect("captured completed outputs");
        outputs[0][0].clone()
    };
    history.push(first_output);
    history.push(json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": "continue"}]
    }));
    let second = super::provider_entry(
        state.clone(),
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": history.clone(),
                    "stream": false
                }))
                .expect("second body"),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);

    let second_output = {
        let outputs = captured
            .completed_outputs
            .lock()
            .expect("captured completed outputs");
        outputs[1][0].clone()
    };
    history.push(second_output);
    history.push(json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": "next"}]
    }));
    let third = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": history.clone(),
                    "stream": false
                }))
                .expect("third body"),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(third.status(), StatusCode::OK);

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 3);
    let first_session = requests[0]
        .session_id
        .as_deref()
        .expect("first request should have a derived session");
    assert_eq!(requests[1].session_id.as_deref(), Some(first_session));
    assert_eq!(requests[2].session_id.as_deref(), Some(first_session));
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer upstream-token-a"));
    assert_eq!(requests[1].authorization.as_deref(), Some("Bearer upstream-token-a"));
    assert_eq!(requests[2].authorization.as_deref(), Some("Bearer upstream-token-a"));
}

#[tokio::test]
async fn codex_dispatch_stream_without_completed_uses_output_item_for_recovery() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_stream_without_completed))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);
    let first_input = json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": "stream once"}]
    });

    let first = super::provider_entry(
        state.clone(),
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": [first_input.clone()],
                    "stream": true
                }))
                .expect("first body"),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let _ = to_bytes(first.into_body(), usize::MAX)
        .await
        .expect("consume first stream");

    let first_output = {
        let outputs = captured
            .completed_outputs
            .lock()
            .expect("captured completed outputs");
        outputs[0][0].clone()
    };
    let second = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "model": "gpt-5.4",
                    "input": [
                        first_input,
                        first_output,
                        {
                            "type": "message",
                            "role": "user",
                            "content": [{"type": "input_text", "text": "continue"}]
                        }
                    ],
                    "stream": false
                }))
                .expect("second body"),
            ))
            .expect("request"),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 2);
    let first_session = requests[0]
        .session_id
        .as_deref()
        .expect("first request should have a derived session");
    assert_eq!(requests[1].session_id.as_deref(), Some(first_session));
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer upstream-token-a"));
    assert_eq!(requests[1].authorization.as_deref(), Some("Bearer upstream-token-a"));
}

#[tokio::test]
async fn codex_dispatch_retries_invalid_encrypted_content_without_cross_account_failover() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_invalid_encrypted_until_trimmed))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "prompt_cache_key": "thread-anchor",
                        "input": [
                            {
                                "type": "reasoning",
                                "encrypted_content": "sealed"
                            },
                            {
                                "type": "message",
                                "role": "user",
                                "content": [{"type": "input_text", "text": "hello"}]
                            }
                        ],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests
            .iter()
            .map(|request| request.authorization.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("Bearer upstream-token-a"), Some("Bearer upstream-token-a")]
    );
    assert!(codex_request_has_encrypted_reasoning_input(&requests[0].body));
    assert!(!codex_request_has_encrypted_reasoning_input(&requests[1].body));
}

#[tokio::test]
async fn codex_dispatch_does_not_failover_unrecoverable_invalid_encrypted_content() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_always_invalid_encrypted_content))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "prompt_cache_key": "thread-anchor",
                        "input": "hello",
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer upstream-token-a"));
}

#[tokio::test]
async fn codex_dispatch_does_not_failover_invalid_value_client_error() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_invalid_value))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "input": "hello",
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer upstream-token-a"));
}

#[tokio::test]
async fn codex_dispatch_retries_retryable_error_on_same_account_before_failover() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_rate_limit_once_then_success))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec![
        "Bearer upstream-token-a".to_string(),
        "Bearer upstream-token-a".to_string(),
    ]);
}

#[tokio::test]
async fn codex_dispatch_fails_over_recoverable_overload_in_sse_stream() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_overload_sse))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    let status = response.status();
    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    // A recoverable overload (server_is_overloaded) must be retried on the other
    // account before the client ever sees it. If only account A is attempted, the
    // in-stream/SSE path surfaced the error without failing over.
    assert!(
        auths.contains(&"Bearer upstream-token-b".to_string()),
        "recoverable overload should fail over to the second account; client status={status}, \
         attempted accounts={auths:?}"
    );
}

#[tokio::test]
async fn codex_dispatch_fails_over_chat_stream_error_before_first_client_chunk() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_chat_lifecycle_then_overload_sse))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("hello back"), "body: {body}");
    assert!(body.contains("[DONE]"), "body: {body}");
    assert!(!body.contains("server_is_overloaded"), "body: {body}");

    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec![
        "Bearer upstream-token-a".to_string(),
        "Bearer upstream-token-b".to_string(),
    ]);
    drop(requests);

    wait_for_usage_event_count(store.as_ref(), 1).await;
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 200);
    assert_eq!(events[0].quota_failover_count, 1);
    assert_eq!(events[0].account_name.as_deref(), Some("codex-b"));
}

#[tokio::test]
async fn codex_dispatch_fails_over_anthropic_stream_error_before_first_client_chunk() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_chat_lifecycle_then_overload_sse))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/llm-gateway/v1/messages")
            .header("x-api-key", "codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("event: content_block_delta"), "body: {body}");
    assert!(body.contains("hello back"), "body: {body}");
    assert!(!body.contains("server_is_overloaded"), "body: {body}");

    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec![
        "Bearer upstream-token-a".to_string(),
        "Bearer upstream-token-b".to_string(),
    ]);
    drop(requests);

    wait_for_usage_event_count(store.as_ref(), 1).await;
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 200);
    assert_eq!(events[0].quota_failover_count, 1);
    assert_eq!(events[0].account_name.as_deref(), Some("codex-b"));
}

#[tokio::test]
async fn codex_dispatch_stops_preflight_buffering_after_lifecycle_event_cap() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_long_lifecycle_then_overload_sse))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("server_is_overloaded"), "body: {body}");
    assert!(body.contains("[DONE]"), "body: {body}");

    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec!["Bearer upstream-token-a".to_string()]);
    drop(requests);

    wait_for_usage_event_count(store.as_ref(), 1).await;
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, i64::from(StatusCode::SERVICE_UNAVAILABLE.as_u16()));
    assert_ne!(events[0].quota_failover_count, 1);
}

#[tokio::test]
async fn codex_dispatch_fails_over_recoverable_overload_in_forced_sse_non_stream() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_overload_sse))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    let status = response.status();
    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert!(
        auths.contains(&"Bearer upstream-token-b".to_string()),
        "recoverable forced-SSE overload should fail over to the second account; client \
         status={status}, attempted accounts={auths:?}"
    );
}

#[tokio::test]
async fn codex_dispatch_strict_session_rejection_blocks_repeated_cyber_policy_session() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_cyber_policy))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let mut route = codex_route_for_account("codex-a", "upstream-token-a");
    route.codex_strict_session_rejection_enabled = true;
    let state = super::ProviderState::new(
        Arc::new(TestStore),
        Arc::new(StaticRouteStore {
            codex_route: route,
            kiro_route: static_kiro_route(),
        }),
    );
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "prompt_cache_key": "strict-session-a",
                        "input": "hello",
                        "stream": false
                    }"#,
            ))
            .expect("request")
    };

    let first = super::provider_entry(state.clone(), request()).await;
    let second = super::provider_entry(state, request()).await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(first.status(), StatusCode::BAD_REQUEST);
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    let second_body = to_bytes(second.into_body(), usize::MAX)
        .await
        .expect("second response body");
    let second_body = String::from_utf8(second_body.to_vec()).expect("utf8 response");
    assert!(second_body.contains("start a new session"), "body: {second_body}");

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn codex_dispatch_context_window_does_not_strict_block_session() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_context_window_exceeded))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let mut route = codex_route_for_account("codex-a", "upstream-token-a");
    route.codex_strict_session_rejection_enabled = true;
    let state = super::ProviderState::new(
        Arc::new(TestStore),
        Arc::new(StaticRouteStore {
            codex_route: route,
            kiro_route: static_kiro_route(),
        }),
    );
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "prompt_cache_key": "strict-session-a",
                        "input": "hello",
                        "stream": false
                    }"#,
            ))
            .expect("request")
    };

    let first = super::provider_entry(state.clone(), request()).await;
    let second = super::provider_entry(state, request()).await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(first.status(), StatusCode::BAD_REQUEST);
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    let second_body = to_bytes(second.into_body(), usize::MAX)
        .await
        .expect("second response body");
    let second_body = String::from_utf8(second_body.to_vec()).expect("utf8 response");
    assert!(!second_body.contains("start a new session"), "body: {second_body}");

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn codex_dispatch_strict_session_rejection_ignores_derived_sessions() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_cyber_policy))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let mut route = codex_route_for_account("codex-a", "upstream-token-a");
    route.codex_strict_session_rejection_enabled = true;
    let state = super::ProviderState::new(
        Arc::new(TestStore),
        Arc::new(StaticRouteStore {
            codex_route: route,
            kiro_route: static_kiro_route(),
        }),
    );
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "input": "hello",
                        "stream": false
                    }"#,
            ))
            .expect("request")
    };

    let first = super::provider_entry(state.clone(), request()).await;
    let second = super::provider_entry(state, request()).await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(first.status(), StatusCode::BAD_REQUEST);
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    let second_body = to_bytes(second.into_body(), usize::MAX)
        .await
        .expect("second response body");
    let second_body = String::from_utf8(second_body.to_vec()).expect("utf8 response");
    assert!(!second_body.contains("start a new session"), "body: {second_body}");

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn codex_compact_forwards_conversation_header_without_injecting_prompt_cache_key() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .route("/v1/responses/compact", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/responses/compact")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .header("conversation_id", "compact-thread")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "input": "hello"
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].conversation_id.as_deref(), Some("compact-thread"));
    assert_eq!(requests[0].session_id, None);
    assert_eq!(requests[0].x_client_request_id, None);
    assert_eq!(requests[0].body.get("prompt_cache_key"), None);
}

#[tokio::test]
async fn codex_models_fetches_upstream_with_runtime_client_version() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/models", get(fake_codex_models))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let config = AdminRuntimeConfig {
        codex_client_version: "0.125.0".to_string(),
        ..AdminRuntimeConfig::default()
    };
    let state = super::ProviderState::new_with_config_store(
        Arc::new(TestStore),
        static_codex_route_store(),
        Arc::new(StaticAdminConfigStore {
            config,
        }),
    );
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("GET")
            .uri("/api/llm-gateway/v1/models")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok()),
        Some(r#""models-test""#)
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["object"], "list");
    let ids = body["data"]
        .as_array()
        .expect("model data")
        .iter()
        .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    assert!(ids.contains(&"gpt-5.3-codex"));
    assert!(ids.contains(&"gpt-5.5"));

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(requests[0].query.as_deref(), Some("client_version=0.125.0"));
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer upstream-token"));
    assert_eq!(requests[0].accept.as_deref(), Some("application/json"));
    assert_eq!(requests[0].user_agent.as_deref(), Some("codex_cli_rs/0.125.0"));
}

#[tokio::test]
async fn kiro_models_fetches_local_catalog_on_root_models_route() {
    let state = super::ProviderState::new(Arc::new(TestStore), empty_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("GET")
            .uri("/v1/models")
            .header("x-api-key", "valid-secret")
            .body(Body::empty())
            .expect("request"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["object"], "list");
    let ids = body["data"]
        .as_array()
        .expect("model data")
        .iter()
        .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    assert!(ids.contains(&"claude-sonnet-5"));
    assert!(ids.contains(&"claude-sonnet-5-thinking"));
    assert!(ids.contains(&"claude-opus-4-7"));
    assert!(ids.contains(&"claude-opus-4-7-thinking"));
    assert!(ids.contains(&"claude-opus-4-8"));
    assert!(ids.contains(&"claude-opus-4-8-thinking"));
}

#[tokio::test]
async fn codex_dispatch_streams_chat_completion_chunks_from_responses_sse() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .route("/v1/responses/compact", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains(r#""object":"chat.completion.chunk""#));
    assert!(body.contains(r#""model":"gpt-5.3-codex""#));
    assert!(body.contains(r#""content":"hello ""#));
    assert!(body.contains(r#""content":"back""#));
    assert!(body.contains("data: [DONE]\n\n"));

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(requests[0].accept.as_deref(), Some("text/event-stream"));
    assert_eq!(requests[0].body["stream"], true);
}

#[tokio::test]
async fn codex_dispatch_adapts_non_streaming_anthropic_messages_from_responses_sse() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/llm-gateway/v1/messages")
            .header("x-api-key", "codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["type"], "message");
    assert_eq!(body["role"], "assistant");
    assert_eq!(body["content"][0]["type"], "text");
    assert_eq!(body["content"][0]["text"], "hello back");
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(body["usage"]["input_tokens"], 10);
    assert_eq!(body["usage"]["cache_read_input_tokens"], 2);
    assert_eq!(body["usage"]["output_tokens"], 3);

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer upstream-token"));
    assert_eq!(requests[0].accept.as_deref(), Some("text/event-stream"));

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].protocol_family, llm_access_core::provider::ProtocolFamily::Anthropic);
    assert_eq!(events[0].endpoint, "/v1/messages");
}

#[tokio::test]
async fn codex_responses_drops_previous_response_id_when_store_is_false() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_json_success))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let first = super::provider_entry(
            state.clone(),
            Request::builder()
                .method("POST")
                .uri("/api/llm-gateway/v1/responses")
                .header(header::AUTHORIZATION, "Bearer codex-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model": "gpt-5.3-codex",
                        "input": [{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}],
                        "max_output_tokens": 64,
                        "stream": false
                    }"#,
                ))
                .expect("request"),
        )
        .await;

    assert_eq!(first.status(), StatusCode::OK);
    let first_body = to_bytes(first.into_body(), usize::MAX)
        .await
        .expect("first response body");
    let first_body =
        serde_json::from_slice::<serde_json::Value>(&first_body).expect("first json response");
    let previous_response_id = first_body["id"]
        .as_str()
        .expect("upstream response id")
        .to_string();
    assert_eq!(previous_response_id, "rs_compact_1");

    let second = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/llm-gateway/v1/responses")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "gpt-5.3-codex",
                    "previous_response_id": previous_response_id,
                    "max_output_tokens": 64,
                    "input": [
                        {
                            "type":"message",
                            "id":"rs_item_1",
                            "role":"assistant",
                            "content":[{"type":"output_text","text":"prev"}]
                        },
                        {
                            "type":"message",
                            "role":"user",
                            "content":[{"type":"input_text","text":"next"}]
                        }
                    ],
                    "stream": false
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(second.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].body.get("previous_response_id"), None);
    assert_eq!(requests[1].body.get("max_output_tokens"), None);
    assert_eq!(requests[1].body.get("store"), Some(&json!(false)));
    assert_eq!(requests[1].body["input"][0].get("id"), None);
    let input = requests[1].body["input"]
        .as_array()
        .expect("upstream input array");
    assert_eq!(input.len(), 2);
    assert_eq!(input[1]["role"], json!("user"));
}

#[tokio::test]
async fn codex_compact_preserves_previous_response_id_without_local_anchor() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_json_success))
        .route("/v1/responses/compact", post(fake_codex_responses_json_success))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let first = super::provider_entry(
        state.clone(),
        Request::builder()
            .method("POST")
            .uri("/api/llm-gateway/v1/responses/compact")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "input": "hello compact",
                        "max_output_tokens": 64
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    assert_eq!(first.status(), StatusCode::OK);
    let first_body = to_bytes(first.into_body(), usize::MAX)
        .await
        .expect("first response body");
    let first_body =
        serde_json::from_slice::<serde_json::Value>(&first_body).expect("first json response");
    let previous_response_id = first_body["id"]
        .as_str()
        .expect("upstream response id")
        .to_string();
    assert_eq!(previous_response_id, "rs_compact_1");

    let second = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/llm-gateway/v1/responses/compact")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "model": "gpt-5.3-codex",
                    "previous_response_id": previous_response_id,
                    "input": "next compact",
                    "max_output_tokens": 64,
                    "store": true,
                    "include": ["reasoning.encrypted_content"],
                    "client_metadata": {"source": "test"},
                    "tool_choice": "required"
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(second.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].body.get("previous_response_id"), None);
    assert_eq!(requests[1].body["input"], json!("next compact"));
    assert_eq!(requests[1].body.get("max_output_tokens"), None);
    assert_eq!(requests[1].body.get("store"), None);
    assert_eq!(requests[1].body.get("include"), None);
    assert_eq!(requests[1].body.get("client_metadata"), None);
    assert_eq!(requests[1].body.get("tool_choice"), None);
}


#[tokio::test]
async fn codex_dispatch_rejects_invalid_anthropic_messages_with_json_error_and_usage() {
    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/llm-gateway/v1/messages")
            .header("x-api-key", "codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("[]"))
            .expect("request"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(body["error"]["message"]
        .as_str()
        .expect("error message")
        .contains("messages requires a JSON object body"));

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 400);
    assert_eq!(events[0].endpoint, "/v1/messages");
    assert_eq!(events[0].request_url, "/api/llm-gateway/v1/messages");
    assert_eq!(events[0].account_name, None);
    assert!(events[0].client_request_body_json.is_some());
}

#[tokio::test]
async fn codex_dispatch_adapts_upstream_error_for_anthropic_messages() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_always_bad_gateway))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/llm-gateway/v1/messages")
            .header("x-api-key", "codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["error"]["type"], "api_error");
    assert_eq!(body["error"]["message"], "temporary upstream failure");

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 502);
    assert_eq!(events[0].endpoint, "/v1/messages");
    assert_eq!(events[0].account_name.as_deref(), Some("codex-a"));
    assert!(events[0].client_request_body_json.is_some());
    assert!(events[0].upstream_request_body_json.is_some());
    assert!(events[0].full_request_json.is_some());
}

#[tokio::test]
async fn codex_dispatch_adapts_failed_sse_for_anthropic_messages() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_failed_sse))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/llm-gateway/v1/messages")
            .header("x-api-key", "codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "tools": [{
                            "name": "lookup",
                            "description": "lookup tool",
                            "input_schema": {"type": "object", "properties": {}}
                        }],
                        "tool_choice": {"type": "tool", "name": "missing_tool"},
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert_eq!(body["error"]["message"], "tool_choice references a missing tool");

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 400);
    assert_eq!(events[0].endpoint, "/v1/messages");
    assert_eq!(events[0].account_name.as_deref(), Some("codex-a"));
}

#[tokio::test]
async fn codex_dispatch_streaming_first_failure_returns_error_without_sse_body() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_failed_sse))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("tool_choice references a missing tool"));
    assert!(!body.contains("event: response.failed"));

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 400);
}

#[tokio::test]
async fn codex_dispatch_streaming_mid_failure_emits_error_done_and_records_failure() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_mid_stream_failed_sse))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("hello "), "body: {body}");
    assert!(body.contains("tool_choice references a missing tool"), "body: {body}");
    assert!(body.contains("[DONE]"), "body: {body}");

    wait_for_usage_event_count(store.as_ref(), 1).await;
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 400);
    assert_eq!(events[0].error_message.as_deref(), Some("tool_choice references a missing tool"));
    assert!(events[0].usage_missing);
}

#[tokio::test]
async fn codex_dispatch_streams_anthropic_messages_events_from_responses_sse() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/llm-gateway/messages")
            .header("x-api-key", "codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("event: message_start"));
    assert!(body.contains("event: content_block_start"));
    assert!(body.contains("event: content_block_delta"));
    assert!(body.contains(r#""type":"text_delta""#));
    assert!(body.contains(r#""text":"hello ""#));
    assert!(body.contains(r#""text":"back""#));
    assert!(body.contains("event: message_stop"));
    assert!(!body.contains("[DONE]"));
}

#[tokio::test]
async fn codex_dispatch_streams_anthropic_tool_use_input_deltas_from_responses_sse() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_custom_tool_stream))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/llm-gateway/messages")
            .header("x-api-key", "codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("event: content_block_start"));
    assert!(body.contains(r#""type":"tool_use""#));
    assert!(body.contains(r#""name":"apply_patch""#));
    assert!(body.contains(r#""type":"input_json_delta""#));
    assert!(body.contains(r#""partial_json":"*** Begin Patch""#));
    assert!(body.contains("event: content_block_stop"));
}

#[tokio::test]
async fn codex_dispatch_records_usage_rollup_from_completed_response() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.key_id, "key-usage");
    assert_eq!(event.key_name, "usage-key");
    assert_eq!(event.account_name.as_deref(), Some("codex-a"));
    assert_eq!(event.endpoint, "/v1/chat/completions");
    assert_eq!(event.model.as_deref(), Some("gpt-5.3-codex"));
    assert_eq!(event.mapped_model.as_deref(), Some("gpt-5.3-codex-spark"));
    assert_eq!(event.status_code, 200);
    assert_eq!(event.input_uncached_tokens, 10);
    assert_eq!(event.input_cached_tokens, 2);
    assert_eq!(event.output_tokens, 3);
    assert_eq!(event.billable_tokens, 25);
    assert!(!event.usage_missing);
    assert_eq!(event.client_request_body_json, None);
    assert_eq!(event.upstream_request_body_json, None);
    assert_eq!(event.full_request_json, None);
}

#[tokio::test]
async fn codex_dispatch_strips_fast_service_tier_when_key_disables_fast() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let mut route = codex_route_for_account("codex-a", "upstream-token");
    route.codex_fast_enabled = false;
    let state = super::ProviderState::new(
        store.clone(),
        Arc::new(StaticRouteStore {
            codex_route: route,
            kiro_route: static_kiro_route(),
        }),
    );
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "service_tier": "fast",
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body.get("service_tier"), None);
    drop(requests);

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].billable_tokens, 25);
}

#[tokio::test]
async fn codex_dispatch_keeps_fast_service_tier_when_key_enables_fast() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_codex_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "service_tier": "fast",
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["service_tier"], "priority");
    drop(requests);

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].billable_tokens, 50);
}

#[tokio::test]
async fn codex_dispatch_repairs_chat_tool_call_without_output_and_records_usage() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses))
        .with_state(captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_codex_route_store());
    let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::AUTHORIZATION, "Bearer codex-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model":"gpt-5.3-codex",
                        "messages":[
                            {"role":"user","content":"hello"},
                            {"role":"assistant","tool_calls":[{"id":"callauto12","type":"function","function":{"name":"lookup","arguments":"{}"}}]}
                        ]
                    }"#,
                ))
                .expect("request"),
        )
        .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "hello back");

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 200);
    assert_eq!(events[0].endpoint, "/v1/chat/completions");
    assert_eq!(events[0].request_url, "/v1/chat/completions");
    assert_eq!(events[0].account_name.as_deref(), Some("codex-a"));
}

#[tokio::test]
async fn codex_dispatch_cools_down_quota_exhausted_account_between_requests() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_quota_then_success))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request")
    };

    let first = super::provider_entry(state.clone(), request()).await;
    let second = super::provider_entry(state, request()).await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec![
        "Bearer upstream-token-a".to_string(),
        "Bearer upstream-token-b".to_string(),
        "Bearer upstream-token-b".to_string(),
    ]);
}

#[tokio::test]
async fn codex_dispatch_uses_default_failover_limit_of_ten() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_fail_first_three))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
            codex_route_for_account("codex-c", "upstream-token-c"),
            codex_route_for_account("codex-d", "upstream-token-d"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec![
        "Bearer upstream-token-a".to_string(),
        "Bearer upstream-token-b".to_string(),
        "Bearer upstream-token-c".to_string(),
        "Bearer upstream-token-d".to_string(),
    ]);
}

#[tokio::test]
async fn codex_dispatch_cools_down_transiently_failed_accounts_between_requests() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_fail_first_three))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
            codex_route_for_account("codex-c", "upstream-token-c"),
            codex_route_for_account("codex-d", "upstream-token-d"),
        ],
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store);
    let request = || {
        Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, "Bearer codex-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model": "gpt-5.3-codex",
                        "input": [{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}],
                        "max_output_tokens": 64,
                        "stream": false
                    }"#,
                ))
                .expect("request")
    };

    let first = super::provider_entry(state.clone(), request()).await;
    let second = super::provider_entry(state, request()).await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec![
        "Bearer upstream-token-a".to_string(),
        "Bearer upstream-token-b".to_string(),
        "Bearer upstream-token-c".to_string(),
        "Bearer upstream-token-d".to_string(),
        "Bearer upstream-token-d".to_string(),
    ]);
}

#[tokio::test]
async fn codex_dispatch_respects_runtime_account_failure_retry_limit() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_fail_first_three))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let route_store = Arc::new(StaticMultiCodexRouteStore {
        codex_routes: vec![
            codex_route_for_account("codex-a", "upstream-token-a"),
            codex_route_for_account("codex-b", "upstream-token-b"),
            codex_route_for_account("codex-c", "upstream-token-c"),
            codex_route_for_account("codex-d", "upstream-token-d"),
        ],
        kiro_route: static_kiro_route(),
    });
    let config = AdminRuntimeConfig {
        account_failure_retry_limit: 2,
        ..AdminRuntimeConfig::default()
    };
    let state = super::ProviderState::new_with_config_store(
        Arc::new(TestStore),
        route_store,
        Arc::new(StaticAdminConfigStore {
            config,
        }),
    );
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 body");
    assert!(body.contains("temporary upstream failure"));
    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec![
        "Bearer upstream-token-a".to_string(),
        "Bearer upstream-token-b".to_string(),
    ]);
}

#[tokio::test]
async fn codex_dispatch_persists_terminal_request_auth_error_after_forced_refresh_failure() {
    let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
        .lock()
        .expect("codex upstream env lock");
    let captured = Arc::new(CapturedCodexUpstream::default());
    let app = Router::new()
        .route("/v1/responses", post(fake_codex_responses_always_unauthorized))
        .with_state(captured.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake upstream");
    let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve fake upstream");
    });
    std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

    let stale_route = codex_route_for_account("codex-a", "upstream-token-stale");
    let latest_route = codex_route_for_account("codex-a", "upstream-token-fresh");
    let route_store = Arc::new(RefreshingCodexRouteStore {
        candidate_routes: vec![stale_route],
        latest_routes: Arc::new(Mutex::new(HashMap::from([(
            "codex-a".to_string(),
            latest_route.clone(),
        )]))),
        codex_updates: Arc::new(Mutex::new(Vec::new())),
        kiro_route: static_kiro_route(),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store.clone());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec![
        "Bearer upstream-token-stale".to_string(),
        "Bearer upstream-token-fresh".to_string(),
    ]);

    let updates = route_store.codex_updates.lock().expect("codex updates");
    assert_eq!(updates.len(), 1);
    let update = &updates[0];
    assert_eq!(update.account_name, "codex-a");
    assert_eq!(update.auth_json, latest_route.auth_json);
    let error = update
        .last_error
        .as_deref()
        .expect("request auth error should be persisted");
    assert!(error.contains("codex request returned 401 Unauthorized after forced refresh"));
    assert!(error.contains("access token rejected"));
    assert!(is_terminal_codex_auth_error(error));
}

#[tokio::test]
async fn codex_route_selection_skips_terminal_auth_error_routes() {
    let limiter = Arc::new(RequestLimiter::default());
    let cooldowns = Arc::new(CodexAccountCooldowns::default());
    let mut blocked = codex_route_for_account("codex-a", "upstream-token-a");
    blocked.cached_error_message = Some(
        "codex refresh token returned 401 Unauthorized: \
         {\"error\":{\"code\":\"refresh_token_reused\"}}"
            .to_string(),
    );
    let healthy = codex_route_for_account("codex-b", "upstream-token-b");

    let (route, _permit) = select_codex_route_with_account_permit(
        &limiter,
        &cooldowns,
        &[blocked, healthy],
        &HashSet::new(),
        None,
        None,
    )
    .await
    .expect("healthy route should still be selected");

    assert_eq!(route.account_name, "codex-b");
}

#[tokio::test]
async fn codex_route_selection_prefers_affinity_account_when_usable() {
    let limiter = Arc::new(RequestLimiter::default());
    let cooldowns = Arc::new(CodexAccountCooldowns::default());
    let first = codex_route_for_account("codex-a", "upstream-token-a");
    let preferred = codex_route_for_account("codex-b", "upstream-token-b");

    let (route, _permit) = select_codex_route_with_account_permit(
        &limiter,
        &cooldowns,
        &[first, preferred],
        &HashSet::new(),
        Some("codex-b"),
        None,
    )
    .await
    .expect("preferred route should be selected");

    assert_eq!(route.account_name, "codex-b");
}

#[tokio::test]
async fn codex_route_selection_spreads_new_sessions_to_least_bound_account() {
    let limiter = Arc::new(RequestLimiter::default());
    let cooldowns = Arc::new(CodexAccountCooldowns::default());
    let alpha = codex_route_for_account("codex-a", "upstream-token-a");
    let beta = codex_route_for_account("codex-b", "upstream-token-b");
    let mut session_counts = HashMap::new();
    session_counts.insert("codex-a".to_string(), 5usize);
    session_counts.insert("codex-b".to_string(), 1usize);

    let (route, _permit) = select_codex_route_with_account_permit(
        &limiter,
        &cooldowns,
        &[alpha, beta],
        &HashSet::new(),
        None,
        Some(&session_counts),
    )
    .await
    .expect("least-bound route should be selected");

    assert_eq!(route.account_name, "codex-b");
}

#[test]
fn codex_affinity_id_uses_explicit_resolved_session() {
    let affinity = build_codex_affinity_id(
        "key-a",
        Some("session-a"),
        Some(CodexResolvedSessionSource::HeaderSessionId),
    )
    .expect("explicit affinity id");

    assert_eq!(affinity.source, CodexAffinitySource::Explicit);
    assert_eq!(affinity.key.as_str(), "5:key-asession-a");
}

#[test]
fn codex_affinity_id_uses_derived_source_for_generated_session() {
    let affinity = build_codex_affinity_id(
        "key-a",
        Some("codex-session-v1-abcdef"),
        Some(CodexResolvedSessionSource::StablePrefix),
    )
    .expect("derived affinity id");

    assert_eq!(affinity.source, CodexAffinitySource::Derived);
    assert_eq!(affinity.key.as_str(), "5:key-acodex-session-v1-abcdef");
}

#[test]
fn codex_session_affinity_remembers_and_expires_entries() {
    let affinity = CodexSessionAffinity::default();
    let config = CodexAffinityRuntimeConfig {
        session_ttl: Duration::from_millis(1),
        ..CodexAffinityRuntimeConfig::default()
    };
    let affinity_id = CodexAffinityId {
        key: "key-a:explicit:session-a".to_string(),
        source: CodexAffinitySource::Explicit,
    };

    affinity.remember(&affinity_id, "codex-a", &config);
    assert_eq!(affinity.lookup(&affinity_id, &config).as_deref(), Some("codex-a"));

    std::thread::sleep(Duration::from_millis(5));
    assert!(affinity.lookup(&affinity_id, &config).is_none());
}

#[test]
fn codex_session_affinity_preserves_entries_when_capacity_changes() {
    let affinity = CodexSessionAffinity::default();
    let initial_config = CodexAffinityRuntimeConfig {
        max_entries: 2,
        ..CodexAffinityRuntimeConfig::default()
    };
    let expanded_config = CodexAffinityRuntimeConfig {
        max_entries: 4,
        ..CodexAffinityRuntimeConfig::default()
    };
    let affinity_id = CodexAffinityId {
        key: "key-a:explicit:session-a".to_string(),
        source: CodexAffinitySource::Explicit,
    };

    affinity.remember(&affinity_id, "codex-a", &initial_config);

    assert_eq!(affinity.lookup(&affinity_id, &expanded_config).as_deref(), Some("codex-a"));
}

#[test]
fn codex_session_affinity_ignores_derived_sessions_when_fallback_affinity_is_disabled() {
    let affinity = CodexSessionAffinity::default();
    let config = CodexAffinityRuntimeConfig {
        fallback_enabled: false,
        ..CodexAffinityRuntimeConfig::default()
    };
    let affinity_id = CodexAffinityId {
        key: "key-a:derived:session-a".to_string(),
        source: CodexAffinitySource::Derived,
    };

    affinity.remember(&affinity_id, "codex-a", &config);

    assert!(affinity.lookup(&affinity_id, &config).is_none());
    assert!(affinity.account_session_counts(&config).is_empty());
}

#[tokio::test]
async fn codex_route_selection_returns_bad_gateway_when_all_routes_have_terminal_auth_errors() {
    let limiter = Arc::new(RequestLimiter::default());
    let cooldowns = Arc::new(CodexAccountCooldowns::default());
    let mut blocked_a = codex_route_for_account("codex-a", "upstream-token-a");
    blocked_a.cached_error_message = Some(
        "codex refresh token returned 401 Unauthorized: \
         {\"error\":{\"code\":\"refresh_token_reused\"}}"
            .to_string(),
    );
    let mut blocked_b = codex_route_for_account("codex-b", "upstream-token-b");
    blocked_b.cached_error_message = Some(
        "codex refresh token returned 401 Unauthorized: \
         {\"error\":{\"code\":\"refresh_token_invalidated\"}}"
            .to_string(),
    );

    let response = match select_codex_route_with_account_permit(
        &limiter,
        &cooldowns,
        &[blocked_a, blocked_b],
        &HashSet::new(),
        None,
        None,
    )
    .await
    {
        Ok(_) => panic!("terminal auth errors should block all routes"),
        Err(response) => response,
    };

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 body");
    assert!(body.contains("all eligible codex accounts failed for this request"));
}

#[test]
fn codex_account_cooldown_marks_only_extend_existing_window() {
    let cooldowns = CodexAccountCooldowns::default();
    cooldowns.mark_account_cooldown("codex-a", Duration::from_secs(60));
    let first_until = *cooldowns
        .blocked_until
        .lock()
        .expect("cooldown mutex")
        .get("codex-a")
        .expect("initial cooldown");

    cooldowns.mark_account_cooldown("codex-a", Duration::from_secs(5));
    let second_until = *cooldowns
        .blocked_until
        .lock()
        .expect("cooldown mutex")
        .get("codex-a")
        .expect("shorter cooldown should not remove entry");
    assert!(second_until >= first_until);

    cooldowns.mark_account_cooldown("codex-a", Duration::from_secs(90));
    let third_until = *cooldowns
        .blocked_until
        .lock()
        .expect("cooldown mutex")
        .get("codex-a")
        .expect("longer cooldown should keep entry");
    assert!(third_until > second_until);
}

#[tokio::test]
async fn kiro_dispatch_adapts_non_streaming_messages_from_eventstream() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["type"], "text");
    assert_eq!(body["content"][0]["text"], "hello back");
    assert_eq!(body["usage"]["input_tokens"], 1);
    assert_eq!(body["usage"]["cache_creation_input_tokens"], 0);
    assert_eq!(body["usage"]["output_tokens"], 3);

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/generateAssistantResponse");
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer kiro-upstream-token"));
    assert_eq!(requests[0].body["profileArn"], "arn:aws:kiro:test");
}

#[tokio::test]
async fn kiro_generate_uses_fixed_social_profile_arn_when_route_is_missing_it() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let mut route = static_kiro_route_with_auth_method_and_provider("social", "github");
    route.profile_arn = None;
    let state = super::ProviderState::new(
        Arc::new(TestStore),
        Arc::new(StaticRouteStore {
            codex_route: ProviderCodexRoute {
                account_name: "codex-a".to_string(),
                account_group_id_at_event: None,
                route_strategy_at_event: RouteStrategy::Auto,
                auth_json: r#"{"access_token":"upstream-token"}"#.to_string(),
                map_gpt53_codex_to_spark: true,
                auth_refresh_enabled: true,
                codex_fast_enabled: true,
                codex_strict_session_rejection_enabled: false,
                codex_image_generation_enabled: true,
                codex_image_direct_generation_enabled: false,
                request_max_concurrency: None,
                request_min_start_interval_ms: None,
                account_request_max_concurrency: None,
                account_request_min_start_interval_ms: None,
                account_codex_image_generation_enabled: true,
                account_codex_image_generation_max_concurrency:
                    llm_access_core::store::DEFAULT_CODEX_IMAGE_GENERATION_MAX_CONCURRENCY,
                cached_error_message: None,
                proxy: None,
            },
            kiro_route: route,
        }),
    );

    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["profileArn"],
        "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK"
    );
}

#[tokio::test]
async fn kiro_generate_headers_include_runtime_middleware_for_external_idp_internal() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    let expected_host = upstream_base
        .strip_prefix("http://")
        .expect("http upstream host")
        .to_string();
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let route = static_kiro_route_with_auth_method_and_provider("external_idp", "Internal");
    let state = super::ProviderState::new(
        Arc::new(TestStore),
        Arc::new(StaticRouteStore {
            codex_route: ProviderCodexRoute {
                account_name: "codex-a".to_string(),
                account_group_id_at_event: None,
                route_strategy_at_event: RouteStrategy::Auto,
                auth_json: r#"{"access_token":"upstream-token"}"#.to_string(),
                map_gpt53_codex_to_spark: true,
                auth_refresh_enabled: true,
                codex_fast_enabled: true,
                codex_strict_session_rejection_enabled: false,
                codex_image_generation_enabled: true,
                codex_image_direct_generation_enabled: false,
                request_max_concurrency: None,
                request_min_start_interval_ms: None,
                account_request_max_concurrency: None,
                account_request_min_start_interval_ms: None,
                account_codex_image_generation_enabled: true,
                account_codex_image_generation_max_concurrency:
                    llm_access_core::store::DEFAULT_CODEX_IMAGE_GENERATION_MAX_CONCURRENCY,
                cached_error_message: None,
                proxy: None,
            },
            kiro_route: route,
        }),
    );
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/generateAssistantResponse");
    assert_eq!(requests[0].token_type.as_deref(), Some("EXTERNAL_IDP"));
    assert_eq!(requests[0].redirect_for_internal.as_deref(), Some("true"));
    assert_eq!(requests[0].agent_mode.as_deref(), Some("vibe"));
    assert_eq!(requests[0].opt_out.as_deref(), Some("true"));
    assert_eq!(requests[0].host.as_deref(), Some(expected_host.as_str()));
    assert!(requests[0]
        .x_amz_user_agent
        .as_deref()
        .is_some_and(|value| value.contains("aws-sdk-js/1.0.34")));
    assert!(requests[0]
        .x_amz_user_agent
        .as_deref()
        .is_some_and(|value| value.contains("KiroIDE-0.12.155-")));
    assert!(requests[0]
        .user_agent
        .as_deref()
        .is_some_and(|value| value.contains("api/codewhispererstreaming#1.0.34")));
    assert!(requests[0]
        .user_agent
        .as_deref()
        .is_some_and(|value| !value.contains(" m/")));
}

#[tokio::test]
async fn kiro_mcp_headers_match_streaming_client_middleware_without_chat_only_headers() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    let expected_host = upstream_base
        .strip_prefix("http://")
        .expect("http upstream host")
        .to_string();
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let route = static_kiro_route_with_auth_method_and_provider("external_idp", "Internal");
    let request_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    })
    .to_string();
    let resolver = crate::geoip::GeoIpResolver::fixed_for_tests("local");
    let headers = HeaderMap::new();
    let uri = "/mcp".parse().expect("mcp uri");
    let mut usage_meta = super::ProviderUsageMetadata::from_request_parts(
        &super::Method::POST,
        &uri,
        &headers,
        &resolver,
    )
    .await;
    let response = super::call_kiro_mcp_for_route(
        &route,
        empty_route_store().as_ref(),
        &request_body,
        &mut usage_meta,
    )
    .await
    .expect("mcp response");

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert!(response.result.is_some());
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/mcp");
    assert_eq!(requests[0].token_type.as_deref(), Some("EXTERNAL_IDP"));
    assert_eq!(requests[0].redirect_for_internal.as_deref(), Some("true"));
    assert_eq!(requests[0].agent_mode, None);
    assert_eq!(requests[0].opt_out, None);
    assert_eq!(requests[0].host.as_deref(), Some(expected_host.as_str()));
}

#[tokio::test]
async fn kiro_usage_headers_match_runtime_client_middleware_without_chat_only_headers() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    let expected_host = upstream_base
        .strip_prefix("http://")
        .expect("http upstream host")
        .to_string();
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let route = static_kiro_route_with_auth_method_and_provider("external_idp", "Internal");
    let usage = crate::kiro_refresh::fetch_usage_limits_for_route(
        &route,
        empty_route_store().as_ref(),
        false,
    )
    .await
    .expect("usage response");

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(
        usage
            .subscription_info
            .as_ref()
            .and_then(|info| info.subscription_title.as_deref()),
        Some("Pro")
    );
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/getUsageLimits");
    assert_eq!(requests[0].token_type.as_deref(), Some("EXTERNAL_IDP"));
    assert_eq!(requests[0].redirect_for_internal.as_deref(), Some("true"));
    assert_eq!(requests[0].agent_mode, None);
    assert_eq!(requests[0].opt_out, None);
    assert_eq!(requests[0].host.as_deref(), Some(expected_host.as_str()));
    assert!(requests[0]
        .user_agent
        .as_deref()
        .is_some_and(|value| value.contains("api/codewhispererruntime#1.0.0")));
    assert!(requests[0]
        .user_agent
        .as_deref()
        .is_some_and(|value| !value.contains(" m/")));
}

#[tokio::test]
async fn kiro_dispatch_streaming_messages_normalize_reasoning_signature() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_reasoning_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-opus-4-7",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true,
                        "thinking": {"type": "adaptive", "display": "summarized"},
                        "output_config": {"effort": "medium"}
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains(r#""type":"thinking_delta""#));
    assert!(body.contains(r#""thinking":"先想一步""#));
    assert!(body.contains(r#""type":"signature_delta""#));
    assert!(!body.contains(r#""signature":"upstream-signature-47""#));
    assert!(body.contains(r#""type":"text_delta""#));
    assert!(body.contains(r#""text":"最终答案""#));
}

#[tokio::test]
async fn kiro_dispatch_streaming_messages_hides_adaptive_bare_reasoning() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_reasoning_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-opus-4-8",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true,
                        "thinking": {"type": "adaptive"}
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(!body.contains(r#""type":"thinking_delta""#));
    assert!(!body.contains(r#""type":"signature_delta""#));
    assert!(body.contains(r#""type":"text_delta""#));
    assert!(body.contains(r#""text":"最终答案""#));

    let requests = captured.requests.lock().expect("captured requests");
    let current_content = requests[0].body["conversationState"]["currentMessage"]
        ["userInputMessage"]["content"]
        .as_str()
        .expect("current content");
    assert!(current_content.contains("<thinking_mode>adaptive</thinking_mode>"));
    assert!(current_content.contains("<thinking_effort>xhigh</thinking_effort>"));
}

#[tokio::test]
async fn kiro_dispatch_non_stream_messages_normalize_reasoning_signature() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_reasoning_upstream(captured).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-opus-4-7",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false,
                        "thinking": {"type": "adaptive", "display": "summarized"},
                        "output_config": {"effort": "medium"}
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["content"][0]["type"], "thinking");
    assert_eq!(body["content"][0]["thinking"], "先想一步");
    assert_ne!(body["content"][0]["signature"], "upstream-signature-47");
    assert!(body["content"][0]["signature"]
        .as_str()
        .is_some_and(|signature| signature.len() == 436));
    assert_eq!(body["content"][1]["type"], "text");
    assert_eq!(body["content"][1]["text"], "最终答案");
}

#[tokio::test]
async fn kiro_dispatch_rejects_tampered_thinking_when_protected_content_validation_enabled() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    std::env::set_var("KIRO_THINKING_SIGNATURE_SECRET", "server-secret");
    let signature = protected_thinking_signature(
        "claude-opus-4-8",
        "private reasoning",
        "key-1",
        "server-secret",
    );
    let body = json!({
        "model": "claude-opus-4-8",
        "max_tokens": 128,
        "messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": [
                {
                    "type": "thinking",
                    "thinking": "changed reasoning",
                    "signature": signature
                }
            ]},
            {"role": "user", "content": "continue"}
        ]
    });
    let state = super::ProviderState::new(Arc::new(TestStore), protected_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await;
    std::env::remove_var("KIRO_THINKING_SIGNATURE_SECRET");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(response.headers().get("x-amzn-errortype").is_none());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("Bedrock"));
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("ValidationException"));
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("invalid thinking signature"));
}

#[tokio::test]
async fn kiro_dispatch_rejects_encrypted_content_when_protected_content_validation_enabled() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    std::env::set_var("KIRO_THINKING_SIGNATURE_SECRET", "server-secret");
    let body = json!({
        "model": "claude-opus-4-8",
        "max_tokens": 128,
        "messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": [
                {
                    "type": "text",
                    "text": "answer",
                    "encrypted_content": "opaque"
                }
            ]},
            {"role": "user", "content": "continue"}
        ]
    });
    let state = super::ProviderState::new(Arc::new(TestStore), protected_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await;
    std::env::remove_var("KIRO_THINKING_SIGNATURE_SECRET");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(response.headers().get("x-amzn-errortype").is_none());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("Bedrock"));
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("ValidationException"));
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("encrypted_content"));
}

#[tokio::test]
async fn kiro_dispatch_reports_missing_protected_secret_as_anthropic_error_with_bedrock_message() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    std::env::remove_var("KIRO_THINKING_SIGNATURE_SECRET");
    let body = json!({
        "model": "claude-opus-4-8",
        "max_tokens": 128,
        "messages": [{"role": "user", "content": "hello"}]
    });
    let state = super::ProviderState::new(Arc::new(TestStore), protected_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.headers().get("x-amzn-errortype").is_none());
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
    assert_eq!(body["error"]["type"], "api_error");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("Bedrock"));
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("InternalServerException"));
    assert!(body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("protected content validation"));
}

#[tokio::test]
async fn kiro_dispatch_does_not_refresh_missing_status_on_request_path() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let mut route = static_kiro_route();
    route.auth_json = format!(
        r#"{{
                "accessToken":"kiro-upstream-token",
                "machineId":"{}",
                "apiRegion":"us-east-1"
            }}"#,
        "a".repeat(64)
    );
    route.cached_status = None;
    route.cached_remaining_credits = None;
    route.cached_balance = None;
    route.cached_cache = None;
    let route_store = Arc::new(CapturingKiroStatusRouteStore {
        route: Arc::new(Mutex::new(route)),
        updates: Arc::new(Mutex::new(Vec::new())),
    });
    let state = super::ProviderState::new(Arc::new(TestStore), route_store.clone());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let updates = route_store.updates.lock().expect("updates");
    assert!(updates.is_empty());
    let requests = captured.requests.lock().expect("captured requests");
    let paths = requests
        .iter()
        .map(|request| request.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["/generateAssistantResponse"]);
}

#[tokio::test]
async fn kiro_dispatch_applies_key_model_mapping_before_upstream_conversion() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let mut route = static_kiro_route();
    route.model_name_map_json = r#"{"claude-haiku-4-5-20251001":"claude-sonnet-4-6"}"#.to_string();
    let state = super::ProviderState::new(
        Arc::new(TestStore),
        Arc::new(StaticRouteStore {
            codex_route: ProviderCodexRoute {
                account_name: "codex-a".to_string(),
                account_group_id_at_event: None,
                route_strategy_at_event: RouteStrategy::Auto,
                auth_json: r#"{"access_token":"upstream-token"}"#.to_string(),
                map_gpt53_codex_to_spark: true,
                auth_refresh_enabled: true,
                codex_fast_enabled: true,
                codex_strict_session_rejection_enabled: false,
                codex_image_generation_enabled: true,
                codex_image_direct_generation_enabled: false,
                request_max_concurrency: None,
                request_min_start_interval_ms: None,
                account_request_max_concurrency: None,
                account_request_min_start_interval_ms: None,
                account_codex_image_generation_enabled: true,
                account_codex_image_generation_max_concurrency:
                    llm_access_core::store::DEFAULT_CODEX_IMAGE_GENERATION_MAX_CONCURRENCY,
                cached_error_message: None,
                proxy: None,
            },
            kiro_route: route,
        }),
    );
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-haiku-4-5-20251001",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["conversationState"]["currentMessage"]["userInputMessage"]["modelId"],
        "claude-sonnet-4.6"
    );
}

#[tokio::test]
async fn kiro_dispatch_sends_history_images_without_stable_session_upstream() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "model": "claude-sonnet-4-6",
                    "max_tokens": 128,
                    "messages": [
                        {
                            "role": "user",
                            "content": [
                                {
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": "image/png",
                                        "data": SAMPLE_PNG_BASE64
                                    }
                                },
                                {"type": "text", "text": "old image"}
                            ]
                        },
                        {"role": "assistant", "content": "ok"},
                        {"role": "user", "content": "continue"}
                    ],
                    "stream": false
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    let history_user = requests[0].body["conversationState"]["history"]
        .as_array()
        .and_then(|history| {
            history.iter().find_map(|message| {
                let user = message.get("userInputMessage")?;
                (user.get("content") == Some(&serde_json::json!("old image"))).then_some(user)
            })
        })
        .expect("history image user message should be present");
    assert_eq!(history_user["content"], "old image");
    assert_eq!(history_user["images"][0]["format"], "png");
    assert_eq!(history_user["origin"], "AI_EDITOR");
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 200);
    assert_eq!(events[0].endpoint, "/v1/messages");
    assert_eq!(events[0].request_url, "/api/kiro-gateway/v1/messages");
}

#[tokio::test]
async fn kiro_dispatch_rejects_more_than_five_documents_before_upstream() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/api/kiro-gateway/v1/messages")
                .header("x-api-key", "valid-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{
                            "role": "user",
                            "content": [
                                {"type": "document", "name": "doc-1.txt", "source": {"type": "text", "media_type": "text/plain", "data": "one"}},
                                {"type": "document", "name": "doc-2.txt", "source": {"type": "text", "media_type": "text/plain", "data": "two"}},
                                {"type": "document", "name": "doc-3.txt", "source": {"type": "text", "media_type": "text/plain", "data": "three"}},
                                {"type": "document", "name": "doc-4.txt", "source": {"type": "text", "media_type": "text/plain", "data": "four"}},
                                {"type": "document", "name": "doc-5.txt", "source": {"type": "text", "media_type": "text/plain", "data": "five"}},
                                {"type": "document", "name": "doc-6.txt", "source": {"type": "text", "media_type": "text/plain", "data": "six"}},
                                {"type": "text", "text": "Summarize these documents."}
                            ]
                        }],
                        "stream": false
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("Too many documents attached"));
    assert!(captured
        .requests
        .lock()
        .expect("captured requests")
        .is_empty());
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 400);
}

#[tokio::test]
async fn kiro_dispatch_keeps_only_the_last_ten_images_before_upstream() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let images = (0..11)
        .map(|index| {
            serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": distinct_sample_png_base64(index)
                }
            })
        })
        .collect::<Vec<_>>();
    let mut content = images;
    content.push(serde_json::json!({
        "type": "text",
        "text": "Describe these images."
    }));
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "model": "claude-sonnet-4-6",
                    "max_tokens": 128,
                    "messages": [{
                        "role": "user",
                        "content": content
                    }],
                    "stream": false
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    let images = requests[0].body["conversationState"]["currentMessage"]["userInputMessage"]
        ["images"]
        .as_array()
        .expect("images array");
    assert_eq!(images.len(), 10);
    assert_eq!(images[0]["source"]["bytes"], distinct_sample_png_base64(1));
    assert_eq!(images[9]["source"]["bytes"], distinct_sample_png_base64(10));
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 200);
}

#[tokio::test]
async fn kiro_dispatch_sends_opus_images_directly_without_vision_bridge() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "model": "claude-opus-4-7",
                    "max_tokens": 128,
                    "messages": [{
                        "role": "user",
                        "content": [
                            {"type": "text", "text": "What is in this image?"},
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": SAMPLE_PNG_BASE64
                                }
                            }
                        ]
                    }],
                    "stream": false
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    let current = &requests[0].body["conversationState"]["currentMessage"]["userInputMessage"];
    assert_eq!(current["modelId"], "claude-opus-4.7");
    assert_eq!(current["content"], "What is in this image?");
    assert_eq!(current["images"][0]["format"], "png");
    assert_eq!(current["origin"], "AI_EDITOR");
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 200);
}

#[tokio::test]
async fn kiro_dispatch_keeps_thinking_model_tags_on_current_turn_only() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "model": "claude-opus-4-7-thinking",
                    "max_tokens": 128,
                    "system": "You are Claude Code, Anthropic's official CLI for Claude.",
                    "messages": [{
                        "role": "user",
                        "content": "Hello"
                    }],
                    "stream": false
                })
                .to_string(),
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 1);
    let state = &requests[0].body["conversationState"];
    assert!(state.get("agentContinuationId").is_none());
    assert!(state.get("agentTaskType").is_none());
    let current = &state["currentMessage"]["userInputMessage"];
    assert_eq!(current["modelId"], "claude-opus-4.7");
    let current_content = current["content"].as_str().expect("current content");
    assert!(current_content.contains("<thinking_mode>adaptive</thinking_mode>"));
    assert!(current_content.contains("<thinking_effort>xhigh</thinking_effort>"));
    assert!(current_content.contains("Hello"));
    let system_prefix = state["history"][0]["userInputMessage"]["content"]
        .as_str()
        .expect("system prefix");
    assert!(!system_prefix.contains("<thinking_effort>xhigh</thinking_effort>"));
    assert!(!system_prefix.contains("claude-opus-4-7-thinking"));
    assert!(system_prefix.contains("You are Claude Code, Anthropic's official CLI for Claude."));
    assert!(!system_prefix.contains("You are powered by the model named Opus 4.7"));
    assert!(!system_prefix.contains("identity override"));
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 200);
}

#[tokio::test]
async fn kiro_dispatch_returns_json_service_unavailable_when_no_route_exists() {
    let state = super::ProviderState::new(Arc::new(TestStore), empty_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}]
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    assert_provider_neutral_json_error(
        response,
        StatusCode::SERVICE_UNAVAILABLE,
        "api_error",
        "Service unavailable.",
    )
    .await;
}

#[tokio::test]
async fn kiro_dispatch_records_route_cooldown_usage_with_aws_bedrock_label() {
    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    state.kiro_request_scheduler.mark_account_cooldown(
        "kiro-a",
        Duration::from_secs(30),
        "upstream rate limited",
    );
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                    "model": "claude-sonnet-4-6",
                    "max_tokens": 128,
                    "messages": [{"role": "user", "content": "hello"}]
                }"#,
            ))
            .expect("request"),
    )
    .await;

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let raw = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(raw.contains("AWS Bedrock"));
    assert!(!raw.to_ascii_lowercase().contains("kiro"));

    wait_for_usage_event_count(&store, 1).await;
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events[0].status_code, 429);
    assert_eq!(
        events[0].error_message.as_deref(),
        Some("all eligible AWS Bedrock accounts are cooling down")
    );
    assert_eq!(events[0].error_class.as_deref(), Some("aws_bedrock_all_accounts_cooling_down"));
}

#[tokio::test]
async fn kiro_dispatch_maps_upstream_content_length_errors_to_prompt_too_long() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_content_length_error_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let oversized_text = "a".repeat(2 * 1024 * 1024);
    let body = serde_json::json!({
        "model": "claude-sonnet-4-6",
        "max_tokens": 128,
        "messages": [{"role": "user", "content": oversized_text}],
        "stream": false
    });
    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    let raw = assert_provider_neutral_json_error(
        response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "invalid_request_error",
        "Prompt is too long: 1000001 tokens > 1000000 tokens for the model context window.",
    )
    .await;
    assert!(!raw.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD"));
    assert!(captured.requests.lock().expect("captured requests").len() == 1);
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 413);
}

#[test]
fn kiro_overlength_detection_matches_generic_input_too_long_message() {
    assert!(super::kiro_text_is_content_length_exceeded("Input is too long."));
}

#[tokio::test]
async fn kiro_dispatch_maps_stream_upstream_content_length_errors_to_json_prompt_too_long() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_content_length_error_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_provider_neutral_json_error(
        response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "invalid_request_error",
        "Prompt is too long: 1000001 tokens > 1000000 tokens for the model context window.",
    )
    .await;
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 413);
}

#[tokio::test]
async fn kiro_dispatch_maps_non_stream_content_length_exception_to_prompt_too_long() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base =
        spawn_fake_kiro_content_length_exception_eventstream_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_provider_neutral_json_error(
        response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "invalid_request_error",
        "Prompt is too long: 1000001 tokens > 1000000 tokens for the model context window.",
    )
    .await;
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 413);
}

#[tokio::test]
async fn kiro_dispatch_maps_stream_content_length_exception_to_json_prompt_too_long() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base =
        spawn_fake_kiro_content_length_exception_eventstream_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_provider_neutral_json_error(
        response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "invalid_request_error",
        "Prompt is too long: 1000001 tokens > 1000000 tokens for the model context window.",
    )
    .await;
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 413);
}

#[tokio::test]
async fn kiro_dispatch_buffers_split_stream_content_length_exception_before_sse() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base =
        spawn_fake_kiro_split_content_length_exception_eventstream_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_provider_neutral_json_error(
        response,
        StatusCode::PAYLOAD_TOO_LARGE,
        "invalid_request_error",
        "Prompt is too long: 1000001 tokens > 1000000 tokens for the model context window.",
    )
    .await;
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 413);
}

#[tokio::test]
async fn kiro_dispatch_streams_messages_from_eventstream() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("event: message_start"));
    assert!(body.contains("hello "));
    assert!(body.contains("back"));
    let message_delta = body
        .split("\n\n")
        .find(|frame| frame.starts_with("event: message_delta"))
        .expect("message_delta frame");
    assert!(message_delta.contains(r#""input_tokens":1"#));
    assert!(message_delta.contains(r#""output_tokens":3"#));
    assert!(message_delta.contains(r#""cache_creation_input_tokens":0"#));
    assert!(message_delta.contains(r#""cache_read_input_tokens":0"#));
    assert!(body.contains("event: message_stop"));
}

#[tokio::test]
async fn kiro_dispatch_retries_empty_stream_before_sending_sse() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_empty_once_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("hello "));
    assert!(body.contains("back"));
    assert!(body.contains("event: message_stop"));

    let requests = captured.requests.lock().expect("captured requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/generateAssistantResponse");
    assert_eq!(requests[1].path, "/generateAssistantResponse");
}

#[tokio::test]
async fn kiro_dispatch_fails_over_after_empty_stream_retries_exhausted() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_empty_route_then_success_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let route_store = Arc::new(StaticMultiKiroRouteStore {
        codex_route: codex_route_for_account("codex-a", "upstream-token"),
        kiro_routes: vec![
            kiro_route_for_account("kiro-empty", "kiro-empty-token"),
            kiro_route_for_account("kiro-success", "kiro-success-token"),
        ],
    });
    let state = super::ProviderState::new(store.clone(), route_store);
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-opus-4-8",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("hello "));
    assert!(body.contains("back"));
    assert!(body.contains("event: message_stop"));

    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec![
        "Bearer kiro-empty-token".to_string(),
        "Bearer kiro-empty-token".to_string(),
        "Bearer kiro-empty-token".to_string(),
        "Bearer kiro-success-token".to_string(),
    ]);

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status_code, 200);
    assert_eq!(events[0].endpoint, "/v1/messages");
}

#[tokio::test]
async fn kiro_dispatch_rejects_mixed_cctest_route_candidates() {
    let mut first = kiro_route_for_account("kiro-a", "kiro-a-token");
    first.cctest_text_handling_enabled = false;
    let mut second = kiro_route_for_account("kiro-b", "kiro-b-token");
    second.cctest_text_handling_enabled = true;

    let state = super::ProviderState::new(
        Arc::new(TestStore),
        Arc::new(StaticMultiKiroRouteStore {
            codex_route: codex_route_for_account("codex-a", "upstream-token"),
            kiro_routes: vec![first, second],
        }),
    );

    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-opus-4-8",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    assert_provider_neutral_json_error(
        response,
        StatusCode::INTERNAL_SERVER_ERROR,
        "api_error",
        "Internal server error.",
    )
    .await;
}

#[tokio::test]
async fn kiro_dispatch_hides_provider_details_when_empty_stream_retries_exhausted() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_empty_route_then_success_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(
        Arc::new(TestStore),
        Arc::new(StaticRouteStore {
            codex_route: codex_route_for_account("codex-a", "upstream-token"),
            kiro_route: kiro_route_for_account("kiro-empty", "kiro-empty-token"),
        }),
    );
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-opus-4-8",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_provider_neutral_json_error(
        response,
        StatusCode::BAD_GATEWAY,
        "api_error",
        "Upstream service unavailable.",
    )
    .await;
}

#[tokio::test]
async fn kiro_dispatch_streams_cc_messages_without_buffering_special_case() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/cc/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    let body = String::from_utf8(body.to_vec()).expect("utf8 response");
    assert!(body.contains("event: message_start"));
    assert!(body.contains(r#""input_tokens":1"#));
    assert!(body.contains("hello "));
    assert!(body.contains("back"));
}

#[tokio::test]
async fn kiro_dispatch_records_usage_rollup_from_eventstream() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base = spawn_fake_kiro_upstream(captured).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.provider_type, llm_access_core::provider::ProviderType::Kiro);
    assert_eq!(event.protocol_family, llm_access_core::provider::ProtocolFamily::Anthropic);
    assert_eq!(event.key_id, "key-kiro-usage");
    assert_eq!(event.account_name.as_deref(), Some("kiro-a"));
    assert_eq!(event.endpoint, "/v1/messages");
    assert_eq!(event.model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(event.input_uncached_tokens, 1);
    assert_eq!(event.input_cached_tokens, 0);
    assert_eq!(event.output_tokens, 3);
    assert_eq!(event.billable_tokens, 16);
    assert_eq!(event.credit_usage.as_deref(), Some("0.25"));
    assert!(!event.credit_usage_missing);
    assert_eq!(event.request_method, "POST");
    assert_eq!(event.request_url, "/api/kiro-gateway/v1/messages");
    assert!(event.request_body_bytes.unwrap_or_default() > 0);
    assert!(event.timing.request_body_read_ms.is_some());
    assert!(event.timing.request_json_parse_ms.is_some());
    assert!(event.timing.pre_handler_ms.is_some());
    assert!(event.timing.routing_wait_ms.is_some());
    assert!(event.timing.upstream_headers_ms.is_some());
    assert!(event.timing.post_headers_body_ms.is_some());
    assert!(event.timing.stream_finish_ms.is_some());
    assert_eq!(event.last_message_content.as_deref(), Some("hello"));
}

#[tokio::test]
async fn kiro_dispatch_rate_limit_marks_cooldown_and_fails_over_without_same_account_retry() {
    let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
        .lock()
        .expect("kiro upstream env lock");
    let captured = Arc::new(CapturedKiroUpstream::default());
    let upstream_base =
        spawn_fake_kiro_rate_limit_route_then_success_upstream(captured.clone()).await;
    std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

    let store = Arc::new(RecordingControlStore::default());
    let state = super::ProviderState::new(
        store.clone(),
        Arc::new(StaticMultiKiroRouteStore {
            codex_route: codex_route_for_account("codex-a", "upstream-token"),
            kiro_routes: vec![
                kiro_route_for_account("kiro-limited", "kiro-rate-limited-token"),
                kiro_route_for_account("kiro-success", "kiro-success-token"),
            ],
        }),
    );
    let response = super::provider_entry(
        state,
        Request::builder()
            .method("POST")
            .uri("/api/kiro-gateway/v1/messages")
            .header("x-api-key", "valid-secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
            ))
            .expect("request"),
    )
    .await;

    std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

    assert_eq!(response.status(), StatusCode::OK);
    let requests = captured.requests.lock().expect("captured requests");
    let auths = requests
        .iter()
        .filter(|request| request.path == "/generateAssistantResponse")
        .filter_map(|request| request.authorization.clone())
        .collect::<Vec<_>>();
    assert_eq!(auths, vec![
        "Bearer kiro-rate-limited-token".to_string(),
        "Bearer kiro-success-token".to_string(),
    ]);
    drop(requests);

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.account_name.as_deref(), Some("kiro-success"));
    assert_eq!(event.quota_failover_count, 1);
    assert_eq!(event.retry.same_account_retry_count, 0);
    assert_eq!(event.retry.same_account_retry_delay_ms, 0);
    assert!(event.retry.same_account_retry_reasons.is_empty());
}

#[test]
fn kiro_billable_tokens_discounts_cached_input_like_legacy_gateway() {
    let usage = super::KiroUsageSummary {
        input_uncached_tokens: 100,
        input_cached_tokens: 1_000,
        output_tokens: 4,
        credit_usage: None,
        credit_usage_missing: false,
    };
    let multipliers = BTreeMap::from([("sonnet".to_string(), 2.0)]);

    let billable =
        super::kiro_billable_tokens_with_multipliers("claude-sonnet-4-6", usage, &multipliers);

    assert_eq!(billable, (100 + 1_000 / 10 + 4 * 5) * 2);
}

#[tokio::test]
async fn kiro_websearch_usage_omits_heavy_payload_on_success() {
    let store = RecordingControlStore::default();
    let key = AuthenticatedKey {
        key_id: "kiro-key".to_string(),
        key_name: "Kiro key".to_string(),
        provider_type: "kiro".to_string(),
        protocol_family: "anthropic".to_string(),
        status: "active".to_string(),
        quota_billable_limit: 1_000,
        billable_tokens_used: 0,
    };
    let meta = super::ProviderUsageMetadata {
        started_at: Instant::now(),
        request_method: "POST".to_string(),
        request_url: "/api/kiro-gateway/v1/messages".to_string(),
        request_body_bytes: Some(128),
        request_body_read_ms: None,
        request_json_parse_ms: None,
        pre_handler_ms: None,
        routing_wait_ms: None,
        upstream_headers_ms: None,
        post_headers_body_ms: None,
        first_sse_write_ms: None,
        stream_finish_ms: None,
        stream_completed_cleanly: None,
        downstream_disconnect: None,
        final_event_type: None,
        bytes_streamed: None,
        quota_failover_count: 0,
        retry: Default::default(),
        routing_diagnostics_json: None,
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        request_headers_json: "{}".to_string(),
        last_message_content: Some("search query".to_string()),
        client_request_body_json: Some(captured_json_bytes(r#"{"client":true}"#)),
        upstream_request_body_json: Some(captured_json_bytes(r#"{"mcp":true}"#)),
        full_request_json: Some(captured_json_bytes(r#"{"full":true}"#)),
        error_message: Some(
            "400 AWS Bedrock error message: A text block must be included when using documents."
                .to_string(),
        ),
        error_body: Some(
            r#"{"error":{"message":"A text block must be included when using documents."}}"#
                .to_string(),
        ),
        error_class: None,
        session_blocked: false,
        response_body: None,
    };

    let route = static_kiro_route();
    super::record_kiro_websearch_usage(super::KiroWebsearchUsageRecord {
        control_store: &store,
        key: &key,
        route: &route,
        model: "claude-sonnet-4-6",
        status: StatusCode::OK,
        usage: super::KiroUsageSummary {
            input_uncached_tokens: 10,
            input_cached_tokens: 0,
            output_tokens: 3,
            credit_usage: None,
            credit_usage_missing: true,
        },
        meta: &meta,
        capture_request_details: false,
    })
    .await
    .expect("record websearch usage");

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].endpoint, "/mcp");
    assert_eq!(events[0].last_message_content.as_deref(), Some("search query"));
    assert_eq!(events[0].client_request_body_json, None);
    assert_eq!(events[0].upstream_request_body_json, None);
    assert_eq!(events[0].full_request_json, None);
}

#[tokio::test]
async fn kiro_websearch_usage_captures_heavy_payload_on_error_by_default() {
    let store = RecordingControlStore::default();
    let key = AuthenticatedKey {
        key_id: "kiro-key".to_string(),
        key_name: "Kiro key".to_string(),
        provider_type: "kiro".to_string(),
        protocol_family: "anthropic".to_string(),
        status: "active".to_string(),
        quota_billable_limit: 1_000,
        billable_tokens_used: 0,
    };
    let meta = super::ProviderUsageMetadata {
        started_at: Instant::now(),
        request_method: "POST".to_string(),
        request_url: "/api/kiro-gateway/v1/messages".to_string(),
        request_body_bytes: Some(128),
        request_body_read_ms: None,
        request_json_parse_ms: None,
        pre_handler_ms: None,
        routing_wait_ms: None,
        upstream_headers_ms: None,
        post_headers_body_ms: None,
        first_sse_write_ms: None,
        stream_finish_ms: None,
        stream_completed_cleanly: None,
        downstream_disconnect: None,
        final_event_type: None,
        bytes_streamed: None,
        quota_failover_count: 0,
        retry: Default::default(),
        routing_diagnostics_json: None,
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        request_headers_json: "{}".to_string(),
        last_message_content: Some("search query".to_string()),
        client_request_body_json: Some(captured_json_bytes(r#"{"client":true}"#)),
        upstream_request_body_json: Some(captured_json_bytes(r#"{"mcp":true}"#)),
        full_request_json: Some(captured_json_bytes(r#"{"full":true}"#)),
        error_message: Some(
            "400 AWS Bedrock error message: A text block must be included when using documents."
                .to_string(),
        ),
        error_body: Some(
            r#"{"error":{"message":"A text block must be included when using documents."}}"#
                .to_string(),
        ),
        error_class: None,
        session_blocked: false,
        response_body: None,
    };

    let route = static_kiro_route();
    super::record_kiro_websearch_usage(super::KiroWebsearchUsageRecord {
        control_store: &store,
        key: &key,
        route: &route,
        model: "claude-sonnet-4-6",
        status: StatusCode::BAD_GATEWAY,
        usage: super::KiroUsageSummary {
            input_uncached_tokens: 10,
            input_cached_tokens: 0,
            output_tokens: 3,
            credit_usage: None,
            credit_usage_missing: true,
        },
        meta: &meta,
        capture_request_details: false,
    })
    .await
    .expect("record websearch error usage");

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].endpoint, "/mcp");
    assert_eq!(events[0].status_code, 502);
    assert_eq!(events[0].client_request_body_json.as_deref(), Some(r#"{"client":true}"#));
    assert_eq!(events[0].upstream_request_body_json.as_deref(), Some(r#"{"mcp":true}"#));
    assert_eq!(events[0].full_request_json.as_deref(), Some(r#"{"full":true}"#));
    assert_eq!(
        events[0].error_message.as_deref(),
        Some("400 AWS Bedrock error message: A text block must be included when using documents.")
    );
    assert_eq!(
        events[0].error_body.as_deref(),
        Some(r#"{"error":{"message":"A text block must be included when using documents."}}"#)
    );
}

#[tokio::test]
async fn kiro_usage_captures_full_payload_when_key_full_request_logging_enabled() {
    let store = RecordingControlStore::default();
    let key = AuthenticatedKey {
        key_id: "kiro-key".to_string(),
        key_name: "Kiro key".to_string(),
        provider_type: "kiro".to_string(),
        protocol_family: "anthropic".to_string(),
        status: "active".to_string(),
        quota_billable_limit: 1_000,
        billable_tokens_used: 0,
    };
    let mut route = static_kiro_route();
    route.full_request_logging_enabled = true;
    let conversation_state =
        llm_access_kiro::wire::ConversationState::new("diag-conversation".to_string());
    let cache_simulator = llm_access_kiro::cache_sim::KiroCacheSimulator::default();
    let cache_ctx = super::build_kiro_cache_context(&route, &conversation_state, &cache_simulator)
        .expect("cache context");
    let meta = super::ProviderUsageMetadata {
        started_at: Instant::now(),
        request_method: "POST".to_string(),
        request_url: "/api/kiro-gateway/v1/messages".to_string(),
        request_body_bytes: Some(128),
        request_body_read_ms: None,
        request_json_parse_ms: None,
        pre_handler_ms: None,
        routing_wait_ms: None,
        upstream_headers_ms: None,
        post_headers_body_ms: None,
        first_sse_write_ms: None,
        stream_finish_ms: None,
        stream_completed_cleanly: None,
        downstream_disconnect: None,
        final_event_type: None,
        bytes_streamed: None,
        quota_failover_count: 0,
        retry: Default::default(),
        routing_diagnostics_json: None,
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        request_headers_json: "{}".to_string(),
        last_message_content: Some("normal cached request".to_string()),
        client_request_body_json: Some(captured_json_bytes(r#"{"client":true}"#)),
        upstream_request_body_json: Some(captured_json_bytes(r#"{"upstream":true}"#)),
        full_request_json: Some(captured_json_bytes(r#"{"full":true}"#)),
        error_message: None,
        error_body: None,
        error_class: None,
        session_blocked: false,
        response_body: None,
    };

    super::record_kiro_usage(super::KiroUsageRecord {
        control_store: &store,
        key: &key,
        route: &route,
        endpoint: "/v1/messages",
        model: "claude-sonnet-4-6",
        status: StatusCode::OK,
        usage: super::KiroUsageSummary {
            input_uncached_tokens: 10,
            input_cached_tokens: 200,
            output_tokens: 3,
            credit_usage: None,
            credit_usage_missing: true,
        },
        cache_ctx: &cache_ctx,
        meta: &meta,
    })
    .await
    .expect("record kiro usage");

    let events = store.usage_events.lock().expect("usage events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].client_request_body_json.as_deref(), Some(r#"{"client":true}"#));
    assert_eq!(events[0].upstream_request_body_json.as_deref(), Some(r#"{"upstream":true}"#));
    assert_eq!(events[0].full_request_json.as_deref(), Some(r#"{"full":true}"#));
}

#[test]
fn provider_usage_metadata_tracks_stream_outcome_fields() {
    let mut meta = super::ProviderUsageMetadata {
        started_at: Instant::now(),
        request_method: "POST".to_string(),
        request_url: "/v1/messages".to_string(),
        request_body_bytes: Some(64),
        request_body_read_ms: Some(1),
        request_json_parse_ms: Some(1),
        pre_handler_ms: Some(2),
        routing_wait_ms: Some(3),
        upstream_headers_ms: Some(4),
        post_headers_body_ms: Some(5),
        first_sse_write_ms: None,
        stream_finish_ms: None,
        stream_completed_cleanly: None,
        downstream_disconnect: None,
        final_event_type: None,
        bytes_streamed: None,
        quota_failover_count: 0,
        retry: Default::default(),
        routing_diagnostics_json: None,
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        request_headers_json: "{}".to_string(),
        last_message_content: None,
        client_request_body_json: None,
        upstream_request_body_json: None,
        full_request_json: None,
        error_message: None,
        error_body: None,
        error_class: None,
        session_blocked: false,
        response_body: None,
    };

    meta.observe_stream_write(12, Some("message_start"));
    meta.observe_stream_write(8, Some("message_stop"));
    meta.mark_stream_completed_cleanly();

    assert_eq!(meta.to_stream_details(), UsageStreamDetails {
        stream_completed_cleanly: Some(true),
        downstream_disconnect: Some(false),
        final_event_type: Some("message_stop".to_string()),
        bytes_streamed: Some(20),
    });
    assert!(meta.to_timing().stream_finish_ms.is_some());
}

#[tokio::test]
async fn provider_entry_rejects_kiro_key_on_codex_route_before_dispatch() {
    let dispatcher = Arc::new(CapturingDispatcher::default());
    let state = test_state_with_dispatcher(dispatcher.clone());

    let response = super::provider_entry(
        state,
        request_with_bearer_to_path("/v1/responses", Some("Bearer valid-secret")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(dispatcher.seen.lock().expect("dispatcher state").is_empty());
}

#[tokio::test]
async fn provider_entry_rejects_codex_key_on_kiro_route_before_dispatch() {
    let dispatcher = Arc::new(CapturingDispatcher::default());
    let state = test_state_with_dispatcher(dispatcher.clone());

    let response = super::provider_entry(
        state,
        request_with_bearer_to_path("/api/kiro-gateway/v1/messages", Some("Bearer codex-secret")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(dispatcher.seen.lock().expect("dispatcher state").is_empty());
}

#[tokio::test]
async fn provider_entry_rejects_exhausted_kiro_key_before_dispatch() {
    let dispatcher = Arc::new(CapturingDispatcher::default());
    let state = test_state_with_dispatcher(dispatcher.clone());

    let response = super::provider_entry(
        state,
        request_with_bearer_to_path(
            "/api/kiro-gateway/v1/messages",
            Some("Bearer exhausted-kiro-secret"),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    assert!(dispatcher.seen.lock().expect("dispatcher state").is_empty());
}

#[tokio::test]
async fn provider_entry_rejects_exhausted_codex_key_before_dispatch() {
    let dispatcher = Arc::new(CapturingDispatcher::default());
    let state = test_state_with_dispatcher(dispatcher.clone());

    let response = super::provider_entry(
        state,
        request_with_bearer_to_path("/v1/responses", Some("Bearer exhausted-codex-secret")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(dispatcher.seen.lock().expect("dispatcher state").is_empty());
}

#[tokio::test]
async fn provider_entry_dispatches_authenticated_active_requests() {
    let dispatcher = Arc::new(CapturingDispatcher::default());
    let state = test_state_with_dispatcher(dispatcher.clone());

    let response =
        super::provider_entry(state, request_with_bearer(Some("Bearer valid-secret"))).await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(dispatcher.seen.lock().expect("dispatcher state").as_slice(), &[(
        "key-1".to_string(),
        "/api/kiro-gateway/v1/messages".to_string()
    )]);
}

#[tokio::test]
async fn provider_entry_dispatches_codex_key_on_codex_routes() {
    let dispatcher = Arc::new(CapturingDispatcher::default());
    let state = test_state_with_dispatcher(dispatcher.clone());

    let response = super::provider_entry(
        state,
        request_with_bearer_to_path("/api/codex-gateway/v1/responses", Some("Bearer codex-secret")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(dispatcher.seen.lock().expect("dispatcher state").as_slice(), &[(
        "key-2".to_string(),
        "/api/codex-gateway/v1/responses".to_string()
    )]);
}

#[tokio::test]
async fn provider_entry_requires_codex_route_for_models_after_auth() {
    let state = test_state();
    let request = Request::builder()
        .method("GET")
        .uri("/api/llm-gateway/v1/models")
        .header(header::AUTHORIZATION, "Bearer codex-secret")
        .body(Body::empty())
        .expect("request");

    let response = super::provider_entry(state, request).await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
