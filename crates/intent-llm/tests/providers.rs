//! HTTP-level tests against a wiremock server: OpenAI-compatible success, 429 with
//! Retry-After, repair turns, fenced/<think> outputs, schema fallback, error metadata
//! for Gemini/Bedrock, embeddings and the reachability probe.

use std::sync::Arc;
use std::time::{Duration, Instant};

use intent_llm::client::{
    BedrockAuth, BedrockClient, BedrockConfig, ChatClient, GeminiAuth, GeminiClient, GeminiConfig, JsonRequest,
    OpenAiCompatible, OpenAiConfig,
};
use intent_llm::embed::{OpenAiEmbedder, OpenAiEmbedderConfig};
use intent_llm::harness::{Harness, HarnessConfig, HarnessError, RetryPolicy, StructuredCall};
use intent_llm::label::LlmLabelProvider;
use intent_llm::settings::probe_local_server;
use intent_llm::traits::{Embedder, LabelLevel, LabelProvider};
use intent_types::SafeIntentFeatures;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Answer {
    label: String,
    n: i64,
}

fn chat_reply(content: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "x", "object": "chat.completion",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}]
    }))
}

fn local_client(server: &MockServer) -> Arc<dyn ChatClient> {
    let mut cfg = OpenAiConfig::local(format!("{}/v1", server.uri()), "qwen2.5:3b");
    cfg.timeout = Duration::from_secs(5);
    Arc::new(OpenAiCompatible::new(cfg))
}

fn harness(server: &MockServer) -> Harness {
    let client = local_client(server);
    let mut config = HarnessConfig::for_identity(&client.identity());
    config.retry = RetryPolicy { base_delay: Duration::from_millis(20), ..RetryPolicy::default() };
    config.default_deadline = Duration::from_secs(10);
    Harness::new(client, config, None)
}

fn schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["label", "n"],
           "properties": {"label": {"type": "string"}, "n": {"type": "integer"}}})
}

fn call(user: &str) -> StructuredCall<'static> {
    StructuredCall::new("test", "v1", "Return JSON.".into(), user.into(), "answer", schema())
}

fn bodies(requests: &[Request]) -> Vec<Value> {
    requests.iter().map(|r| serde_json::from_slice(&r.body).unwrap()).collect()
}

#[tokio::test]
async fn openai_compatible_success_uses_json_schema_and_caches() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(chat_reply(r#"{"label":"ok","n":3}"#))
        .mount(&server)
        .await;
    let h = harness(&server);
    let first = h.structured::<Answer, _>(call("hello"), |_| Ok(())).await.unwrap();
    assert_eq!(first.value, Answer { label: "ok".into(), n: 3 });
    assert!(!first.cached);
    let second = h.structured::<Answer, _>(call("hello"), |_| Ok(())).await.unwrap();
    assert!(second.cached, "identical request served from cache");

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body = &bodies(&reqs)[0];
    assert_eq!(body["model"], "qwen2.5:3b");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["schema"], schema());
    assert_eq!(body["messages"][0]["role"], "system");
    let stats = h.stats();
    assert_eq!((stats.cache_hits, stats.calls), (1, 2));
}

#[tokio::test]
async fn retries_429_honouring_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1").set_body_json(
            json!({"error": {"message": "slow down", "type": "rate_limit", "code": "rate_limit_exceeded"}}),
        ))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(chat_reply(r#"{"label":"after","n":1}"#))
        .with_priority(2)
        .mount(&server)
        .await;

    let h = harness(&server);
    let started = Instant::now();
    let out = h.structured::<Answer, _>(call("x"), |_| Ok(())).await.unwrap();
    assert_eq!(out.value.label, "after");
    assert_eq!(out.attempts, 2);
    assert!(started.elapsed() >= Duration::from_secs(1), "waited for Retry-After");
}

#[tokio::test]
async fn transport_preserves_status_retry_after_and_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "7")
                .set_body_json(json!({"error": {"message": "tokens", "code": "rate_limit_exceeded"}})),
        )
        .mount(&server)
        .await;
    let err = local_client(&server).respond_json(&JsonRequest::new("s", "u", "answer", schema())).await.unwrap_err();
    assert_eq!(err.status_code, Some(429));
    assert_eq!(err.retry_after_seconds, Some(7.0));
    assert_eq!(err.error_code.as_deref(), Some("rate_limit_exceeded"));
}

#[tokio::test]
async fn non_retryable_error_fails_fast() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": {"message": "bad key", "code": "invalid_api_key"}})),
        )
        .mount(&server)
        .await;
    let h = harness(&server);
    match h.structured::<Answer, _>(call("x"), |_| Ok(())).await {
        Err(HarnessError::Provider(e)) => {
            assert_eq!(e.status_code, Some(401));
            assert_eq!(e.error_code.as_deref(), Some("invalid_api_key"));
        }
        other => panic!("expected provider error, got {other:?}"),
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_json_then_repair_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(chat_reply("Sure! The label is ok."))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(chat_reply(r#"{"label":"fixed","n":2}"#))
        .with_priority(2)
        .mount(&server)
        .await;
    let h = harness(&server);
    let out = h.structured::<Answer, _>(call("x"), |_| Ok(())).await.unwrap();
    assert!(out.repaired);
    assert_eq!(out.value.label, "fixed");
    let reqs = bodies(&server.received_requests().await.unwrap());
    let repair_messages = reqs[1]["messages"].as_array().unwrap();
    assert_eq!(repair_messages.len(), 4);
    assert_eq!(repair_messages[2]["role"], "assistant");
    assert_eq!(repair_messages[2]["content"], "Sure! The label is ok.");
    assert!(repair_messages[3]["content"].as_str().unwrap().contains("rejected"));
}

#[tokio::test]
async fn validation_error_is_sent_back_and_second_failure_is_invalid() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(chat_reply(r#"{"label":"x","n":-1}"#)).mount(&server).await;
    let h = harness(&server);
    let result = h
        .structured::<Answer, _>(call("x"), |a| if a.n >= 0 { Ok(()) } else { Err("n must be non-negative".into()) })
        .await;
    assert!(matches!(result, Err(HarnessError::Invalid(ref m)) if m.contains("non-negative")));
    let reqs = bodies(&server.received_requests().await.unwrap());
    assert_eq!(reqs.len(), 2, "exactly one repair turn");
    assert!(reqs[1]["messages"][3]["content"].as_str().unwrap().contains("n must be non-negative"));
}

#[tokio::test]
async fn fenced_and_think_outputs_are_extracted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("fenced"))
        .respond_with(chat_reply("Here you go:\n```json\n{\"label\": \"fence\", \"n\": 1}\n```\nAnything else?"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("thinking"))
        .respond_with(chat_reply(
            "<think>The user wants {label}. Maybe {\"label\":\"wrong\"}?</think>\n{\"label\":\"think\",\"n\":2}",
        ))
        .mount(&server)
        .await;
    let h = harness(&server);
    let a = h.structured::<Answer, _>(call("fenced"), |_| Ok(())).await.unwrap();
    assert_eq!(a.value.label, "fence");
    assert!(!a.repaired);
    let b = h.structured::<Answer, _>(call("thinking"), |_| Ok(())).await.unwrap();
    assert_eq!(b.value.label, "think");
    assert!(!b.repaired);
}

#[tokio::test]
async fn json_schema_rejection_falls_back_to_json_object() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("json_schema"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"error": {"message": "response_format json_schema is not supported"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("json_object"))
        .respond_with(chat_reply(r#"{"label":"obj","n":5}"#))
        .mount(&server)
        .await;
    let h = harness(&server);
    let out = h.structured::<Answer, _>(call("x"), |_| Ok(())).await.unwrap();
    assert_eq!(out.value.label, "obj");
    let reqs = bodies(&server.received_requests().await.unwrap());
    assert_eq!(reqs.len(), 2);
    assert!(
        reqs[1]["messages"][0]["content"].as_str().unwrap().contains("JSON schema"),
        "schema moved into the prompt"
    );
    // The client remembers: the next call goes straight to json_object.
    h.structured::<Answer, _>(call("y"), |_| Ok(())).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn deadline_bounds_retries() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "30"))
        .mount(&server)
        .await;
    let h = harness(&server);
    let mut c = call("x");
    c.deadline = Some(Duration::from_millis(500));
    let started = Instant::now();
    let err = h.structured::<Answer, _>(c, |_| Ok(())).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(matches!(err, HarnessError::Provider(ref e) if e.status_code == Some(503)));
}

#[tokio::test]
async fn secrets_are_redacted_before_sending() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(chat_reply(r#"{"label":"ok","n":0}"#)).mount(&server).await;
    let h = harness(&server);
    h.structured::<Answer, _>(
        call(r#"{"cmd":"export OPENAI_API_KEY=sk-abcdefghijk123 && curl -H 'Authorization: Bearer abc.def'"}"#),
        |_| Ok(()),
    )
    .await
    .unwrap();
    let raw = String::from_utf8(server.received_requests().await.unwrap()[0].body.clone()).unwrap();
    assert!(!raw.contains("sk-abcdefghijk123"));
    assert!(!raw.contains("abc.def"));
}

#[tokio::test]
async fn label_provider_round_trip_and_fallback_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(chat_reply(
            r#"{"label":"Terraform VPC work","summary":"Planned VPC changes with terraform.","confidence":0.82}"#,
        ))
        .mount(&server)
        .await;
    let h = Arc::new(harness(&server));
    let provider = LlmLabelProvider::new(h, None);
    assert!(provider.cache_identity().starts_with("label:local:qwen2.5:3b:"));
    let features = SafeIntentFeatures { command_families: vec!["terraform".into()], ..Default::default() };
    let out = provider.label(&features, LabelLevel::Cluster).await.unwrap();
    assert_eq!(out.label, "Terraform VPC work");
    assert!((out.confidence - 0.82).abs() < 1e-9);
    let body = String::from_utf8(server.received_requests().await.unwrap()[0].body.clone()).unwrap();
    assert!(body.contains("terraform"));
}

#[tokio::test]
async fn gemini_api_key_success_and_retry_info() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
        .and(header("x-goog-api-key", "AIza-test"))
        .and(body_string_contains("first"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "candidates": [{"content": {"role": "model", "parts": [{"text": "thinking", "thought": true}, {"text": "{\"label\":\"g\",\"n\":1}"}]}}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("second"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({"error": {
            "code": 429, "message": "Resource exhausted", "status": "RESOURCE_EXHAUSTED",
            "details": [{"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "36s"}]
        }})))
        .mount(&server)
        .await;
    let client = GeminiClient::new(GeminiConfig {
        auth: GeminiAuth::ApiKey("AIza-test".into()),
        model: "gemini-2.5-flash".into(),
        timeout: Duration::from_secs(5),
        base_url: Some(server.uri()),
    });
    let text = client.respond_json(&JsonRequest::new("s", "first", "answer", schema())).await.unwrap();
    assert_eq!(text, r#"{"label":"g","n":1}"#);
    let body = &bodies(&server.received_requests().await.unwrap())[0];
    assert_eq!(body["generationConfig"]["responseSchema"]["type"], "OBJECT");
    assert!(body["generationConfig"]["responseSchema"].get("additionalProperties").is_none());

    let err = client.respond_json(&JsonRequest::new("s", "second", "answer", schema())).await.unwrap_err();
    assert_eq!(err.status_code, Some(429));
    assert_eq!(err.retry_after_seconds, Some(36.0));
    assert_eq!(err.error_code.as_deref(), Some("RESOURCE_EXHAUSTED"));
}

#[tokio::test]
async fn bedrock_api_key_converse_and_throttling_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/amazon.nova-lite-v1%3A0/converse"))
        .and(header("authorization", "Bearer brk-key"))
        .and(body_string_contains("ok-please"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": {"message": {"role": "assistant", "content": [{"text": "{\"label\":\"b\",\"n\":4}"}]}},
            "stopReason": "end_turn"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("throttle-me"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("x-amzn-ErrorType", "ThrottlingException:http://internal.amazon.com/coral/")
                .insert_header("retry-after", "2")
                .set_body_json(json!({"message": "Too many requests"})),
        )
        .mount(&server)
        .await;
    let client = BedrockClient::new(BedrockConfig {
        region: "us-east-1".into(),
        auth: BedrockAuth::ApiKey("brk-key".into()),
        model: "amazon.nova-lite-v1:0".into(),
        timeout: Duration::from_secs(5),
        max_tokens: 512,
        structured_output: Some(true),
        base_url: Some(server.uri()),
    });
    let text = client.respond_json(&JsonRequest::new("s", "ok-please", "answer", schema())).await.unwrap();
    assert_eq!(text, r#"{"label":"b","n":4}"#);
    let body = &bodies(&server.received_requests().await.unwrap())[0];
    assert_eq!(body["outputConfig"]["textFormat"]["type"], "json_schema");

    let err = client.respond_json(&JsonRequest::new("s", "throttle-me", "answer", schema())).await.unwrap_err();
    assert_eq!(err.status_code, Some(429));
    assert_eq!(err.retry_after_seconds, Some(2.0));
    assert_eq!(err.error_code.as_deref(), Some("ThrottlingException"));
    assert!(err.is_retryable());
}

#[tokio::test]
async fn native_tool_calls_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"choices": [{"message": {
            "role": "assistant", "content": null,
            "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "search_intents", "arguments": "{\"query\":\"vpc\"}"}}]
        }}]})))
        .mount(&server)
        .await;
    let turn = local_client(&server)
        .respond_with_tools("sys", &[intent_llm::client::ChatMessage::user("q")], &[])
        .await
        .unwrap();
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].arguments, json!({"query": "vpc"}));
    assert_eq!(turn.tool_calls[0].id, "call_1");
}

#[tokio::test]
async fn ollama_without_tool_support_is_flagged() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400).set_body_json(
                json!({"error": {"message": "registry.ollama.ai/library/gemma:2b does not support tools"}}),
            ),
        )
        .mount(&server)
        .await;
    let client = local_client(&server);
    let err = client.respond_with_tools("sys", &[intent_llm::client::ChatMessage::user("q")], &[]).await.unwrap_err();
    assert_eq!(err.error_code.as_deref(), Some("tools_unsupported"));
    assert!(!client.supports_native_tools());
}

#[tokio::test]
async fn openai_embeddings_batch_in_order() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(|req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let n = body["input"].as_array().unwrap().len();
            // Return out of order to check index handling.
            let data: Vec<Value> = (0..n)
                .rev()
                .map(|i| json!({"object": "embedding", "index": i, "embedding": [i as f64, 1.0, 0.0]}))
                .collect();
            ResponseTemplate::new(200)
                .set_body_json(json!({"object": "list", "data": data, "model": "nomic-embed-text"}))
        })
        .mount(&server)
        .await;
    let e = OpenAiEmbedder::new(OpenAiEmbedderConfig {
        provider: "local".into(),
        base_url: format!("{}/v1", server.uri()),
        api_key: None,
        model: "nomic-embed-text".into(),
        batch_size: 2,
        timeout: Duration::from_secs(5),
    });
    assert_eq!(e.dims(), 768);
    let texts: Vec<String> = (0..5).map(|i| format!("t{i}")).collect();
    let out = e.embed(&texts).await.unwrap();
    assert_eq!(out.len(), 5);
    assert_eq!(out[1][0], 1.0);
    assert_eq!(out[4][0], 0.0, "second batch indexes restart at 0");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert_eq!(e.dims(), 3, "dims learned from responses");
}

#[tokio::test]
async fn probe_lists_models_and_handles_down_servers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"object": "list", "data": [{"id": "qwen2.5:3b"}, {"id": "nomic-embed-text"}]})),
        )
        .mount(&server)
        .await;
    let ok = probe_local_server(&format!("{}/v1/", server.uri()), Duration::from_secs(2)).await;
    assert!(ok.reachable);
    assert_eq!(ok.models, vec!["qwen2.5:3b", "nomic-embed-text"]);
    let down = probe_local_server("http://127.0.0.1:9/v1", Duration::from_millis(500)).await;
    assert!(!down.reachable);
    assert!(down.error.is_some());
}
