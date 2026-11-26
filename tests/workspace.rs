use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::IntoResponse,
    routing::post,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use confuse::{
    api,
    store::{DEFAULT_PROJECT, Store},
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower::ServiceExt;

#[test]
fn version_labels_move_without_rewriting_prompt_content() {
    let store = Store::memory().unwrap();
    let project = DEFAULT_PROJECT;
    let old = store
        .put_resource(
            project,
            "prompts",
            json!({"name":"hello","prompt":"First","labels":["production"]}),
        )
        .unwrap();
    assert_eq!(old["version"], 1);
    let new = store
        .put_resource(
            project,
            "prompts",
            json!({"name":"hello","prompt":"Second","labels":[]}),
        )
        .unwrap();
    assert_eq!(new["version"], 2);
    assert_eq!(
        store
            .versioned(project, "prompts", "hello", None, None)
            .unwrap()
            .unwrap()["prompt"],
        "First"
    );
    store
        .set_version_labels(project, "prompts", "hello", 2, &["production".to_owned()])
        .unwrap();
    assert_eq!(
        store
            .versioned(project, "prompts", "hello", None, None)
            .unwrap()
            .unwrap()["prompt"],
        "Second"
    );
    let v1 = store
        .versioned(project, "prompts", "hello", Some(1), None)
        .unwrap()
        .unwrap();
    assert_eq!(v1["prompt"], "First");
    assert_eq!(v1["labels"], json!([]));
}

#[tokio::test]
async fn gateway_preserves_stream_and_records_usage() {
    let store = Store::memory().unwrap();
    let stream = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\n",
        "data: [DONE]\n\n"
    );
    let provider = Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            ([(header::CONTENT_TYPE, "text/event-stream")], stream).into_response()
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
    store.configure_provider(DEFAULT_PROJECT,json!({
        "name":"fixture","format":"openai","baseUrl":format!("http://{address}/v1"),"model":"fixture"
    })).unwrap();
    let key = store.create_key(DEFAULT_PROJECT, "gateway").unwrap();
    let router = api::router(store.clone());
    let request = Request::post("/openai/v1/chat/completions")
        .header("authorization", format!("Bearer {}", key.secret_key))
        .body(Body::from(
            json!({"model":"fixture","messages":[{"role":"user","content":"Hello"}],"stream":true})
                .to_string(),
        ))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), stream.as_bytes());
    let rows = store
        .observations(DEFAULT_PROJECT, None, Some("GENERATION"), 100, 0)
        .unwrap()
        .0;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].output, "Hello");
    assert_eq!(rows[0].usage["input"], 3);
    task.abort();
}

#[tokio::test]
async fn media_grant_validates_bytes_and_the_project() {
    let store = Store::memory().unwrap();
    let key = store.create_key(DEFAULT_PROJECT, "sdk").unwrap();
    let auth = STANDARD.encode(format!("{}:{}", key.public_key, key.secret_key));
    let app = api::router(store);
    use sha2::{Digest, Sha256};
    let bytes = b"fixture content";
    let response=app.clone().oneshot(Request::post("/api/public/media").header("authorization",format!("Basic {auth}"))
        .header("host","127.0.0.1:32127").body(Body::from(json!({
            "contentType":"image/png","contentLength":bytes.len(),"sha256Hash":STANDARD.encode(Sha256::digest(bytes)),
            "traceId":"fixture","field":"input"
        }).to_string())).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap(),
    )
    .unwrap();
    let url = url::Url::parse(payload["uploadUrl"].as_str().unwrap()).unwrap();
    let bad = app
        .clone()
        .oneshot(Request::put(url.path()).body(Body::from("wrong")).unwrap())
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let good = app
        .clone()
        .oneshot(
            Request::put(url.path())
                .body(Body::from(bytes.as_slice()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(good.status(), StatusCode::OK);
    let result = app
        .oneshot(Request::get(url.path()).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(result.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(result.into_body(), 1_000_000)
            .await
            .unwrap()
            .as_ref(),
        bytes
    );
}
