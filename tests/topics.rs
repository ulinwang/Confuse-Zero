use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    routing::post,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use confuse::{
    api,
    model::{Observation, Record, Trace, TraceFilter, iso},
    store::{DEFAULT_PROJECT, Store},
    topics,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::net::TcpListener;
use tower::ServiceExt;

const NOW: i64 = 1_790_000_000_000_000_000;

#[derive(Default)]
struct Calls {
    summary: AtomicUsize,
    embed: AtomicUsize,
    naming: AtomicUsize,
}

async fn complete(State(calls): State<Arc<Calls>>, Json(body): Json<Value>) -> Json<Value> {
    let prompt = body["messages"][0]["content"].as_str().unwrap_or_default();
    let text = if prompt.starts_with("Return a short topic name") {
        calls.naming.fetch_add(1, Ordering::Relaxed);
        "Workflow\nA repeated workflow".to_owned()
    } else {
        calls.summary.fetch_add(1, Ordering::Relaxed);
        let marker = if prompt.contains("Publish") {
            "Publish"
        } else {
            "Import"
        };
        let suffix = prompt.bytes().map(usize::from).sum::<usize>() % 17;
        format!("{marker} files {suffix}")
    };
    Json(
        json!({"choices":[{"message":{"content":text}}],"usage":{"prompt_tokens":1,"completion_tokens":2}}),
    )
}

async fn embeddings(State(calls): State<Arc<Calls>>, Json(body): Json<Value>) -> Json<Value> {
    calls.embed.fetch_add(1, Ordering::Relaxed);
    let summary = body["input"].as_str().unwrap_or_default();
    let dimension = body["dimensions"].as_u64().unwrap_or(64) as usize;
    let mut vector = vec![0.01; dimension];
    let cluster = if summary.contains("Publish") { 31 } else { 3 };
    let noise = (summary.bytes().map(usize::from).sum::<usize>() % 19) as f64;
    vector[cluster] = 1.0 + noise * 0.001;
    vector[cluster + 1] = noise * 0.0327;
    Json(json!({"data":[{"embedding":vector}]}))
}

fn trace(i: usize, age_secs: i64) -> Record {
    let id = format!("{i:032x}");
    let kind = if i.is_multiple_of(2) {
        "Publish"
    } else {
        "Import"
    };
    let source = if i % 4 < 2 { "SDK" } else { "Pi" };
    let at = NOW - age_secs * 1_000_000_000;
    let input = json!(format!("{kind} files {i}"));
    let output = json!(format!("Completed task {i}"));
    Record {
        trace: Trace {
            id: id.clone(),
            name: kind.to_owned(),
            source: source.to_owned(),
            input: input.clone(),
            output: output.clone(),
            start_ns: at,
            end_ns: at + 20,
            received_ns: at + 20,
            environment: "default".to_owned(),
            ..Default::default()
        },
        observation: Some(Observation {
            id: format!("{i:016x}"),
            trace_id: id,
            kind: "GENERATION".to_owned(),
            name: "generation".to_owned(),
            input,
            output,
            start_ns: at,
            end_ns: at + 20,
            ..Default::default()
        }),
        root: true,
    }
}

#[tokio::test]
async fn topics_runs_local_models_keeps_snapshots_and_resolves_project_scoped_results() {
    let store = Store::memory().unwrap();
    let calls = Arc::new(Calls::default());
    let server = Router::new()
        .route("/v1/chat/completions", post(complete))
        .route("/v1/embeddings", post(embeddings))
        .with_state(calls.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    let provider=store.configure_provider(DEFAULT_PROJECT,json!({
        "name":"fixture","model":"summary","embeddingModel":"vector","format":"openai","baseUrl":format!("http://{address}/v1")
    })).unwrap();
    let mut traces: Vec<_> = (1..=24).map(|index| trace(index, 600)).collect();
    traces.push(trace(25, 60));
    store.ingest(DEFAULT_PROJECT, &traces).unwrap();
    let live=topics::save(&store,DEFAULT_PROJECT,json!({
        "name":"Live","mode":"live","facetIds":["intent","sentiment","outcome","issues"],"summaryProvider":provider["id"],
        "embeddingProvider":provider["id"],"namingProvider":provider["id"],"dimensions":64,"sample":100
    })).unwrap();
    let id = live["id"].as_str().unwrap();
    let run = topics::process(&store, DEFAULT_PROJECT, live.clone(), NOW)
        .await
        .unwrap();
    assert_eq!(run["status"], "Complete");
    assert_eq!(calls.summary.load(Ordering::Relaxed), 96);
    assert_eq!(calls.embed.load(Ordering::Relaxed), 96);
    assert_eq!(
        topics::result(&store, DEFAULT_PROJECT, id, Some("intent")).unwrap()["assignments"]
            .as_array()
            .unwrap()
            .len(),
        24
    );

    let mut snapshot_data = live.clone();
    snapshot_data.as_object_mut().unwrap().remove("id");
    snapshot_data["name"] = json!("Snapshot");
    snapshot_data["mode"] = json!("snapshot");
    snapshot_data["fromTimestamp"] = json!(iso(NOW - 1_000_000_000_000));
    snapshot_data["toTimestamp"] = json!(iso(NOW - 90_000_000_000));
    let snapshot = topics::save(&store, DEFAULT_PROJECT, snapshot_data).unwrap();
    let snapshot_id = snapshot["id"].as_str().unwrap();
    topics::process(&store, DEFAULT_PROJECT, snapshot.clone(), NOW)
        .await
        .unwrap();
    assert_eq!(
        calls.summary.load(Ordering::Relaxed),
        96,
        "snapshot reuses compatible summaries"
    );
    assert!(topics::change(&store, DEFAULT_PROJECT, snapshot_id, "refresh").is_err());

    let key = store.create_key(DEFAULT_PROJECT, "sdk").unwrap();
    let foreign = store.create_project("foreign").unwrap();
    let other = store.create_key(&foreign.id, "sdk").unwrap();
    let router = api::router(store.clone());
    let uri = format!(
        "/api/public/unstable/topic-assignments?limit=100&fromTimestamp={}&toTimestamp={}",
        iso(NOW - 2_000_000_000_000),
        iso(NOW)
    );
    for (key, expected) in [(key, 96), (other, 0)] {
        let auth = STANDARD.encode(format!("{}:{}", key.public_key, key.secret_key));
        let response = router
            .clone()
            .oneshot(
                Request::get(&uri)
                    .header("authorization", format!("Basic {auth}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["data"].as_array().unwrap().len(), expected);
        assert!(result["meta"]["cursor"].is_null());
    }

    let key = store.create_key(DEFAULT_PROJECT, "sdk").unwrap();
    let authorization = format!(
        "Basic {}",
        STANDARD.encode(format!("{}:{}", key.public_key, key.secret_key))
    );
    let listing = router
        .clone()
        .oneshot(
            Request::post("/api/public/mcp")
                .header("authorization", &authorization)
                .body(Body::from(
                    json!({"id":1,"method":"tools/list","jsonrpc":"2.0"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listing.status(), StatusCode::OK);
    let listing: Value = serde_json::from_slice(
        &axum::body::to_bytes(listing.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        listing["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "listTopicAssignments")
    );
    let reply=router.oneshot(Request::post("/api/public/mcp").header("authorization",&authorization)
        .body(Body::from(json!({"id":2,"method":"tools/call","params":{"name":"listTopicFacets"},"jsonrpc":"2.0"}).to_string())).unwrap()).await.unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    let reply: Value = serde_json::from_slice(
        &axum::body::to_bytes(reply.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains(r#""intent""#)
    );
    let first = traces[0].trace.id.clone();
    assert!(store.delete_trace(DEFAULT_PROJECT, &first).unwrap());
    let active = topics::result(&store, DEFAULT_PROJECT, id, Some("intent")).unwrap();
    assert_eq!(active["assignments"].as_array().unwrap().len(), 23);
    assert!(
        active["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["traceId"] != first)
    );
    let trace_filter = TraceFilter::default();
    assert_eq!(
        store
            .traces(DEFAULT_PROJECT, &trace_filter, 100, 0)
            .unwrap()
            .1,
        24
    );
    server.abort();
}
