use crate::{
    model::{TraceFilter, now_ns, parse_time},
    otlp::Batch,
    store::Store,
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::read::GzDecoder;
use percent_encoding::percent_decode_str;
use prost::Message;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Read,
    net::{Ipv4Addr, SocketAddr},
};
use tokio::{net::TcpListener, sync::oneshot};

const MAX_BODY: usize = 32 * 1024 * 1024;
pub type Query = HashMap<String, String>;

pub struct Server {
    pub address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Server {
    pub async fn start(store: Store, port: u16) -> anyhow::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
        let address = listener.local_addr()?;
        let (stop, wait) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router(store))
                .with_graceful_shutdown(async {
                    let _ = wait.await;
                })
                .await;
        });
        Ok(Self {
            address,
            stop: Some(stop),
            task: Some(task),
        })
    }

    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(std::time::Duration::from_secs(2), &mut task)
                .await
                .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

pub fn router(store: Store) -> Router {
    Router::new().fallback(handle).with_state(store)
}

pub(crate) struct Failure(StatusCode, String);
type ApiResult<T> = Result<T, Failure>;

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"message":self.1}))).into_response()
    }
}

impl From<anyhow::Error> for Failure {
    fn from(_: anyhow::Error) -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Local storage operation failed".to_owned(),
        )
    }
}

impl Failure {
    pub(crate) fn bad(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    pub(crate) fn missing() -> Self {
        Self(StatusCode::NOT_FOUND, "Resource not found".to_owned())
    }
    pub(crate) fn unauthorized() -> Self {
        Self(
            StatusCode::UNAUTHORIZED,
            "Valid project credentials required".to_owned(),
        )
    }
}

async fn handle(State(store): State<Store>, request: Request) -> ApiResult<Response> {
    let (parts, body) = request.into_parts();
    let method = parts.method;
    let path = parts.uri.path().trim_end_matches('/');
    if method == Method::GET && matches!(path, "/api/public/health" | "/api/public/ready") {
        return Ok(response(
            json!({"status":"OK","version":env!("CARGO_PKG_VERSION")}),
        ));
    }
    if let Some((format, route)) = path
        .strip_prefix('/')
        .and_then(|value| value.split_once("/v1/"))
        && matches!(format, "openai" | "anthropic")
    {
        return crate::providers::gateway(&store, method, format, route, &parts.headers, body)
            .await;
    }
    if let Some(grant) = path.strip_prefix("/media-object/") {
        return crate::workspace_api::media_transfer(&store, &method, grant, &parts.headers, body)
            .await;
    }
    let path = path
        .strip_prefix("/api/public/")
        .ok_or_else(Failure::missing)?;
    let project = authenticate(&store, &parts.headers)?;
    let query: Query =
        url::form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    if path == "otel/v1/traces" && method == Method::POST {
        let is_json = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"));
        let bytes = read_body(body, &parts.headers).await?;
        let batch = Batch::decode(&bytes, is_json, now_ns())
            .map_err(|_| Failure::bad("Invalid OTLP trace payload"))?;
        store.ingest(&project, &batch.records)?;
        let result = batch.response();
        if is_json {
            return Ok(response(
                serde_json::to_value(&result).map_err(|_| Failure::bad("Invalid OTLP reply"))?,
            ));
        }
        return Ok((
            [(header::CONTENT_TYPE, "application/x-protobuf")],
            result.encode_to_vec(),
        )
            .into_response());
    }
    if path == "ingestion" && method == Method::POST {
        let data = body_json(body, &parts.headers).await?;
        let batch = data
            .get("batch")
            .and_then(Value::as_array)
            .ok_or_else(|| Failure::bad("Expected an ingestion batch"))?;
        let mut successes = Vec::new();
        let mut errors = Vec::new();
        for event in batch {
            let id = event.get("id").cloned().unwrap_or(Value::Null);
            let result = match event.get("type").and_then(Value::as_str) {
                Some("score-create") => {
                    store.put_score(&project, event["body"].clone()).map(|_| ())
                }
                Some("sdk-log") => Ok(()),
                _ => Err(anyhow::anyhow!("Use the OTLP endpoint for traces")),
            };
            match result {
                Ok(()) => successes.push(json!({"id":id,"status":201})),
                Err(error) => {
                    errors.push(json!({"id":id,"status":400,"message":error.to_string()}))
                }
            }
        }
        return Ok((
            StatusCode::MULTI_STATUS,
            Json(json!({"successes":successes,"errors":errors})),
        )
            .into_response());
    }
    dispatch(
        &store,
        &project,
        &method,
        path,
        &query,
        &parts.headers,
        body,
    )
    .await
}

async fn dispatch(
    store: &Store,
    project: &str,
    method: &Method,
    path: &str,
    query: &Query,
    headers: &HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    let raw_segments: Vec<_> = path.split('/').collect();
    let segments = raw_segments
        .iter()
        .map(|value| percent_decode_str(value).decode_utf8_lossy())
        .collect::<Vec<_>>();
    let mut route = segments
        .iter()
        .map(|value| value.as_ref())
        .collect::<Vec<_>>();
    if matches!(route.first(), Some(&"v2" | &"v3")) {
        route.remove(0);
    }
    if path == "v2/observations" && method == Method::GET {
        return observations_v2(store, project, query).map(response);
    }
    let (page, limit, offset) = pagination(query)?;
    let data = match (method, route.as_slice()) {
        (&Method::GET, ["projects"]) => {
            let data=store.projects()?.into_iter().filter(|p|p.id==project).map(|p|json!({
                "id":p.id,"name":p.name,"organization":{"id":"confuse","name":"Local"},"metadata":{}
            })).collect::<Vec<_>>();
            json!({"data":data})
        }
        (&Method::GET, ["traces"]) => {
            let (traces, total) = store.traces(project, &trace_filter(query)?, limit, offset)?;
            let mut rows = Vec::with_capacity(traces.len());
            for trace in traces {
                let mut row = trace.public();
                let observations = store
                    .observations(project, Some(&trace.id), None, 10000, 0)?
                    .0;
                row["observations"] = json!(observations.iter().map(|o| &o.id).collect::<Vec<_>>());
                row["totalCost"] = json!(
                    observations
                        .iter()
                        .filter_map(|o| crate::model::cost_total(&o.cost))
                        .sum::<f64>()
                );
                row["scores"] = json!(
                    store
                        .scores(project, Some(&trace.id), None, 1000, 0)?
                        .0
                        .iter()
                        .map(|s| &s["id"])
                        .collect::<Vec<_>>()
                );
                rows.push(row);
            }
            paged(rows, total, page, limit)
        }
        (&Method::GET, ["traces", id]) => {
            let trace = store.trace(project, id)?.ok_or_else(Failure::missing)?;
            let mut row = trace.public();
            row["observations"] = json!(
                store
                    .observations(project, Some(id), None, 10000, 0)?
                    .0
                    .iter()
                    .map(|o| o.public(project))
                    .collect::<Vec<_>>()
            );
            row["scores"] = json!(store.scores(project, Some(id), None, 1000, 0)?.0);
            row
        }
        (&Method::DELETE, ["traces", id]) => removed(store.delete_trace(project, id)?)?,
        (&Method::GET, ["observations" | "spans" | "generations"]) => {
            let kind = param(query, "type").map(str::to_ascii_uppercase);
            let kind = kind.as_deref().or_else(|| match route[0] {
                "spans" => Some("SPAN"),
                "generations" => Some("GENERATION"),
                _ => None,
            });
            let (data, total) =
                store.observations(project, param(query, "traceId"), kind, limit, offset)?;
            let data = data.iter().map(|o| o.public(project)).collect();
            paged(data, total, page, limit)
        }
        (&Method::GET, ["observations" | "spans" | "generations", id]) => store
            .observation(project, id)?
            .ok_or_else(Failure::missing)?
            .public(project),
        (&Method::GET, ["sessions"]) => {
            let (data, total) = store.sessions(project, limit, offset)?;
            paged(data, total, page, limit)
        }
        (&Method::GET, ["sessions", id]) => {
            let (traces, _) = store.traces(
                project,
                &TraceFilter {
                    session: Some(id),
                    ..Default::default()
                },
                10000,
                0,
            )?;
            let first = traces.last().ok_or_else(Failure::missing)?;
            json!({"id":id,"projectId":project,"createdAt":crate::model::iso(first.start_ns),"environment":first.environment,
                "bookmarked":false,"public":false,"traces":traces.iter().map(|t|t.public()).collect::<Vec<_>>()})
        }
        (&Method::GET, ["scores"]) => {
            let (data, total) = store.scores(
                project,
                param(query, "traceId"),
                param(query, "name"),
                limit,
                offset,
            )?;
            paged(data, total, page, limit)
        }
        (&Method::GET, ["scores", id]) => store.score(project, id)?.ok_or_else(Failure::missing)?,
        (&Method::DELETE, ["scores", id]) => removed(store.delete_score(project, id)?)?,
        (&Method::POST, ["scores"]) => {
            let data = body_json(body, headers).await?;
            if let Some(scores) = data.as_array() {
                let mut accepted = 0;
                let mut errors = Vec::new();
                for score in scores {
                    match store.put_score(project, score.clone()) {
                        Ok(_) => accepted += 1,
                        Err(error) => errors.push(json!({"message":error.to_string()})),
                    }
                }
                if errors.is_empty() {
                    return Ok(
                        (StatusCode::ACCEPTED, Json(json!({"message":"Accepted"}))).into_response()
                    );
                }
                return Ok((
                    StatusCode::MULTI_STATUS,
                    Json(json!({"accepted":accepted,"rejected":errors.len(),"errors":errors})),
                )
                    .into_response());
            }
            let score = store
                .put_score(project, data)
                .map_err(|error| Failure::bad(error.to_string()))?;
            json!({"id":score["id"]})
        }
        _ => {
            return crate::workspace_api::dispatch(
                store, project, method, &route, query, headers, body,
            )
            .await;
        }
    };
    Ok(response(data))
}

fn authenticate(store: &Store, headers: &HeaderMap) -> ApiResult<String> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(name, _)| name.eq_ignore_ascii_case("Basic"))
        .map(|(_, value)| value)
        .ok_or_else(Failure::unauthorized)?;
    let bytes = STANDARD
        .decode(token)
        .map_err(|_| Failure::unauthorized())?;
    let decoded = std::str::from_utf8(&bytes).map_err(|_| Failure::unauthorized())?;
    let (public, secret) = decoded.split_once(':').ok_or_else(Failure::unauthorized)?;
    store
        .project_for_key(public, secret)?
        .ok_or_else(Failure::unauthorized)
}

pub(crate) async fn read_body(body: Body, headers: &HeaderMap) -> ApiResult<Vec<u8>> {
    let bytes = to_bytes(body, MAX_BODY).await.map_err(|_| {
        Failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Payload too large".to_owned(),
        )
    })?;
    let encoding = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("identity");
    match encoding {
        "identity" => Ok(bytes.to_vec()),
        "gzip" => {
            let mut decoder = GzDecoder::new(bytes.as_ref()).take((MAX_BODY as u64) + 1);
            let mut data = Vec::new();
            decoder
                .read_to_end(&mut data)
                .map_err(|_| Failure::bad("Invalid gzip body"))?;
            if data.len() > MAX_BODY {
                return Err(Failure(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Payload too large".to_owned(),
                ));
            }
            Ok(data)
        }
        _ => Err(Failure(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Unsupported content encoding".to_owned(),
        )),
    }
}

pub(crate) async fn body_json(body: Body, headers: &HeaderMap) -> ApiResult<Value> {
    serde_json::from_slice(&read_body(body, headers).await?)
        .map_err(|_| Failure::bad("Invalid JSON body"))
}

pub(crate) fn param<'a>(query: &'a Query, key: &str) -> Option<&'a str> {
    query.get(key).map(String::as_str)
}

pub(crate) fn pagination(query: &Query) -> ApiResult<(usize, usize, usize)> {
    let value = |param_name: &str, fallback: usize| -> ApiResult<usize> {
        param(query, param_name)
            .map(|v| {
                v.parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| Failure::bad(format!("Invalid {param_name}")))
            })
            .unwrap_or(Ok(fallback))
    };
    let page = value("page", 1)?;
    let limit = value("limit", 50)?;
    if limit > 100 {
        return Err(Failure::bad("limit must be between 1 and 100"));
    }
    let offset = (page - 1)
        .checked_mul(limit)
        .filter(|offset| *offset <= i64::MAX as usize)
        .ok_or_else(|| Failure::bad("Invalid page"))?;
    Ok((page, limit, offset))
}

pub(crate) fn trace_filter(query: &Query) -> ApiResult<TraceFilter<'_>> {
    let time = |key| {
        param(query, key)
            .map(|value| {
                parse_time(&json!(value)).ok_or_else(|| Failure::bad(format!("Invalid {key}")))
            })
            .transpose()
    };
    Ok(TraceFilter {
        query: param(query, "search").or_else(|| param(query, "q")),
        session: param(query, "sessionId"),
        name: param(query, "name"),
        user: param(query, "userId"),
        source: param(query, "source"),
        topic: param(query, "topicStableId"),
        from: time("fromTimestamp")?,
        to: time("toTimestamp")?,
    })
}

pub(crate) fn paged(data: Vec<Value>, total: usize, page: usize, limit: usize) -> Value {
    json!({"data":data,"meta":{"page":page,"limit":limit,"totalItems":total,"totalPages":total.div_ceil(limit)}})
}

pub(crate) fn removed(exists: bool) -> ApiResult<Value> {
    if !exists {
        return Err(Failure::missing());
    }
    Ok(json!({"success":true}))
}

pub(crate) fn response(data: Value) -> Response {
    Json(data).into_response()
}

fn observations_v2(store: &Store, project: &str, query: &Query) -> ApiResult<Value> {
    if param(query, "parseIoAsJson") == Some("true") {
        return Err(Failure::bad("parseIoAsJson is no longer supported"));
    }
    let limit = param(query, "limit")
        .map(|value| {
            value
                .parse::<usize>()
                .ok()
                .filter(|limit| (1..=1000).contains(limit))
                .ok_or_else(|| Failure::bad("Invalid limit"))
        })
        .unwrap_or(Ok(50))?;
    let offset = param(query, "cursor")
        .map(|value| {
            let bytes = STANDARD
                .decode(value)
                .map_err(|_| Failure::bad("Invalid cursor"))?;
            let cursor = std::str::from_utf8(&bytes).map_err(|_| Failure::bad("Invalid cursor"))?;
            let (scope, offset) = cursor
                .rsplit_once(':')
                .ok_or_else(|| Failure::bad("Invalid cursor"))?;
            if scope != project {
                return Err(Failure::bad("Invalid cursor"));
            }
            offset
                .parse::<usize>()
                .ok()
                .filter(|offset| *offset <= i64::MAX as usize - limit)
                .ok_or_else(|| Failure::bad("Invalid cursor"))
        })
        .transpose()?
        .unwrap_or_default();
    let structured = param(query, "filter")
        .map(|v| {
            serde_json::from_str::<Value>(v).map_err(|_| Failure::bad("Invalid observation filter"))
        })
        .transpose()?;
    let kind = param(query, "type").map(str::to_ascii_uppercase);
    let trace_id = structured
        .as_ref()
        .is_none()
        .then(|| param(query, "traceId"))
        .flatten();
    let kind = structured
        .as_ref()
        .is_none()
        .then_some(kind.as_deref())
        .flatten();
    let (rows, total) = store.observations(project, trace_id, kind, limit, offset)?;
    let next = offset + rows.len();
    let cursor = (next < total).then(|| STANDARD.encode(format!("{project}:{next}")));
    let groups = param(query, "fields")
        .unwrap_or("basic")
        .split(',')
        .collect::<Vec<_>>();
    let parse_date = |key| {
        param(query, key)
            .map(|v| parse_time(&json!(v)).ok_or_else(|| Failure::bad(format!("Invalid {key}"))))
            .transpose()
    };
    let from = parse_date("fromStartTime")?;
    let to = parse_date("toStartTime")?;
    let mut data = Vec::new();
    for row in rows {
        let mut all = row.public(project);
        if let Some(trace) = store.trace(project, &row.trace_id)? {
            all["userId"] = json!(trace.user_id);
            all["sessionId"] = json!(trace.session_id);
            all["traceName"] = json!(trace.name);
            all["tags"] = json!(trace.tags);
            all["release"] = json!(trace.release);
            all["createdAt"] = json!(crate::model::iso(trace.received_ns));
            all["updatedAt"] = all["createdAt"].clone();
        }
        all["isRootObservation"] = json!(row.parent_id.is_none());
        if let Some(filter) = &structured {
            if !crate::resources::matches_filters(&all, filter) {
                continue;
            }
        } else {
            if [
                "name",
                "userId",
                "sessionId",
                "version",
                "environment",
                "level",
            ]
            .iter()
            .any(|key| param(query, key).is_some_and(|value| all[*key] != value))
            {
                continue;
            }
            if param(query, "parentObservationId").is_some_and(|value| {
                if value.is_empty() {
                    row.parent_id.is_some()
                } else {
                    row.parent_id.as_deref() != Some(value)
                }
            }) {
                continue;
            }
            if param(query, "isRootObservation")
                .is_some_and(|value| (value == "true") != row.parent_id.is_none())
            {
                continue;
            }
        }
        if from.is_some_and(|time| row.start_ns < time)
            || to.is_some_and(|time| row.start_ns >= time)
        {
            continue;
        }
        const GROUPS: &[(&str, &[&str])] = &[
            (
                "basic",
                &[
                    "name",
                    "level",
                    "statusMessage",
                    "version",
                    "environment",
                    "bookmarked",
                    "public",
                    "userId",
                    "sessionId",
                    "isRootObservation",
                ],
            ),
            ("time", &["completionStartTime", "createdAt", "updatedAt"]),
            ("io", &["input", "output"]),
            ("metadata", &["metadata"]),
            ("model", &["model", "internalModelId", "modelParameters"]),
            (
                "usage",
                &[
                    "usageDetails",
                    "costDetails",
                    "totalCost",
                    "usagePricingTierName",
                ],
            ),
            ("prompt", &["promptId", "promptName", "promptVersion"]),
            ("metrics", &["latency", "timeToFirstToken"]),
            ("trace_context", &["tags", "release", "traceName"]),
        ];
        let mut output = serde_json::Map::new();
        for key in [
            "id",
            "traceId",
            "startTime",
            "endTime",
            "projectId",
            "parentObservationId",
            "type",
        ]
        .into_iter()
        .chain(
            GROUPS
                .iter()
                .filter(|(name, _)| groups.contains(name))
                .flat_map(|(_, keys)| keys.iter().copied()),
        ) {
            let mut value = all.get(key).cloned().unwrap_or_default();
            if matches!(key, "input" | "output") && !value.is_null() {
                value = json!(value.to_string());
            }
            output.insert(key.to_owned(), value);
        }
        data.push(Value::Object(output));
    }
    Ok(json!({"data":data,"meta":{"cursor":cursor}}))
}
