//! Real HTTP fixtures for gateway regressions and repeatable latency measurements.
use super::{
    handlers::{post_messages, post_messages_cc},
    middleware::{AppState, KeyContext},
    openai::post_chat_completions,
    responses::post_responses,
};
use crate::{
    admin::trace_db::{TraceKeySource, TraceQuery, TraceStore},
    kiro::{
        endpoint::{KiroEndpoint, RequestContext},
        model::credentials::KiroCredentials,
        parser::crc::crc32,
        provider::KiroProvider,
        token_manager::MultiTokenManager,
    },
    model::config::{Config, ToolCompatibilityMode},
};
use axum::{
    Extension, Json, Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use bytes::Bytes;
use futures::{StreamExt, stream};
use parking_lot::Mutex;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
enum Mode {
    Normal(Duration),
    Interrupted,
    Overloaded,
    Large,
    Truncated,
    InvalidFrame,
    Empty,
    Tools,
    Stalled,
    SlowHeaders,
    SlowErrorBody,
    MetadataFirst,
    Active,
    OutputLimit,
    NativeReasoning,
    NativeReasoningOnly,
    NativeReasoningTools,
    /// 上游内容过滤拦截：只有 reasoning，`metadataEvent.stopReason = CONTENT_FILTERED`，
    /// 随后 contextUsage 100%（验证拒答不被上下文溢出覆盖）。帧序列取自线上抓包。
    Refused,
    RefusedWithSearch,
    RefusedWithPartialTool,
    RefusedWithInvalidTool,
    RefusedWithEmptyTool,
    RefusedBare,
}

struct Fixture {
    manager: Arc<MultiTokenManager>,
    provider: Arc<KiroProvider>,
    state: AppState,
    calls: Arc<AtomicUsize>,
    connections: Arc<Mutex<HashSet<SocketAddr>>>,
    close_headers: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct LocalEndpoint(String);
impl KiroEndpoint for LocalEndpoint {
    fn name(&self) -> &'static str {
        "fixture"
    }
    fn api_url(&self, _: &RequestContext<'_>) -> String {
        self.0.clone()
    }
    fn mcp_url(&self, _: &RequestContext<'_>) -> String {
        self.0.clone()
    }
    fn decorate_api(
        &self,
        req: reqwest::RequestBuilder,
        _: &RequestContext<'_>,
    ) -> reqwest::RequestBuilder {
        req
    }
    fn decorate_mcp(
        &self,
        req: reqwest::RequestBuilder,
        _: &RequestContext<'_>,
    ) -> reqwest::RequestBuilder {
        req
    }
    fn transform_api_body(&self, body: &str, _: &RequestContext<'_>) -> String {
        body.to_owned()
    }
}

fn frame(event: &str, payload: serde_json::Value) -> Bytes {
    let mut headers = Vec::new();
    for (name, value) in [(":message-type", "event"), (":event-type", event)] {
        headers.push(name.len() as u8);
        headers.extend_from_slice(name.as_bytes());
        headers.push(7);
        headers.extend_from_slice(&(value.len() as u16).to_be_bytes());
        headers.extend_from_slice(value.as_bytes());
    }
    let payload = serde_json::to_vec(&payload).unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(&((16 + headers.len() + payload.len()) as u32).to_be_bytes());
    out.extend_from_slice(&(headers.len() as u32).to_be_bytes());
    out.extend_from_slice(&crc32(&out).to_be_bytes());
    out.extend(headers);
    out.extend(payload);
    out.extend_from_slice(&crc32(&out).to_be_bytes());
    Bytes::from(out)
}

async fn fixture(mode: Mode) -> Fixture {
    fixture_with_config(mode, Config::default()).await
}

async fn fixture_with_config(mode: Mode, config: Config) -> Fixture {
    let calls = Arc::new(AtomicUsize::new(0));
    let connections = Arc::new(Mutex::new(HashSet::new()));
    let close_headers = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen_requests = requests.clone();
    let (seen_calls, seen_connections, seen_close) =
        (calls.clone(), connections.clone(), close_headers.clone());
    let app = Router::new().route("/", post(move |ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<serde_json::Value>| {
        let (calls, connections, close) = (seen_calls.clone(), seen_connections.clone(), seen_close.clone());
        let requests = seen_requests.clone();
        async move {
            requests.lock().push(body);
            calls.fetch_add(1, Ordering::Relaxed);
            connections.lock().insert(peer);
            if headers.get("connection").is_some_and(|v| v == "close") { close.fetch_add(1, Ordering::Relaxed); }
            if matches!(mode, Mode::Overloaded) {
                return Response::builder().status(500).body(Body::from(r#"{"message":"high load","reason":"MODEL_TEMPORARILY_UNAVAILABLE"}"#)).unwrap();
            }
            if matches!(mode, Mode::SlowHeaders) { tokio::time::sleep(Duration::from_secs(4)).await; }
            if matches!(mode, Mode::SlowErrorBody) {
                return Response::builder().status(500).body(Body::from_stream(stream::pending::<Result<Bytes, Infallible>>())).unwrap();
            }
            if matches!(mode, Mode::Empty) { return Response::new(Body::empty()); }
            if matches!(mode, Mode::Large) {
                let frames = (0..18000).map(|_| frame("assistantResponseEvent", json!({"content": "x".repeat(1024)})));
                return Response::new(Body::from_stream(stream::iter(frames.map(Ok::<_, Infallible>))));
            }
            if matches!(mode, Mode::Truncated | Mode::InvalidFrame) {
                let mut bytes = frame("assistantResponseEvent", json!({"content": "damaged"})).to_vec();
                if matches!(mode, Mode::Truncated) { bytes.truncate(bytes.len() - 4); }
                else { let last = bytes.len() - 1; bytes[last] ^= 1; }
                return Response::new(Body::from(bytes));
            }
            if matches!(mode, Mode::OutputLimit) {
                let mut bytes = Vec::new();
                for part in [
                    frame("assistantResponseEvent", json!({"content": "unfinished sentence"})),
                    frame("metadataEvent", json!({"stopReason":"MAX_TOKENS"})),
                    // Usage/normal metadata must not erase the earlier truncation reason.
                    frame("metadataEvent", json!({"stopReason":"END_TURN", "tokenUsage":{"outputTokens":1024}})),
                ] { bytes.extend_from_slice(&part); }
                return Response::new(Body::from(bytes));
            }
            if matches!(mode, Mode::NativeReasoning | Mode::NativeReasoningOnly | Mode::NativeReasoningTools) {
                let mut bytes = Vec::new();
                bytes.extend_from_slice(&frame("reasoningContentEvent", json!({"text":"fixture reasoning", "signature":"fixture-signature"})));
                if !matches!(mode, Mode::NativeReasoningOnly) {
                    bytes.extend_from_slice(&frame("assistantResponseEvent", json!({"content":"fixture answer"})));
                }
                if matches!(mode, Mode::NativeReasoningTools) {
                    bytes.extend_from_slice(&frame("toolUseEvent", json!({"name":"run", "toolUseId":"call_1", "input":"{\"command\":\"hello\"}", "stop":true})));
                }
                bytes.extend_from_slice(&frame("metadataEvent", json!({"tokenUsage":{"uncachedInputTokens":13,"outputTokens":17,"cacheReadInputTokens":3,"cacheWriteInputTokens":2}})));
                return Response::new(Body::from(bytes));
            }
            if matches!(mode, Mode::Tools) {
                let mut bytes = Vec::new();
                for part in [
                    frame("toolUseEvent", json!({"name":"run", "toolUseId":"call_1", "input":"{\"command\":", "stop":false})),
                    frame("toolUseEvent", json!({"name":"run", "toolUseId":"call_1", "input":"\"hello\"}", "stop":true})),
                    frame("metadataEvent", json!({"tokenUsage":{"uncachedInputTokens":13,"outputTokens":17,"cacheReadInputTokens":3,"cacheWriteInputTokens":2}})),
                ] { bytes.extend_from_slice(&part); }
                return Response::new(Body::from(bytes));
            }
            if matches!(mode, Mode::Refused | Mode::RefusedWithSearch | Mode::RefusedWithPartialTool | Mode::RefusedWithInvalidTool | Mode::RefusedWithEmptyTool | Mode::RefusedBare) {
                let mut bytes = Vec::new();
                if matches!(mode, Mode::RefusedWithSearch) {
                    bytes.extend_from_slice(&frame("toolUseEvent", json!({"name":"web_search","toolUseId":"search_1","input":"{\"query\":\"example\"}","stop":true})));
                }
                if matches!(mode, Mode::RefusedWithPartialTool) {
                    bytes.extend_from_slice(&frame("toolUseEvent", json!({"name":"run","toolUseId":"run_1","input":"{\"command\":","stop":false})));
                }
                if matches!(mode, Mode::RefusedWithInvalidTool) {
                    bytes.extend_from_slice(&frame("toolUseEvent", json!({"name":"run","toolUseId":"run_1","input":"{\"command\":","stop":true})));
                }
                if matches!(mode, Mode::RefusedWithEmptyTool) {
                    bytes.extend_from_slice(&frame("toolUseEvent", json!({"name":"run","toolUseId":"run_1","input":"","stop":false})));
                }
                if matches!(mode, Mode::RefusedBare) {
                    bytes.extend_from_slice(&frame("metadataEvent", json!({"stopReason":"CONTENT_FILTERED"})));
                    return Response::new(Body::from(bytes));
                }
                for part in [
                    frame("reasoningContentEvent", json!({"text":"The"})),
                    frame("metadataEvent", json!({"stopReason":"CONTENT_FILTERED","stopDetails":{"refusal":{"category":"CYBER","explanation":"The selected model cannot continue this conversation."}}})),
                    frame("contextUsageEvent", json!({"contextUsagePercentage":100.0})),
                ] { bytes.extend_from_slice(&part); }
                return Response::new(Body::from(bytes));
            }
            let first = frame("assistantResponseEvent", json!({"content": "early content ".repeat(64)}));
            let metadata = frame("metadataEvent", json!({"tokenUsage": {"uncachedInputTokens": 13, "outputTokens": 17, "cacheReadInputTokens": 3, "cacheWriteInputTokens": 2}}));
            let chunks = stream::unfold(0, move |step| {
                let first = first.clone(); let metadata = metadata.clone();
                async move {
                    match step {
                        0 => { tokio::time::sleep(Duration::from_millis(20)).await; Some((Ok::<_, std::io::Error>(if matches!(mode, Mode::MetadataFirst) { metadata } else { first }), 1)) }
                        1 if matches!(mode, Mode::MetadataFirst) => { tokio::time::sleep(Duration::from_millis(100)).await; Some((Ok(first), 2)) }
                        1..=5 if matches!(mode, Mode::Active) => { tokio::time::sleep(Duration::from_millis(350)).await; Some((Ok(if step == 5 { metadata } else { first }), step + 1)) }
                        6 if matches!(mode, Mode::Active) => None,
                        1 if matches!(mode, Mode::Interrupted) => { tokio::time::sleep(Duration::from_millis(50)).await; Some((Err(std::io::Error::other("fixture disconnected")), 2)) }
                        1 if matches!(mode, Mode::Stalled) => futures::future::pending().await,
                        1 => { if let Mode::Normal(gap) = mode { tokio::time::sleep(gap).await; } Some((Ok(metadata), 2)) }
                        _ => None,
                    }
                }
            });
            Response::new(Body::from_stream(chunks))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let creds: KiroCredentials = serde_json::from_value(json!({"id": 1, "kiroApiKey": "fixture-only", "machineId": "0000000000000000000000000000000000000000000000000000000000000000"})).unwrap();
    let manager = Arc::new(MultiTokenManager::new(config, vec![creds], None, None, false).unwrap());
    let endpoints = HashMap::from([(
        "fixture".to_owned(),
        Arc::new(LocalEndpoint(url)) as Arc<dyn KiroEndpoint>,
    )]);
    let provider = Arc::new(KiroProvider::with_proxy(manager.clone(), None, endpoints, "fixture".to_owned()));
    let state = AppState::new(false, ToolCompatibilityMode::Raw)
        .with_shared_kiro_provider(provider.clone());
    Fixture {
        provider,
        manager,
        state,
        calls,
        connections,
        close_headers,
        requests,
        task,
    }
}

fn key() -> KeyContext {
    KeyContext {
        key_id: 0,
        group: None,
        key_source: TraceKeySource::MasterApiKey,
        client_ip: None,
    }
}
async fn request(state: AppState, endpoint: &str, streaming: bool) -> Response {
    let req = json!({"model": "gpt-5.6-sol", "max_tokens": 64, "stream": streaming, "messages": [{"role": "user", "content": "hello"}], "input": [{"role":"user", "content":"hello"}]});
    request_payload(state, endpoint, req).await
}

async fn request_payload(state: AppState, endpoint: &str, req: serde_json::Value) -> Response {
    if endpoint == "chat" {
        post_chat_completions(
            axum::extract::State(state),
            Extension(key()),
            HeaderMap::new(),
            Json(serde_json::from_value(req).unwrap()),
        )
        .await
    } else if endpoint == "responses" {
        post_responses(
            axum::extract::State(state),
            Extension(key()),
            HeaderMap::new(),
            Json(serde_json::from_value(req).unwrap()),
        )
        .await
    } else if endpoint == "cc" {
        post_messages_cc(
            axum::extract::State(state),
            Extension(key()),
            Json(serde_json::from_value(req).unwrap()),
        )
        .await
    } else {
        post_messages(
            axum::extract::State(state),
            Extension(key()),
            Json(serde_json::from_value(req).unwrap()),
        )
        .await
    }
}

async fn first_content(
    body: &mut (impl futures::Stream<Item = Result<Bytes, axum::Error>> + Unpin),
) {
    while let Some(chunk) = body.next().await {
        if String::from_utf8_lossy(&chunk.unwrap()).contains("early content") {
            return;
        }
    }
    panic!("stream ended without content");
}

/// Model-independent output typing must not depend on a request-side thinking flag.
#[tokio::test]
async fn gateway_native_reasoning_stays_separate_across_models_and_protocols() {
    for model in ["claude-opus-5-5", "claude-opus-4-6", "claude-sonnet-4-6", "gpt-5.6-sol"] {
        for endpoint in ["messages", "cc", "chat", "responses"] {
            for streaming in [false, true] {
                for mode in [Mode::NativeReasoning, Mode::NativeReasoningOnly, Mode::NativeReasoningTools] {
                    for search_loop in [false, true] {
                        if search_loop && (model != "claude-opus-5-5" || endpoint == "chat") {
                            continue;
                        }
                        let f = fixture(mode).await;
                        let mut req = json!({
                            "model":model, "stream":streaming,
                            "max_tokens":2048, "max_output_tokens":2048,
                            "messages":[{"role":"user", "content":"hello"}], "input":"hello"
                        });
                        if endpoint == "messages" || endpoint == "cc" {
                            // Explicitly disabled must not downgrade an event the upstream still sends.
                            req["thinking"] = json!({"type":"disabled"});
                        }
                        if search_loop {
                            req["tools"] = match endpoint {
                                "chat" => json!([
                                    {"type":"function", "function":{"name":"web_search", "parameters":{"type":"object"}}},
                                    {"type":"function", "function":{"name":"run", "parameters":{"type":"object"}}}
                                ]),
                                "responses" => json!([
                                    {"type":"web_search"},
                                    {"type":"function", "name":"run", "parameters":{"type":"object"}}
                                ]),
                                _ => json!([
                                    {"type":"web_search_20250305", "name":"web_search"},
                                    {"name":"run", "input_schema":{"type":"object"}}
                                ]),
                            };
                        }
                        let response = request_payload(f.state.clone(), endpoint, req).await;
                        assert_eq!(response.status(), StatusCode::OK, "{model} {endpoint} stream={streaming}");
                        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
                        let expected_text = if matches!(mode, Mode::NativeReasoningOnly) { "" } else { "fixture answer" };
                        let tool_count = usize::from(matches!(mode, Mode::NativeReasoningTools));
                        if streaming {
                            let body = String::from_utf8(body.to_vec()).unwrap();
                            let events: Vec<serde_json::Value> = body.lines()
                                .filter_map(|l| l.strip_prefix("data: "))
                                .filter(|v| *v != "[DONE]")
                                .map(|v| serde_json::from_str(v).unwrap()).collect();
                            let (text, reasoning, tools, output_tokens) = match endpoint {
                                "chat" => (
                                    events.iter().filter_map(|v| v.pointer("/choices/0/delta/content").and_then(|v| v.as_str())).collect::<String>(),
                                    events.iter().filter_map(|v| v.pointer("/choices/0/delta/reasoning_content").and_then(|v| v.as_str())).collect::<String>(),
                                    events.iter().filter(|v| v.pointer("/choices/0/delta/tool_calls/0/id").is_some()).count(),
                                    events.last().unwrap()["usage"]["completion_tokens"].clone(),
                                ),
                                "responses" => (
                                    events.iter().filter(|v| v["type"] == "response.output_text.delta").filter_map(|v| v["delta"].as_str()).collect(),
                                    events.iter().filter(|v| v["type"] == "response.reasoning_summary_text.delta").filter_map(|v| v["delta"].as_str()).collect(),
                                    events.iter().filter(|v| v["type"] == "response.output_item.added" && v["item"]["type"] == "function_call").count(),
                                    events.last().unwrap()["response"]["usage"]["output_tokens"].clone(),
                                ),
                                _ => (
                                    events.iter().filter(|v| v["delta"]["type"] == "text_delta").filter_map(|v| v["delta"]["text"].as_str()).collect(),
                                    events.iter().filter(|v| v["delta"]["type"] == "thinking_delta").filter_map(|v| v["delta"]["thinking"].as_str()).collect(),
                                    events.iter().filter(|v| v["type"] == "content_block_start" && v["content_block"]["type"] == "tool_use").count(),
                                    events.iter().find(|v| v["type"] == "message_delta").unwrap()["usage"]["output_tokens"].clone(),
                                ),
                            };
                            assert_eq!(text, expected_text, "{model} {endpoint}: {body}");
                            assert_eq!(reasoning, "fixture reasoning", "{model} {endpoint}: {body}");
                            assert_eq!(tools, tool_count, "{endpoint}: {body}");
                            assert_eq!(output_tokens, 17, "{endpoint}: {body}");
                        } else {
                            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                            match endpoint {
                                "chat" => {
                                    let message = &body["choices"][0]["message"];
                                    assert_eq!(message["content"], expected_text);
                                    assert_eq!(message["reasoning_content"], "fixture reasoning");
                                    assert_eq!(message["tool_calls"].as_array().map_or(0, Vec::len), tool_count);
                                    assert_eq!(body["usage"]["completion_tokens"], 17);
                                }
                                "responses" => {
                                    let output = body["output"].as_array().unwrap();
                                    let thinking: String = output.iter().filter(|v| v["type"] == "reasoning")
                                        .filter_map(|v| v["summary"][0]["text"].as_str()).collect();
                                    let text: String = output.iter().filter(|v| v["type"] == "message")
                                        .filter_map(|v| v["content"][0]["text"].as_str()).collect();
                                    assert_eq!(thinking, "fixture reasoning");
                                    assert_eq!(text, expected_text);
                                    assert_eq!(output.iter().filter(|v| v["type"] == "function_call").count(), tool_count);
                                    assert_eq!(body["usage"]["output_tokens"], 17);
                                }
                                _ => {
                                    let content = body["content"].as_array().unwrap();
                                    assert_eq!(content[0]["type"], "thinking");
                                    assert_eq!(content[0]["thinking"], "fixture reasoning");
                                    assert_eq!(content[0]["signature"], if search_loop { super::stream::THINKING_SIGNATURE_PLACEHOLDER } else { "fixture-signature" });
                                    let text: String = content.iter().filter(|v| v["type"] == "text").filter_map(|v| v["text"].as_str()).collect();
                                    assert_eq!(text, expected_text);
                                    assert_eq!(content.iter().filter(|v| v["type"] == "tool_use").count(), tool_count);
                                    assert_eq!(body["usage"]["output_tokens"], 17);
                                }
                            }
                        }
                        let requests = f.requests.lock();
                        assert_eq!(requests.len(), 1);
                        let additional = &requests[0]["additionalModelRequestFields"];
                        assert!(additional.get("output_config").is_none());
                        assert!(additional.get("reasoning_effort").is_none());
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn gateway_forwards_output_budget_and_preserves_limit_reason_across_protocols() {
    for model in ["claude-opus-4-6", "claude-opus-4-8", "claude-opus-5-5"] {
        for endpoint in ["messages", "cc", "chat", "responses"] {
            for streaming in [false, true] {
                for budget in [64000, 128000] {
                    let f = fixture(Mode::OutputLimit).await;
                    let req = json!({
                        "model":model, "stream":streaming,
                        "max_tokens":budget, "max_output_tokens":budget,
                        "messages":[{"role":"user", "content":"hello"}], "input":"hello"
                    });
                    let response = request_payload(f.state.clone(), endpoint, req).await;
                    assert_eq!(response.status(), StatusCode::OK, "{endpoint} stream={streaming}");
                    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
                    let body = String::from_utf8(bytes.to_vec()).unwrap();
                    let expected = match endpoint {
                        "chat" => "\"finish_reason\":\"length\"",
                        "responses" => "\"status\":\"incomplete\"",
                        _ => "\"stop_reason\":\"max_tokens\"",
                    };
                    assert!(body.contains(expected), "{endpoint} stream={streaming}: {body}");
                    assert!(body.contains("unfinished sentence"), "{body}");
                    let requests = f.requests.lock();
                    assert_eq!(requests.len(), 1);
                    assert_eq!(requests[0]["additionalModelRequestFields"]["max_tokens"], if model == "claude-opus-4-6" {64000} else {budget});
                }
            }
        }
    }
}

#[tokio::test]
async fn gateway_rejects_unsupported_opus_budget_before_upstream_request() {
    for endpoint in ["messages", "cc", "chat", "responses"] {
        for budget in [1, 1023] {
            let f = fixture(Mode::OutputLimit).await;
            let mut req = json!({
                "model":"claude-opus-4-6", "stream":true,
                "max_tokens":budget, "max_output_tokens":budget,
                "messages":[{"role":"user", "content":"hello"}], "input":"hello"
            });
            if endpoint == "messages" || endpoint == "cc" {
                // Mixed tools normally enter an internal SSE search loop. Invalid
                // budgets must still be rejected before sending its HTTP 200.
                req["tools"] = json!([
                    {"type":"web_search_20250305", "name":"web_search"},
                    {"name":"run", "input_schema":{"type":"object", "properties":{}}}
                ]);
            }
            let response = request_payload(f.state.clone(), endpoint, req).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{endpoint}");
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            let body = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(body.contains("at least 1024"), "{endpoint}: {body}");
            assert_eq!(f.calls.load(Ordering::Relaxed), 0);
        }
    }
}

#[tokio::test]
async fn gateway_cc_emits_content_before_upstream_finishes() {
    let f = fixture(Mode::Normal(Duration::from_millis(400))).await;
    let mut body = request(f.state.clone(), "cc", true)
        .await
        .into_body()
        .into_data_stream();
    tokio::time::timeout(Duration::from_millis(250), first_content(&mut body))
        .await
        .expect("CC buffered content until EOF");
    let mut rest = String::new();
    while let Some(chunk) = body.next().await {
        rest.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
    }
    assert!(rest.contains("\"input_tokens\":13"));
    assert!(rest.contains("\"output_tokens\":17"));
}

#[tokio::test]
async fn gateway_chat_emits_content_before_upstream_finishes() {
    let f = fixture(Mode::Normal(Duration::from_millis(400))).await;
    let started = Instant::now();
    let response = request(f.state.clone(), "chat", true).await;
    let mut body = response.into_body().into_data_stream();
    first_content(&mut body).await;
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "Chat buffered content until EOF"
    );
}

#[tokio::test]
async fn gateway_cc_disconnect_is_an_error_not_message_stop() {
    let f = fixture(Mode::Interrupted).await;
    let response = request(f.state.clone(), "cc", true).await;
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("event: error"),
        "disconnect was hidden: {text}"
    );
    assert!(!text.contains("event: message_stop"));
}

#[tokio::test]
async fn gateway_reuses_upstream_connection() {
    let f = fixture(Mode::Normal(Duration::ZERO)).await;
    for _ in 0..3 {
        let response = request(f.state.clone(), "messages", false).await;
        assert_eq!(response.status(), StatusCode::OK);
        to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    }
    assert_eq!(f.close_headers.load(Ordering::Relaxed), 0);
    assert_eq!(f.connections.lock().len(), 1);
}

#[tokio::test]
async fn gateway_model_overload_does_not_retry_three_times() {
    let f = fixture(Mode::Overloaded).await;
    let response = request(f.state.clone(), "cc", true).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(response.headers().contains_key("retry-after"));
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("overloaded_error"));
    assert_eq!(f.calls.load(Ordering::Relaxed), 1);
    let second = request(f.state.clone(), "cc", true).await;
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        f.calls.load(Ordering::Relaxed),
        1,
        "cooldown still hits overloaded upstream"
    );
}

#[tokio::test]
async fn gateway_non_stream_decodes_responses_larger_than_decoder_buffer() {
    let f = fixture(Mode::Large).await;
    let response = request(f.state.clone(), "messages", false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap();
    let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        response["content"][0]["text"].as_str().unwrap().len(),
        18000 * 1024
    );
}

#[tokio::test]
async fn gateway_damaged_frames_are_errors_not_success() {
    for mode in [Mode::Truncated, Mode::InvalidFrame, Mode::Empty] {
        let f = fixture(mode).await;
        for endpoint in ["cc", "messages"] {
            let response = request(f.state.clone(), endpoint, true).await;
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                text.contains("event: error"),
                "corrupt stream was accepted: {text}"
            );
            assert!(!text.contains("event: message_stop"));
        }
        let response = request(f.state.clone(), "messages", false).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
}

#[tokio::test]
async fn gateway_chat_disconnect_has_no_success_finish_or_done() {
    let f = fixture(Mode::Interrupted).await;
    let response = request(f.state.clone(), "chat", true).await;
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("\"error\""));
    assert!(!text.contains("[DONE]"));
    assert!(!text.contains("\"finish_reason\":\"stop\""));
}

#[tokio::test]
async fn gateway_chat_preserves_overload_retry_after() {
    let f = fixture(Mode::Overloaded).await;
    let response = request(f.state.clone(), "chat", true).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers().get("retry-after").unwrap(), "5");
}

#[tokio::test]
async fn gateway_responses_preserves_overload_retry_after() {
    let f = fixture(Mode::Overloaded).await;
    let response = request(f.state.clone(), "responses", true).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers().get("retry-after").unwrap(), "5");
}

#[tokio::test]
async fn gateway_chat_tool_arguments_and_final_usage_are_complete() {
    let f = fixture(Mode::Tools).await;
    let response = request(f.state.clone(), "chat", true).await;
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    let chunks: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|line| *line != "[DONE]")
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let mut arguments = String::new();
    for chunk in &chunks {
        if let Some(args) = chunk
            .pointer("/choices/0/delta/tool_calls/0/function/arguments")
            .and_then(|v| v.as_str())
        {
            arguments.push_str(args);
        }
    }
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&arguments).unwrap(),
        json!({"command":"hello"})
    );
    assert!(
        chunks
            .iter()
            .any(|chunk| chunk.pointer("/choices/0/finish_reason") == Some(&json!("tool_calls")))
    );
    let usage = &chunks.last().unwrap()["usage"];
    assert_eq!(usage["prompt_tokens"], 18);
    assert_eq!(usage["completion_tokens"], 17);
    assert!(text.ends_with("data: [DONE]\n\n"));
}

#[tokio::test]
async fn gateway_content_filter_refusal_is_not_a_successful_end_turn() {
    let f = fixture(Mode::Refused).await;
    async fn text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    let response = request(f.state.clone(), "messages", false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(body["stop_reason"], "refusal", "{body}");

    for endpoint in ["cc", "messages"] {
        let sse = text(request(f.state.clone(), endpoint, true).await).await;
        assert!(sse.contains(r#""stop_reason":"refusal""#), "{endpoint}: {sse}");
        assert!(!sse.contains("model_context_window_exceeded"), "{endpoint}: {sse}");
    }

    let chat = text(request(f.state.clone(), "chat", true).await).await;
    assert!(chat.contains(r#""finish_reason":"content_filter""#), "{chat}");
    assert!(!chat.contains(r#""finish_reason":"stop""#), "{chat}");

    let responses = text(request(f.state.clone(), "responses", true).await).await;
    assert!(responses.contains("event: response.incomplete"), "{responses}");
    assert!(responses.contains(r#""reason":"content_filter""#), "{responses}");
}

#[tokio::test]
async fn gateway_refusal_nonstream_compatibility_and_bare_metadata() {
    for mode in [Mode::Refused, Mode::RefusedBare] {
        let f = fixture(mode).await;
        for (endpoint, field, expected) in [
            ("messages", "/stop_reason", "refusal"),
            ("chat", "/choices/0/finish_reason", "content_filter"),
            ("responses", "/incomplete_details/reason", "content_filter"),
        ] {
            let response = request(f.state.clone(), endpoint, false).await;
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(status, StatusCode::OK, "{endpoint}: {body}");
            assert_eq!(
                body.pointer(field).and_then(|v| v.as_str()),
                Some(expected),
                "{endpoint}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn gateway_refused_round_does_not_execute_search() {
    let mut failures = Vec::new();
    for streaming in [false, true] {
        let f = fixture(Mode::RefusedWithSearch).await;
        let payload = json!({
            "model":"gpt-5.6-sol","max_tokens":64,"stream":streaming,
            "messages":[{"role":"user","content":"hello"}],
            "tools":[
                {"type":"web_search_20250305","name":"web_search"},
                {"name":"run","description":"test command","input_schema":{"type":"object","properties":{"command":{"type":"string"}}}}
            ]
        });
        let response = post_messages(
            axum::extract::State(f.state.clone()),
            Extension(key()),
            Json(serde_json::from_value(payload).unwrap()),
        )
        .await;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let body = String::from_utf8_lossy(&bytes);
        let calls = f.calls.load(Ordering::Relaxed);
        if status != StatusCode::OK
            || calls != 1
            || !body.contains("\"refusal\"")
            || body.contains("server_tool_use")
            || body.contains("web_search_tool_result")
        {
            failures.push(format!(
                "stream={streaming}, calls={calls}, status={status}, body={body}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn gateway_partial_tool_before_refusal_preserves_terminal_reason() {
    let mut failures = Vec::new();
    for mode in [
        Mode::RefusedWithPartialTool,
        Mode::RefusedWithInvalidTool,
        Mode::RefusedWithEmptyTool,
    ] {
        for streaming in [false, true] {
            let f = fixture(mode).await;
            let payload = json!({
                "model":"gpt-5.6-sol","max_tokens":64,"stream":streaming,
                "messages":[{"role":"user","content":"hello"}],
                "tools":[{"name":"run","description":"test command","input_schema":{"type":"object","properties":{"command":{"type":"string"}}}}]
            });
            let response = post_messages(
                axum::extract::State(f.state.clone()),
                Extension(key()),
                Json(serde_json::from_value(payload).unwrap()),
            )
            .await;
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            let body = String::from_utf8_lossy(&bytes);
            if status != StatusCode::OK
                || !body.contains("\"refusal\"")
                || body.contains("upstream_tool_json_error")
                || body.contains("\"type\":\"tool_use\"")
            {
                failures.push(format!("stream={streaming}, status={status}, body={body}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn gateway_stalled_body_times_out_without_normal_completion() {
    let config: Config = serde_json::from_value(json!({"upstreamReadTimeoutSecs":1})).unwrap();
    let f = fixture_with_config(Mode::Stalled, config).await;
    let response = request(f.state.clone(), "cc", true).await;
    let bytes = tokio::time::timeout(
        Duration::from_secs(2),
        to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .expect("idle stream did not time out")
    .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("event: error"));
    assert!(!text.contains("event: message_stop"));
    assert_eq!(f.manager.traffic_status()[0].concurrency, 0);
}

#[tokio::test]
async fn gateway_header_timeout_does_not_repeat_long_waits() {
    let config: Config = serde_json::from_value(json!({"upstreamReadTimeoutSecs":1})).unwrap();
    let f = fixture_with_config(Mode::SlowHeaders, config).await;
    let response =
        tokio::time::timeout(Duration::from_secs(2), request(f.state.clone(), "cc", true))
            .await
            .expect("header deadline was ignored or retried");
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(f.calls.load(Ordering::Relaxed), 1);
    assert_eq!(f.manager.traffic_status()[0].concurrency, 0);
}

#[tokio::test]
async fn gateway_error_body_timeout_does_not_repeat_long_waits() {
    let config: Config = serde_json::from_value(json!({"upstreamReadTimeoutSecs":1})).unwrap();
    let f = fixture_with_config(Mode::SlowErrorBody, config).await;
    let response =
        tokio::time::timeout(Duration::from_secs(2), request(f.state.clone(), "cc", true))
            .await
            .expect("error response body deadline was ignored or retried");
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(f.calls.load(Ordering::Relaxed), 1);
    assert_eq!(f.manager.traffic_status()[0].concurrency, 0);
}

#[tokio::test]
async fn gateway_first_token_trace_ignores_metadata() {
    let mut f = fixture(Mode::MetadataFirst).await;
    let store = Arc::new(TraceStore::open_in_memory().unwrap());
    f.state = f.state.clone().with_trace_store(Some(store.clone()));
    let response = request(f.state.clone(), "cc", true).await;
    to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let (records, count) = store.query_paged(&TraceQuery {
        limit: 10,
        ..Default::default()
    });
    assert_eq!(count, 1);
    assert!(
        records[0].first_token_ms.unwrap() >= 90,
        "metadata was counted as the first token: {:?}",
        records[0].first_token_ms
    );
}

#[tokio::test]
async fn gateway_active_stream_can_exceed_idle_deadline() {
    let config: Config = serde_json::from_value(json!({"upstreamReadTimeoutSecs":1})).unwrap();
    let f = fixture_with_config(Mode::Active, config).await;
    let started = Instant::now();
    let response = request(f.state.clone(), "cc", true).await;
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(started.elapsed() > Duration::from_secs(1));
    assert!(text.contains("event: message_stop"));
    assert!(!text.contains("event: error"));
}

#[tokio::test]
async fn gateway_total_deadline_is_configurable_even_for_active_streams() {
    for (total, should_finish) in [(1, false), (3, true)] {
        let config: Config = serde_json::from_value(json!({
            "upstreamRequestTimeoutSecs": total,
            "upstreamReadTimeoutSecs": 5,
        })).unwrap();
        let f = fixture_with_config(Mode::Active, config).await;
        let response = request(f.state.clone(), "messages", true).await;
        let bytes = tokio::time::timeout(
            Duration::from_secs(4),
            to_bytes(response.into_body(), 1024 * 1024),
        ).await.expect("total deadline ignored").unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(text.contains("event: message_stop"), should_finish);
        assert_eq!(text.contains("event: error"), !should_finish);
        assert_eq!(f.manager.traffic_status()[0].concurrency, 0);
    }
}

#[tokio::test]
#[ignore = "controlled latency measurement; run with --ignored --nocapture"]
async fn gateway_latency_measurement() {
    for endpoint in ["cc", "chat", "messages"] {
        let f = fixture(Mode::Normal(Duration::from_millis(200))).await;
        let mut first = Vec::new();
        let mut total = Vec::new();
        for _ in 0..10 {
            let start = Instant::now();
            let mut body = request(f.state.clone(), endpoint, true)
                .await
                .into_body()
                .into_data_stream();
            first_content(&mut body).await;
            first.push(start.elapsed().as_secs_f64() * 1000.0);
            while let Some(chunk) = body.next().await {
                chunk.unwrap();
            }
            total.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        first.sort_by(f64::total_cmp);
        total.sort_by(f64::total_cmp);
        println!(
            "LATENCY endpoint={endpoint} first_p50_ms={:.2} first_p95_ms={:.2} total_p50_ms={:.2} connections={} requests={}",
            first[5],
            first[9],
            total[5],
            f.connections.lock().len(),
            f.calls.load(Ordering::Relaxed)
        );
    }
}

#[tokio::test]
async fn gateway_traffic_streams_hold_permit_until_disconnect_or_eof() {
    for endpoint in ["messages", "cc", "chat", "responses"] {
        let mut config = Config::default();
        config.traffic_policies.default_concurrency = 1;
        let f = fixture_with_config(Mode::Normal(Duration::from_millis(150)), config).await;
        let response = request(f.state.clone(), endpoint, true).await;
        assert_eq!(response.status(), 200, "{endpoint}");
        assert_eq!(f.manager.traffic_status()[0].concurrency, 1, "{endpoint}: headers must not release permit");
        let blocked = request(f.state.clone(), endpoint, true).await;
        assert_eq!(blocked.status(), 429, "{endpoint}");
        assert_eq!(f.calls.load(Ordering::Relaxed), 1);
        drop(response);
        // Responses adapters may run a task that observes body cancellation asynchronously.
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.manager.traffic_status()[0].concurrency != 0 { tokio::task::yield_now().await; }
        }).await.unwrap();
        let mut body = request(f.state.clone(), endpoint, true).await.into_body().into_data_stream();
        while let Some(chunk) = body.next().await { chunk.unwrap(); }
        assert_eq!(f.manager.traffic_status()[0].concurrency, 0, "{endpoint}: EOF releases permit");
        assert_eq!(f.manager.traffic_status()[0].rpm, 2);
    }
}

#[tokio::test]
async fn gateway_traffic_non_stream_errors_release_permit() {
    for mode in [Mode::Empty, Mode::Interrupted, Mode::InvalidFrame, Mode::Overloaded] {
        let f = fixture(mode).await;
        let _response = request(f.state.clone(), "messages", false).await;
        assert_eq!(f.manager.traffic_status()[0].concurrency, 0);
    }
}

#[tokio::test]
async fn gateway_traffic_mcp_holds_permit_during_body_and_releases_on_drop() {
    let mut config = Config::default();
    config.traffic_policies.default_concurrency = 1;
    let f = fixture_with_config(Mode::Normal(Duration::from_millis(20)), config).await;
    let response = f.provider.call_mcp("{}", None).await.unwrap();
    assert_eq!(f.manager.traffic_status()[0].concurrency, 1);
    assert!(f.provider.call_mcp("{}", None).await.is_err());
    drop(response);
    assert_eq!(f.manager.traffic_status()[0].concurrency, 0);
    let response = f.provider.call_mcp("{}", None).await.unwrap();
    response.text().await.unwrap();
    assert_eq!(f.manager.traffic_status()[0].concurrency, 0);
    assert_eq!(f.manager.traffic_status()[0].rpm, 2);
}
