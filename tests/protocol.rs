use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use confuse::{
    api::router,
    store::{ApiKey, DEFAULT_PROJECT, Store},
};
use flate2::{Compression, write::GzEncoder};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message as _;
use serde_json::{Value, json};
use std::io::Write;
use tower::ServiceExt;

const TRACE: &str = "0123456789abcdef0123456789abcdef";
const PARENT: &str = "0123456789abcdef";

fn payload() -> Value {
    json!({"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"example"}}]},
        "scopeSpans":[{"scope":{"name":"langfuse-sdk"},"spans":[
        {"traceId":TRACE,"spanId":"fedcba9876543210","parentSpanId":PARENT,"name":"model",
        "startTimeUnixNano":"1760000000100000000","endTimeUnixNano":"1760000000200000000",
        "attributes":[
            {"key":"langfuse.observation.type","value":{"stringValue":"generation"}},
            {"key":"langfuse.observation.model.name","value":{"stringValue":"fixture"}},
            {"key":"langfuse.observation.usage_details","value":{"stringValue":"{\"input\":11,\"output\":7}"}},
            {"key":"langfuse.observation.output","value":{"stringValue":"\"response\""}}
        ]},
        {"traceId":TRACE,"spanId":PARENT,"name":"parent","startTimeUnixNano":"1760000000000000000","endTimeUnixNano":"1760000000300000000",
         "attributes":[
             {"key":"langfuse.trace.session_id","value":{"stringValue":"fixture-session"}},
             {"key":"langfuse.observation.input","value":{"stringValue":"\"example\""}}
        ]}
    ]}]}]})
}

async fn send(
    app: &Router,
    key: Option<&ApiKey>,
    method: &str,
    path: &str,
    body: Body,
    content_type: &str,
    encoding: &str,
) -> (StatusCode, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", content_type)
        .header("content-encoding", encoding);
    if let Some(key) = key {
        request = request.header(
            "authorization",
            format!(
                "Basic {}",
                STANDARD.encode(format!("{}:{}", key.public_key, key.secret_key))
            ),
        );
    }
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    (
        response.status(),
        to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap()
            .to_vec(),
    )
}

#[tokio::test]
async fn otlp_json_and_protobuf_retries_share_trace_and_project_boundary() {
    let store = Store::memory().unwrap();
    let key = store.create_key(DEFAULT_PROJECT, "sdk").unwrap();
    let other = store.create_project("other").unwrap();
    let other = store.create_key(&other.id, "sdk").unwrap();
    let app = router(store);
    let endpoint = "/api/public/otel/v1/traces";
    let json = payload();
    let proto: ExportTraceServiceRequest = serde_json::from_value(json.clone()).unwrap();
    let encoded = serde_json::to_vec(&json).unwrap();
    assert_eq!(
        send(
            &app,
            None,
            "POST",
            endpoint,
            Body::from(encoded.clone()),
            "application/json",
            "identity"
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            Some(&key),
            "POST",
            endpoint,
            Body::from(encoded),
            "application/json",
            "identity"
        )
        .await
        .0,
        StatusCode::OK
    );

    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(&proto.encode_to_vec()).unwrap();
    assert_eq!(
        send(
            &app,
            Some(&key),
            "POST",
            endpoint,
            Body::from(gzip.finish().unwrap()),
            "application/x-protobuf",
            "gzip"
        )
        .await
        .0,
        StatusCode::OK
    );
    let path = format!("/api/public/traces/{TRACE}");
    let (status, data) = send(
        &app,
        Some(&key),
        "GET",
        &path,
        Body::empty(),
        "application/json",
        "identity",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let trace: Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(trace["name"], "parent");
    assert_eq!(trace["sessionId"], "fixture-session");
    assert_eq!(trace["observations"].as_array().unwrap().len(), 2);
    let child = &trace["observations"][1];
    assert_eq!(child["parentObservationId"], PARENT);
    assert_eq!(child["usage"]["total"], 18);
    assert_eq!(child["output"], "response");
    assert_eq!(
        send(
            &app,
            Some(&other),
            "GET",
            &path,
            Body::empty(),
            "application/json",
            "identity"
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn score_ingestion_contract_supports_retry() {
    let store = Store::memory().unwrap();
    let key = store.create_key(DEFAULT_PROJECT, "sdk").unwrap();
    let app = router(store.clone());
    let score = json!({"batch":[{"id":"fixture-event","type":"score-create","body":{"id":"fixture-score","traceId":TRACE,"name":"quality","value":1.0}}]});
    for _ in 0..2 {
        let (status, body) = send(
            &app,
            Some(&key),
            "POST",
            "/api/public/ingestion",
            Body::from(score.to_string()),
            "application/json",
            "identity",
        )
        .await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["successes"][0]["id"], "fixture-event");
    }
    assert_eq!(
        store
            .scores(DEFAULT_PROJECT, Some(TRACE), None, 50, 0)
            .unwrap()
            .1,
        1
    );
}

#[tokio::test]
async fn stop_releases_loopback_port() {
    let store = Store::memory().unwrap();
    let server = confuse::api::Server::start(store.clone(), 0).await.unwrap();
    let port = server.address.port();
    assert!(server.address.ip().is_loopback());
    server.stop().await;
    confuse::api::Server::start(store, port)
        .await
        .unwrap()
        .stop()
        .await;
}
