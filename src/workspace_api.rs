use crate::{
    api::{Failure, Query, body_json, paged, pagination, param, read_body, removed, response},
    model::{iso, now_ns, stable_id},
    resources::{KINDS, matches_filters},
    store::Store,
    topics,
};
use anyhow::{Context, Result};
use axum::{
    body::Body,
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
};
use uuid::Uuid;

pub(crate) async fn dispatch(
    store: &Store,
    project: &str,
    method: &Method,
    route: &[&str],
    query: &Query,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, Failure> {
    let Some((&kind, tail)) = route.split_first() else {
        return Err(Failure::missing());
    };
    let (page, limit, offset) = pagination(query)?;
    let search_filters = param(query, "filter")
        .map(|value| {
            serde_json::from_str::<Value>(value).map_err(|_| Failure::bad("Invalid filter"))
        })
        .transpose()?;
    let result = match (method, kind, tail) {
        (&Method::GET, "metrics", []) => {
            let request = param(query, "query").ok_or_else(|| Failure::bad("query is required"))?;
            let request =
                serde_json::from_str(request).map_err(|_| Failure::bad("Invalid metrics query"))?;
            store.metrics(project, &request)?
        }
        (&Method::GET, "unstable", [kind]) if is_topic(kind) => topics_list(
            store,
            project,
            kind,
            query,
            search_filters.as_ref(),
            page,
            limit,
            offset,
        )?,
        (&Method::GET, "unstable", [kind, id]) if is_topic(kind) => {
            let mut item = topics::public_data(store, project, kind)?
                .into_iter()
                .find(|value| value["id"] == *id || value["stableId"] == *id)
                .ok_or_else(Failure::missing)?;
            if *kind == "topics" {
                let (_, _, window) = topic_window(query)?;
                item["window"] = window;
                item["inCurrentMap"] = json!(true);
            }
            item
        }
        (&Method::POST, "mcp", []) => {
            let request = body_json(body, headers).await?;
            if request.get("id").is_none() {
                return Ok(StatusCode::ACCEPTED.into_response());
            }
            mcp(store, project, &request)?
        }
        (&Method::POST, "media", []) => {
            let request = body_json(body, headers).await?;
            media_create(store, project, &request, headers)?
        }
        (&Method::GET, "media", [id]) => media_get(store, project, id, headers)?,
        (&Method::PATCH, "media", [id]) => {
            let data = body_json(body, headers).await?;
            let updated = data["uploadedAt"]
                .as_str()
                .ok_or_else(|| Failure::bad("uploadedAt is required"))?;
            media_uploaded(store, project, id, updated)?;
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        (&Method::GET, "skills", ["files", "content"]) => {
            let hashes = param(query, "sha256Hashes")
                .ok_or_else(|| Failure::bad("sha256Hashes is required"))?;
            json!({"data":crate::skills::contents(store,project,hashes).map_err(input_error)?})
        }
        (&Method::GET, "skills", []) => {
            let data = filter(
                crate::skills::versions(store, project)?,
                query,
                search_filters.as_ref(),
            );
            let mut result = paginate(data, page, limit, offset);
            result["meta"]["hasNextPage"] =
                json!(page < result["meta"]["totalPages"].as_u64().unwrap_or_default() as usize);
            result
        }
        (&Method::POST, "skills", []) => {
            let data = body_json(body, headers).await?;
            let result = store
                .put_resource(project, "skills", data)
                .map_err(input_error)?;
            return Ok((
                StatusCode::CREATED,
                axum::Json(crate::skills::manifest(result, false)),
            )
                .into_response());
        }
        (&Method::PATCH, "skills", [name]) => {
            let data = body_json(body, headers).await?;
            let result = crate::skills::update_tags(
                store,
                project,
                name,
                crate::resources::string_list(&data["tags"]),
            )
            .map_err(|_| Failure::missing())?;
            crate::skills::manifest(result, false)
        }
        (&Method::GET, "prompts", []) => {
            let items = filter(
                store.resources(project, kind)?,
                query,
                search_filters.as_ref(),
            );
            let mut grouped = BTreeMap::new();
            for item in items {
                let Some(name) = item["name"].as_str() else {
                    continue;
                };
                let value=grouped.entry(name.to_owned()).or_insert_with(||json!({
                    "name":name,"versions":[],"labels":[],"tags":item["tags"],"lastUpdatedAt":item["updatedAt"]
                }));
                value["versions"]
                    .as_array_mut()
                    .expect("initialized array")
                    .push(item["version"].clone());
                for label in item["labels"].as_array().into_iter().flatten() {
                    let values = value["labels"].as_array_mut().expect("initialized array");
                    if !values.contains(label) {
                        values.push(label.clone());
                    }
                }
            }
            paginate(grouped.into_values().collect(), page, limit, offset)
        }
        (&Method::GET, "prompts" | "skills", [name]) => {
            let version = param(query, "version")
                .map(str::parse::<i64>)
                .transpose()
                .map_err(|_| Failure::bad("Invalid version"))?;
            let result = store
                .versioned(project, kind, name, version, param(query, "label"))?
                .ok_or_else(Failure::missing)?;
            if kind == "skills" {
                crate::skills::manifest(result, param(query, "includeContent") == Some("true"))
            } else {
                result
            }
        }
        (&Method::PATCH, "prompts" | "skills", [name, "versions", version])
        | (&Method::PATCH, "prompt-versions", [name, version]) => {
            let data = body_json(body, headers).await?;
            store
                .set_version_labels(
                    project,
                    if kind == "skills" { kind } else { "prompts" },
                    name,
                    version
                        .parse()
                        .map_err(|_| Failure::bad("Invalid version"))?,
                    &crate::resources::string_list(&data["labels"]),
                )
                .map_err(|_| Failure::missing())?
        }
        (&Method::DELETE, "skills", [name, "versions", version]) => {
            let value = store
                .versioned(
                    project,
                    kind,
                    name,
                    Some(
                        version
                            .parse()
                            .map_err(|_| Failure::bad("Invalid version"))?,
                    ),
                    None,
                )?
                .ok_or_else(Failure::missing)?;
            let deleted = store.delete_resource(
                project,
                kind,
                value["id"].as_str().ok_or_else(Failure::missing)?,
            )?;
            json!({"deleted":deleted})
        }
        (&Method::GET, "annotation-queues", [queue, "items"]) => {
            let data = store
                .resources(project, "annotation-items")?
                .into_iter()
                .filter(|value| value["queueId"] == *queue)
                .collect();
            paginate(data, page, limit, offset)
        }
        (&Method::GET, "annotation-queues", [queue, "items", id]) => store
            .resource(project, "annotation-items", id)?
            .filter(|value| value["queueId"] == *queue)
            .ok_or_else(Failure::missing)?,
        (&Method::POST, "annotation-queues", [queue, "items"])
        | (&Method::PATCH, "annotation-queues", [queue, "items", _]) => {
            if store.resource(project, kind, queue)?.is_none() {
                return Err(Failure::missing());
            }
            let mut data = body_json(body, headers).await?;
            data["queueId"] = json!(queue);
            if let Some(id) = tail.get(2) {
                data["id"] = json!(id);
            }
            store.put_resource(project, "annotation-items", data)?
        }
        (&Method::GET, resource, []) if expose(resource) => {
            let rows = filter(
                store.resources(project, resource)?,
                query,
                search_filters.as_ref(),
            );
            paginate(rows, page, limit, offset)
        }
        (&Method::GET, resource, [id]) if expose(resource) => store
            .resource(project, resource, id)?
            .ok_or_else(Failure::missing)?,
        (&Method::POST, resource, []) if expose(resource) => {
            let data = body_json(body, headers).await?;
            if resource == "llm-connections" {
                store
                    .configure_provider(project, data)
                    .map_err(input_error)?
            } else {
                store
                    .put_resource(project, resource, data)
                    .map_err(input_error)?
            }
        }
        (&Method::PATCH, resource, [id]) if expose(resource) => {
            let mut data = body_json(body, headers).await?;
            let old = store
                .resource(project, resource, id)?
                .ok_or_else(Failure::missing)?;
            data["id"] = old["id"].clone();
            if resource == "llm-connections" {
                let mut config = old;
                config.as_object_mut().ok_or_else(Failure::missing)?.extend(
                    data.as_object()
                        .ok_or_else(|| Failure::bad("Expected a JSON object"))?
                        .clone(),
                );
                store
                    .configure_provider(project, config)
                    .map_err(input_error)?
            } else {
                store
                    .put_resource(project, resource, data)
                    .map_err(input_error)?
            }
        }
        (&Method::DELETE, resource, [id]) if expose(resource) => {
            let changed = if resource == "llm-connections" {
                store.remove_provider(project, id)?
            } else {
                store.delete_resource(project, resource, id)?
            };
            removed(changed)?
        }
        _ => return Err(Failure::missing()),
    };
    Ok(response(result))
}

fn input_error(error: anyhow::Error) -> Failure {
    Failure::bad(error.to_string())
}
fn expose(resource: &str) -> bool {
    KINDS.contains(&resource)
        && !matches!(
            resource,
            "topic-maps" | "topic-runs" | "topic-publications" | "exports"
        )
}
fn is_topic(resource: &str) -> bool {
    matches!(
        resource,
        "topic-facets" | "topic-rules" | "topics" | "topic-summaries" | "topic-assignments"
    )
}

fn paginate(data: Vec<Value>, page: usize, limit: usize, offset: usize) -> Value {
    let total = data.len();
    paged(
        data.into_iter().skip(offset).take(limit).collect(),
        total,
        page,
        limit,
    )
}

fn filter(data: Vec<Value>, query: &Query, filters: Option<&Value>) -> Vec<Value> {
    data.into_iter()
        .filter(|row| {
            filters.is_none_or(|value| matches_filters(row, value))
                && [
                    "datasetName",
                    "datasetId",
                    "traceId",
                    "name",
                    "facetId",
                    "ruleId",
                    "topicStableId",
                ]
                .into_iter()
                .all(|key| param(query, key).is_none_or(|value| row[key] == value))
        })
        .collect()
}

fn mcp(store: &Store, project: &str, request: &Value) -> Result<Value> {
    let id = request["id"].clone();
    let arguments = &request["params"]["arguments"];
    let mut tools = vec![
        json!({"name":"listTopics","description":"List current live Topics","inputSchema":{"type":"object","properties":{}}}),
        json!({"name":"listTopicRules","description":"List configured live map rules","inputSchema":{"type":"object","properties":{}}}),
        json!({"name":"listTopicFacets","description":"List built-in Topics facets","inputSchema":{"type":"object","properties":{}}}),
        json!({"name":"listTopicSummaries","description":"Read summaries","inputSchema":{"type":"object","properties":{}}}),
        json!({"name":"listTopicAssignments","description":"Read trace assignments","inputSchema":{"type":"object","properties":{}}}),
    ];
    tools.push(json!({"name":"getTrace","description":"Read a trace and observations",
        "inputSchema":{"type":"object","required":["traceId"],"properties":{"traceId":{"type":"string"}}}}));
    let result = match request["method"].as_str() {
        Some("initialize") => {
            json!({"protocolVersion":request["params"]["protocolVersion"].as_str().unwrap_or("2025-03-26"),
            "capabilities":{"tools":{}},"serverInfo":{"name":"Confuse","version":env!("CARGO_PKG_VERSION")}})
        }
        Some("tools/list") => json!({"tools":tools}),
        Some("ping") => json!({}),
        Some("tools/call") => {
            let name = request["params"]["name"].as_str().unwrap_or_default();
            let data = if name == "getTrace" {
                let id = arguments["traceId"]
                    .as_str()
                    .context("traceId is required")?;
                let trace = store.trace(project, id)?.context("Trace not found")?;
                let observations = store
                    .observations(project, Some(id), None, 1000, 0)?
                    .0
                    .iter()
                    .map(|row| row.public(project))
                    .collect::<Vec<_>>();
                json!({"trace":trace.public(),"observations":observations})
            } else {
                let resource = match name {
                    "listTopics" => "topics",
                    "listTopicFacets" => "topic-facets",
                    "listTopicRules" => "topic-rules",
                    "listTopicSummaries" => "topic-summaries",
                    "listTopicAssignments" => "topic-assignments",
                    _ => {
                        return Ok(
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Unknown tool"}}),
                        );
                    }
                };
                json!(topics::public_data(store, project, resource)?)
            };
            json!({"content":[{"type":"text","text":data.to_string()}],"isError":false})
        }
        _ => {
            return Ok(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unknown method"}}),
            );
        }
    };
    Ok(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

fn address(store: &Store, headers: &HeaderMap) -> Result<String> {
    if let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) {
        if let Ok(address) = host.parse::<SocketAddr>()
            && address.ip().is_loopback()
        {
            return Ok(format!("http://{address}"));
        }
        if let Some((name, port)) = host.split_once(':')
            && name.eq_ignore_ascii_case("localhost")
            && port.parse::<u16>().is_ok()
        {
            return Ok(format!("http://127.0.0.1:{port}"));
        }
        if host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
            return Ok(format!("http://{host}"));
        }
    }
    Ok(format!("http://127.0.0.1:{}", store.endpoint_port()?))
}

fn media_create(
    store: &Store,
    project: &str,
    data: &Value,
    headers: &HeaderMap,
) -> Result<Value, Failure> {
    let mime = data["contentType"]
        .as_str()
        .filter(|value| {
            value.starts_with("image/")
                || value.starts_with("audio/")
                || value.starts_with("video/")
                || *value == "application/pdf"
        })
        .ok_or_else(|| Failure::bad("Invalid media type"))?;
    let size = data["contentLength"]
        .as_i64()
        .filter(|size| *size >= 0 && *size <= 32 * 1024 * 1024)
        .ok_or_else(|| Failure::bad("Invalid media size"))?;
    let digest = data["sha256Hash"]
        .as_str()
        .filter(|digest| !digest.is_empty())
        .ok_or_else(|| Failure::bad("sha256Hash is required"))?;
    let origin = address(store, headers)?;
    let trace = data["traceId"].as_str();
    if trace.is_none() && data["datasetItemId"].as_str().is_none() {
        return Err(Failure::bad("Provide a trace or dataset item"));
    }
    let id = stable_id(
        &[
            project,
            trace.unwrap_or_default(),
            digest,
            data["datasetItemId"].as_str().unwrap_or_default(),
        ],
        16,
    );
    let grant = Uuid::new_v4().to_string();
    let expires = now_ns() + 3_600_000_000_000;
    store.transaction(|tx|{
        tx.execute("INSERT INTO media(project,id,trace,mime,size,digest,grant,expires_ns,bytes)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL)
            ON CONFLICT(project,id) DO UPDATE SET grant=excluded.grant,expires_ns=excluded.expires_ns",
            params![project,id,trace,mime,size,digest,grant,expires])?;
        let uploaded:bool=tx.query_row("SELECT bytes IS NOT NULL FROM media WHERE project=?1 AND id=?2",params![project,id],|r|r.get(0))?;
        Ok(json!({"mediaId":id,"uploadUrl":(!uploaded).then(||format!("{origin}/media-object/{grant}"))}))
    }).map_err(Into::into)
}

fn media_get(
    store: &Store,
    project: &str,
    id: &str,
    headers: &HeaderMap,
) -> Result<Value, Failure> {
    let origin = address(store, headers)?;
    let grant = Uuid::new_v4().to_string();
    let expires = now_ns() + 3_600_000_000_000;
    let result=store.transaction(|tx|{
        let item=tx.query_row("SELECT mime,size,uploaded_at FROM media WHERE project=?1 AND id=?2 AND bytes IS NOT NULL",params![project,id],|r|
            Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?))).optional()?;
        if item.is_some(){
            tx.execute("UPDATE media SET grant=?3,expires_ns=?4 WHERE project=?1 AND id=?2",params![project,id,grant,expires])?;
        }
        Ok(item)
    })?.ok_or_else(Failure::missing)?;
    Ok(
        json!({"mediaId":id,"contentType":result.0,"contentLength":result.1,
        "uploadedAt":result.2.unwrap_or_else(||iso(now_ns())),
        "url":format!("{origin}/media-object/{grant}"),"urlExpiry":iso(expires)}),
    )
}

fn media_uploaded(
    store: &Store,
    project: &str,
    id: &str,
    mut_updated: &str,
) -> Result<(), Failure> {
    let updated = store.access(|c| {
        Ok(c.execute(
            "UPDATE media SET uploaded_at=?3 WHERE project=?1 AND id=?2",
            params![project, id, mut_updated],
        )?)
    })?;
    if updated == 0 {
        return Err(Failure::missing());
    }
    Ok(())
}

pub(crate) async fn media_transfer(
    store: &Store,
    method: &Method,
    grant: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, Failure> {
    let row = store
        .access(|conn| {
            Ok(conn
                .query_row(
                    "SELECT mime,size,digest,bytes FROM media WHERE grant=?1 AND expires_ns>?2",
                    params![grant, now_ns()],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, i64>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, Option<Vec<u8>>>(3)?,
                        ))
                    },
                )
                .optional()?)
        })?
        .ok_or_else(Failure::missing)?;
    if method == Method::GET {
        let mime = header::HeaderValue::from_str(&row.0).map_err(|_| Failure::missing())?;
        return Ok((
            [(header::CONTENT_TYPE, mime)],
            row.3.ok_or_else(Failure::missing)?,
        )
            .into_response());
    }
    if method != Method::PUT || row.3.is_some() {
        return Err(Failure::missing());
    }
    let data = read_body(body, headers).await?;
    let digest = Sha256::digest(&data);
    if data.len() as i64 != row.1
        || (hex::encode(digest) != row.2 && STANDARD.encode(digest) != row.2)
    {
        return Err(Failure::bad("Media checksum or size mismatch"));
    }
    store.access(|conn| {
        conn.execute(
            "UPDATE media SET bytes=?2,uploaded_at=?3 WHERE grant=?1 AND bytes IS NULL",
            params![grant, data, iso(now_ns())],
        )?;
        Ok(())
    })?;
    Ok(StatusCode::OK.into_response())
}

fn topic_window(query: &Query) -> Result<(i64, i64, Value), Failure> {
    const DAY: i64 = 86_400_000_000_000;
    let parse = |name| {
        param(query, name)
            .map(|value| {
                crate::model::parse_time(&json!(value))
                    .ok_or_else(|| Failure::bad(format!("Invalid {name}")))
            })
            .transpose()
    };
    let to = parse("toTimestamp")?.unwrap_or_else(now_ns);
    let from = parse("fromTimestamp")?.unwrap_or(to - 7 * DAY);
    if from >= to || to.saturating_sub(from) > 93 * DAY {
        return Err(Failure::bad("Invalid time window"));
    }
    Ok((from, to, json!({"from":iso(from),"to":iso(to)})))
}

#[allow(clippy::too_many_arguments)]
fn topics_list(
    store: &Store,
    project: &str,
    kind: &str,
    query: &Query,
    filters: Option<&Value>,
    page: usize,
    limit: usize,
    offset: usize,
) -> Result<Value, Failure> {
    let data = filter(topics::public_data(store, project, kind)?, query, filters);
    if kind == "topic-rules" || kind == "topic-facets" {
        let active_condition = filters
            .and_then(Value::as_array)
            .is_some_and(|filters| filters.iter().any(|row| row["column"] == "active"));
        let data = data
            .into_iter()
            .filter(|row| kind != "topic-facets" || active_condition || row["active"] == true)
            .collect();
        return Ok(paginate(data, page, limit, offset));
    }
    let (from, to, window) = topic_window(query)?;
    let in_window = |row: &Value| {
        row.get("timestamp")
            .and_then(crate::model::parse_time)
            .is_none_or(|timestamp| timestamp >= from && timestamp < to)
    };
    if kind == "topics" {
        let mut assignments = BTreeMap::new();
        let mut facets = BTreeMap::new();
        for row in topics::public_data(store, project, "topic-assignments")?
            .into_iter()
            .filter(in_window)
        {
            if let Some(id) = row["topicStableId"].as_str() {
                *assignments.entry(id.to_owned()).or_insert(0_usize) += 1;
            }
            let facet = row["facetId"].as_str().unwrap_or_default();
            let rule = row["ruleId"].as_str().unwrap_or_default();
            let entry=facets.entry((facet.to_owned(),rule.to_owned())).or_insert_with(||json!({
                "facet":{"id":facet,"name":row["facetName"],"version":1},"ruleId":rule,
                "publishedMapId":row["mapId"],"counts":{"assigned":0,"outlier":0,"awaitingMap":0,
                    "notApplicable":0,"insufficientInput":0,"emptyInput":0,"inputTooLarge":0}
            }));
            let key = if row["status"] == "assigned" {
                "assigned"
            } else {
                "outlier"
            };
            entry["counts"][key] = json!(entry["counts"][key].as_u64().unwrap_or_default() + 1);
        }
        let mut data: Vec<_> = data
            .into_iter()
            .map(|mut row| {
                row["traceCount"] = json!(
                    assignments
                        .get(row["stableId"].as_str().unwrap_or_default())
                        .copied()
                        .unwrap_or_default()
                );
                row
            })
            .collect();
        data.sort_by(|a, b| match param(query, "orderBy") {
            Some("name.asc") => a["name"].as_str().cmp(&b["name"].as_str()),
            Some("name.desc") => b["name"].as_str().cmp(&a["name"].as_str()),
            Some("traceCount.asc") => a["traceCount"].as_u64().cmp(&b["traceCount"].as_u64()),
            _ => b["traceCount"].as_u64().cmp(&a["traceCount"].as_u64()),
        });
        let mut response = paginate(data, page, limit, offset);
        response["window"] = window;
        response["facets"] = json!(facets.into_values().collect::<Vec<_>>());
        return Ok(response);
    }
    let mut data: Vec<_> = data.into_iter().filter(in_window).collect();
    data.sort_by(|a, b| {
        if param(query, "orderBy") == Some("timestamp.asc") {
            a["timestamp"].as_str().cmp(&b["timestamp"].as_str())
        } else {
            b["timestamp"].as_str().cmp(&a["timestamp"].as_str())
        }
    });
    let offset = param(query, "cursor")
        .map(|cursor| {
            let raw = STANDARD
                .decode(cursor)
                .map_err(|_| Failure::bad("Invalid cursor"))?;
            let cursor = std::str::from_utf8(&raw).map_err(|_| Failure::bad("Invalid cursor"))?;
            let (prefix, count) = cursor
                .rsplit_once(':')
                .ok_or_else(|| Failure::bad("Invalid cursor"))?;
            if prefix != format!("{project}:{kind}") {
                return Err(Failure::bad("Invalid cursor"));
            }
            count
                .parse::<usize>()
                .ok()
                .filter(|offset| *offset <= usize::MAX - limit)
                .ok_or_else(|| Failure::bad("Invalid cursor"))
        })
        .transpose()?
        .unwrap_or_default();
    let next = offset + limit;
    let total = data.len();
    let rows = data
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    Ok(
        json!({"window":window,"data":rows,"meta":{"cursor":(next<total).then(||STANDARD.encode(format!("{project}:{kind}:{next}")))}}),
    )
}
