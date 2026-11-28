use super::{fail, topic_setup, tracing::includes};
use crate::{
    desktop::required,
    model::{Trace, TraceFilter, iso, now_ns, parse_time},
    resources::string_list,
    store::Store,
    topics,
};
use anyhow::{Context, Result, bail};
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const MAPS: &str = "topic-maps";
const RUNS: &str = "topic-runs";
const DAY: i64 = 86_400_000_000_000;
const SECOND: i64 = 1_000_000_000;

pub(super) async fn request(
    store: &Store,
    project: &str,
    name: &str,
    input: &Value,
) -> Result<Value> {
    let result = match name {
        "topics.facets" => json!(topics::facet_versions(store, project)?),
        "topics.filterOptions" => filter_options(store, project)?,
        "topics.stripeMap" => stripe_map(store, project, required(input, "facet")?)?,
        "topics.maps" => {
            let mut rules = Vec::new();
            let mut analyses = Vec::new();
            for map in store.resources(project, MAPS)? {
                if map["mode"] == "snapshot" {
                    let count = runs_for_map(store, project, &map)?
                        .first()
                        .map(|run| run["traceCount"].clone())
                        .unwrap_or(json!(0));
                    analyses.push(json!({
                        "id": map["id"], "label": map["name"], "filter": map_filters(&map),
                        "timeRange": map_window(&map), "facetIds": map["facetIds"], "traceCount": count,
                        "status": status(&map["status"]), "createdAt": map["createdAt"], "updatedAt": map["updatedAt"],
                    }));
                } else {
                    rules.push(rule(&map));
                }
            }
            json!({ "rules": rules, "analyses": analyses })
        }
        "topics.currentResults" => {
            let map = selected_map(store, project, input)?;
            let facets = map
                .as_ref()
                .map(|map| facet_results(store, project, map))
                .transpose()?
                .unwrap_or_default();
            json!({ "hasStoredSummaries": has_summaries(store, project)?, "facets": facets })
        }
        "topics.analysisResults" => {
            let map = get_map(store, project, required(input, "executionId")?)?;
            if map["mode"] != "snapshot" {
                return Err(fail("NOT_FOUND", "Analysis not found"));
            }
            let facets = facet_results(store, project, &map)?;
            json!({
                "execution": { "id": map["id"], "status": status(&map["status"]), "phase": phase(&map),
                    "label": map["name"], "timeRange": map_window(&map), "createdAt": map["createdAt"],
                    "progress": facets.iter().map(|facet| json!({
                        "facetId": facet["facetId"], "outcome": outcome(&map),
                        "requested": facet["counts"]["total"], "complete": facet["usableCount"],
                    })).collect::<Vec<_>>(),
                },
                "facets": facets,
            })
        }
        "topics.map" | "topics.mapMatches" => {
            let (map_id, facet) = required(input, "runId")?
                .split_once('/')
                .context("Invalid topic map run ID")?;
            let map = get_map(store, project, map_id)?;
            if !string_list(&map["facetIds"]).iter().any(|id| id == facet) {
                return Err(fail("NOT_FOUND", "Map not found"));
            }
            let publication = topics::result(store, project, map_id, Some(facet))?;
            let mut points = Vec::new();
            let mut trace_ids = Vec::new();
            for item in publication["assignments"].as_array().into_iter().flatten() {
                let trace_id = required(item, "traceId")?;
                let Some(trace) = store.trace(project, trace_id)? else {
                    continue;
                };
                if name == "topics.mapMatches" {
                    if matches_trace(store, project, &trace, &input["filter"])? {
                        trace_ids.push(trace_id.to_owned());
                    }
                } else {
                    points.push(json!({
                        "traceId": trace_id, "summary": item["summary"].as_str().unwrap_or_default(),
                        "unitStartTime": iso(trace.start_ns), "topicId": item["topicStableId"],
                        "outcome": if item["topicStableId"].is_null() {"outlier"} else {"assigned"},
                        "x": item["x"], "y": item["y"],
                    }));
                }
            }
            if name == "topics.mapMatches" {
                json!({ "traceIds": trace_ids })
            } else {
                let published = !publication["publishedAt"].is_null();
                json!({ "status": if published { "ready" } else { "unavailable" },
                    "reason": if published { None } else { Some("This map has not been published.") },
                    "missingSummaryCount": 0, "unpositionedCount": 0, "points": points,
                })
            }
        }
        "topics.traces" => trace_page(store, project, input)?,
        "topics.forTrace" => {
            let trace_id = required(input, "traceId")?;
            if store.trace(project, trace_id)?.is_none() {
                return Err(fail("NOT_FOUND", "Trace not found"));
            }
            trace_facets(store, project, trace_id)?
        }
        "topics.forTraces" => {
            let ids = input["traces"]
                .as_array()
                .context("Trace IDs are required")?;
            if ids.len() > 500 {
                bail!("Too many trace IDs");
            }
            let mut data = BTreeMap::new();
            for trace in ids {
                let id = required(trace, "traceId")?;
                if store.trace(project, id)?.is_some() {
                    let facets = trace_facets(store, project, id)?;
                    let mut values = BTreeMap::<(String, Option<String>), Vec<String>>::new();
                    for facet in facets.as_array().into_iter().flatten() {
                        let key = (
                            required(facet, "name")?.to_owned(),
                            facet["topicName"].as_str().map(str::to_owned),
                        );
                        values
                            .entry(key)
                            .or_default()
                            .push(required(facet, "runId")?.to_owned());
                    }
                    if !values.is_empty() {
                        data.insert(
                            id,
                            values
                                .into_iter()
                                .map(|((facet, topic), runs)| {
                                    json!({
                                        "facet":facet,"topic":topic,"runIds":runs,
                                    })
                                })
                                .collect::<Vec<_>>(),
                        );
                    }
                }
            }
            json!(data)
        }
        "topics.modelSettings" => topic_setup::config(store, project)?,
        "topics.tracingReadinessSettings" => {
            json!({ "traceIdleMs": topic_setup::config(store, project)?["traceIdleMs"].as_u64().unwrap_or(300_000) })
        }
        "topics.hasCurrentEmbeddings" => json!(has_summaries(store, project)?),
        "topics.executions" => {
            let maps = store.resources(project, MAPS)?;
            let mut executions = Vec::new();
            for map in &maps {
                let runs = runs_for_map(store, project, map)?;
                if map["mode"] == "snapshot"
                    || matches!(map["status"].as_str(), Some("Queued" | "Processing"))
                {
                    executions.push(execution(project, map, runs.first(), true));
                }
                if map["mode"] == "live" {
                    executions.extend(
                        runs.iter()
                            .map(|run| execution(project, map, Some(run), false)),
                    );
                }
            }
            executions.sort_by(|a, b| b["updatedAt"].as_str().cmp(&a["updatedAt"].as_str()));
            json!(executions)
        }
        "topics.execution" => {
            let id = required(input, "executionId")?;
            let (map, run) = get_execution(store, project, id)?;
            execution(project, &map, run.as_ref(), map["id"] == id)
        }
        "topics.ruleSetupStatus" => {
            let map = get_map(store, project, required(input, "ruleId")?)?;
            let publication = topics::result(store, project, required(&map, "id")?, None)?;
            let populated = !publication["publishedAt"].is_null();
            let count = publication["assignments"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0);
            json!({
                "phase": phase(&map), "startedAt": map["createdAt"], "updatedAt": map["updatedAt"], "error": map["error"],
                "seededCount": count, "facets": string_list(&map["facetIds"]).into_iter().map(|facet| json!({
                    "facetId": facet, "state": if populated { "has_map" } else { "short" }, "settledCount": count,
                    "runStatus": if populated { Some("completed") } else { None },
                })).collect::<Vec<_>>(),
            })
        }
        "topics.ruleMapCosts" | "topics.executionCosts" | "topics.executionCostBreakdown" => {
            // The local model providers do not report run-level pricing for all
            // completions. The original UI interprets null as unavailable.
            json!(
                input["executionIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|id| {
                        let value = if name == "topics.executionCostBreakdown" {
                            json!({"summaries":null,"naming":null})
                        } else {
                            Value::Null
                        };
                        (id, value)
                    })
                    .collect::<BTreeMap<_, _>>()
            )
        }
        "topics.setupTraceCounts"
        | "topics.setupTraceNames"
        | "topics.previewTraces"
        | "topics.previewAnalysis" => preview(store, project, name, input)?,
        "topics.refreshRuleMap" | "topics.startRuleSetup" | "topics.setRuleEnabled" => {
            let id = required(
                input,
                if name == "topics.setRuleEnabled" {
                    "id"
                } else {
                    "ruleId"
                },
            )?;
            let map = get_map(store, project, id)?;
            if map["mode"] != "live" {
                bail!("Select a live map");
            }
            let action = if name == "topics.setRuleEnabled" {
                if input["enabled"].as_bool().context("Enabled is required")? {
                    "resume"
                } else {
                    "pause"
                }
            } else {
                "refresh"
            };
            if name == "topics.startRuleSetup"
                && (map["enabled"] == false || !topic_setup::processing_enabled(store, project)?)
            {
                Value::Null
            } else {
                rule(&topics::change(store, project, id, action)?)
            }
        }
        "topics.retry" => {
            let id = required(input, "executionId")?;
            let (map, run) = get_execution(store, project, id)?;
            if run
                .as_ref()
                .map(|run| &run["status"])
                .unwrap_or(&map["status"])
                != "Failed"
            {
                bail!("Only a failed execution can be retried");
            }
            topics::change(store, project, required(&map, "id")?, "refresh")?;
            Value::Null
        }
        _ => topic_setup::request(store, project, name, input).await?,
    };
    Ok(result)
}

pub(super) fn get_map(store: &Store, project: &str, id: &str) -> Result<Value> {
    store
        .resource(project, MAPS, id)?
        .ok_or_else(|| fail("NOT_FOUND", "Topic map not found"))
}

fn selected_map(store: &Store, project: &str, input: &Value) -> Result<Option<Value>> {
    let selection = input["analysisId"]
        .as_str()
        .map(|id| (id, "snapshot"))
        .or_else(|| input["ruleId"].as_str().map(|id| (id, "live")));
    if let Some((id, mode)) = selection {
        let map = get_map(store, project, id)?;
        if map["mode"] != mode {
            return Err(fail("NOT_FOUND", "Topic map not found"));
        }
        Ok(Some(map))
    } else {
        Ok(store
            .resources(project, MAPS)?
            .into_iter()
            .find(|map| map["mode"] == "live"))
    }
}

fn map_filters(map: &Value) -> Value {
    map["filters"]
        .as_array()
        .map(|filters| json!(filters))
        .unwrap_or(json!([]))
}

pub(super) fn rule(map: &Value) -> Value {
    json!({
        "id": map["id"], "projectId": map["projectId"], "name": map["name"], "createdAt": map["createdAt"],
        "updatedAt": map["updatedAt"], "facetIds": map["facetIds"], "filter": map_filters(map),
        "sampling": map["sample"].as_f64().unwrap_or(100.) / 100., "enabled": map["enabled"] != false,
        "lastRefitAt": map["lastRunNs"].as_i64().map(iso),
        "refresh": { "availableAt": null, "running": map["status"] == "Processing" },
    })
}

fn map_window(map: &Value) -> Value {
    let to = parse_time(&map["toTimestamp"]).unwrap_or_else(now_ns);
    let from = parse_time(&map["fromTimestamp"]).unwrap_or(to.saturating_sub(30 * DAY));
    json!({ "from": iso(from), "to": iso(to) })
}

fn has_summaries(store: &Store, project: &str) -> Result<bool> {
    store.access(|conn| {
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM topic_summaries WHERE project=?1 AND embedding<>'[]')",
            [project],
            |row| row.get(0),
        )?)
    })
}

fn facet_results(store: &Store, project: &str, map: &Value) -> Result<Vec<Value>> {
    let map_id = required(map, "id")?;
    let mut facets = Vec::new();
    for definition in topics::facet_versions(store, project)? {
        let id = required(&definition, "id")?;
        let name = &definition["name"];
        if !string_list(&map["facetIds"])
            .iter()
            .any(|facet| facet == id)
        {
            continue;
        }
        let data = topics::result(store, project, map_id, Some(id))?;
        let assignments = data["assignments"]
            .as_array()
            .context("Invalid topic assignments")?;
        let rows = data["topics"].as_array().context("Invalid topic list")?;
        let run_id = format!("{map_id}/{id}");
        let assigned = assignments
            .iter()
            .filter(|row| !row["topicStableId"].is_null())
            .count();
        let definitions = rows.iter().map(|row| json!({
            "id": row["stableId"], "versionId": row.get("id").unwrap_or(&row["stableId"]),
            "runId": run_id, "name": row["name"], "description": row["description"], "count": row["traceCount"],
        })).collect::<Vec<_>>();
        let published = !data["publishedAt"].is_null();
        facets.push(json!({
            "facetId": id, "name": name, "facetVersion": definition["versions"][0]["version"], "window": map_window(map),
            "usableCount": assignments.len(), "awaitingCount": 0,
            "counts": { "total": assignments.len(), "assigned": assigned, "outlier": assignments.len()-assigned,
                "awaitingMap": 0, "notApplicable": 0, "insufficientInput": 0, "emptyInput": 0, "inputTooLarge": 0,
            },
            "topics": definitions,
            "map": published.then(|| json!({"runId":run_id,"topics":definitions})),
            "outcome": outcome(map),
        }));
    }
    Ok(facets)
}

fn trace_page(store: &Store, project: &str, input: &Value) -> Result<Value> {
    let Some(map) = selected_map(store, project, input)? else {
        return Ok(json!({ "rows": [], "nextCursor": null }));
    };
    let facet = required(input, "facetId")?;
    let publication = topics::result(store, project, required(&map, "id")?, Some(facet))?;
    let mut rows = Vec::new();
    let cursor = parse_time(&input["cursor"]["timestamp"]).zip(input["cursor"]["traceId"].as_str());
    for row in publication["assignments"].as_array().into_iter().flatten() {
        let id = required(row, "traceId")?;
        if input["traceId"]
            .as_str()
            .is_some_and(|expected| expected != id)
        {
            continue;
        }
        let topic = &row["topicStableId"];
        let outcome = if topic.is_null() {
            "outlier"
        } else {
            "assigned"
        };
        if input["assigned"]
            .as_bool()
            .is_some_and(|wanted| wanted == topic.is_null())
            || !choice(&input["topicIds"], topic.as_str().unwrap_or_default())
            || !choice(&input["statuses"], outcome)
        {
            continue;
        }
        let Some(trace) = store.trace(project, id)? else {
            continue;
        };
        if cursor.is_some_and(|(timestamp, trace_id)| (trace.start_ns, id) >= (timestamp, trace_id))
            || !matches_trace(store, project, &trace, &input["filter"])?
        {
            continue;
        }
        let topic_name = publication["topics"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["stableId"] == *topic)
            .map(|row| &row["name"]);
        rows.push((trace.start_ns, id, json!({
            "traceId": id, "unitStartTime": iso(trace.start_ns), "facetVersion": 1,
            "summary": row["summary"], "outcome": outcome, "topicId": topic, "topicName": topic_name,
        })));
    }
    rows.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    let cursor = (rows.len() > 50)
        .then(|| json!({"timestamp":iso(rows[49].0),"traceId":rows[49].1,"facetId":facet}));
    Ok(
        json!({ "rows": rows.into_iter().take(50).map(|(_,_,row)| row).collect::<Vec<_>>(), "nextCursor": cursor }),
    )
}

fn choice(selection: &Value, value: &str) -> bool {
    let Some(values) = selection["values"].as_array() else {
        return true;
    };
    values.iter().any(|possible| possible == value) != (selection["operator"] == "none of")
}

pub(crate) fn matches_trace(
    store: &Store,
    project: &str,
    trace: &Trace,
    filters: &Value,
) -> Result<bool> {
    let public = trace.public();
    if includes(&public, filters) {
        return Ok(true);
    }
    let rows = store
        .observations(project, Some(&trace.id), None, 10_000, 0)?
        .0;
    Ok(rows.iter().any(|observation| {
        let mut fields = public.clone();
        if let (Some(base), Some(observed)) = (
            fields.as_object_mut(),
            observation.public(project).as_object(),
        ) {
            base.extend(observed.clone());
            base.insert("name".to_owned(), json!(trace.name));
            base.insert("observationName".to_owned(), json!(observation.name));
        }
        includes(&fields, filters)
    }))
}

fn trace_facets(store: &Store, project: &str, trace: &str) -> Result<Value> {
    let mut results = Vec::new();
    for map in store
        .resources(project, MAPS)?
        .into_iter()
        .filter(|map| map["mode"] == "live")
    {
        let id = required(&map, "id")?;
        for definition in topics::facet_versions(store, project)? {
            let facet = required(&definition, "id")?;
            let name = &definition["name"];
            let publication = topics::result(store, project, id, Some(facet))?;
            let Some(row) = publication["assignments"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|row| row["traceId"] == trace)
            else {
                continue;
            };
            let topic_id = &row["topicStableId"];
            let topic = publication["topics"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .find(|(_, item)| item["stableId"] == *topic_id);
            results.push(json!({
                "facetId": facet, "name": name, "ruleName": map["name"], "runId": format!("{id}/{facet}"),
                "summary": row["summary"], "outcome": if topic_id.is_null() {"outlier"} else {"assigned"}, "topicId": topic_id,
                "topicName": topic.map(|(_, item)| &item["name"]), "topicIndex": topic.map(|(index, _)| index as i64).unwrap_or(-1),
            }));
        }
    }
    Ok(json!(results))
}

pub(super) fn stored_map_models(store: &Store, project: &str) -> Result<Value> {
    let maps = store.resources(project, MAPS)?;
    let map = maps
        .iter()
        .find(|map| map["mode"] == "live")
        .or_else(|| maps.first());
    let providers = store.resources(project, "llm-connections")?;
    let slot = |name: &str, field: &str| {
        map.and_then(|map| {
            let connection = providers
                .iter()
                .find(|connection| connection["id"] == map[name])?;
            Some(json!({ "provider": connection["name"], "model": connection[field] }))
        })
    };
    Ok(json!({
        "summary": slot("summaryProvider", "model"), "embedding": slot("embeddingProvider", "embeddingModel"),
        "clustering": slot("namingProvider", "model"), "embeddingDimensions": map.and_then(|map| map["dimensions"].as_u64()).unwrap_or(1024),
        "enabled": map.map(|map| map["enabled"] != false), "blockedAt": null, "blockReason": null, "blockMessage": null,
        "traceIdleMs": map.and_then(|map| map["readinessSeconds"].as_u64()).map(|seconds| seconds * 1_000),
    }))
}

fn runs_for_map(store: &Store, project: &str, map: &Value) -> Result<Vec<Value>> {
    Ok(store
        .resources(project, RUNS)?
        .into_iter()
        .filter(|run| run["mapId"] == map["id"])
        .collect())
}

fn filter_options(store: &Store, project: &str) -> Result<Value> {
    let maps = store.resources(project, MAPS)?;
    let mut options = BTreeMap::<String, BTreeSet<String>>::new();
    for definition in topics::facet_versions(store, project)? {
        let id = required(&definition, "id")?;
        let name = required(&definition, "name")?;
        for map in maps.iter().filter(|map| map["mode"] == "live") {
            let publication = topics::result(store, project, required(map, "id")?, Some(id))?;
            for topic in publication["topics"].as_array().into_iter().flatten() {
                options
                    .entry(name.to_owned())
                    .or_default()
                    .insert(required(topic, "name")?.to_owned());
            }
        }
    }
    Ok(json!(options))
}

fn stripe_map(store: &Store, project: &str, facet: &str) -> Result<Value> {
    let Some(definition) = topics::facet_versions(store, project)?
        .into_iter()
        .find(|definition| definition["name"] == facet)
    else {
        return Ok(Value::Null);
    };
    let facet_id = required(&definition, "id")?;
    for map in store
        .resources(project, MAPS)?
        .into_iter()
        .rev()
        .filter(|map| map["mode"] == "live")
    {
        let id = required(&map, "id")?;
        let result = topics::result(store, project, id, Some(facet_id))?;
        if result["assignments"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
        {
            return Ok(
                json!({"facetId":facet_id,"ruleId":id,"mapName":map["name"],"runId":format!("{id}/{facet_id}")}),
            );
        }
    }
    Ok(Value::Null)
}

fn get_execution(store: &Store, project: &str, id: &str) -> Result<(Value, Option<Value>)> {
    if let Some(map) = store.resource(project, MAPS, id)? {
        let recent = runs_for_map(store, project, &map)?.into_iter().next();
        return Ok((map, recent));
    }
    let run = store
        .resource(project, RUNS, id)?
        .ok_or_else(|| fail("NOT_FOUND", "Topic execution not found"))?;
    Ok((
        get_map(store, project, required(&run, "mapId")?)?,
        Some(run),
    ))
}

fn status(value: &Value) -> &'static str {
    match value.as_str() {
        Some("Queued") => "queued",
        Some("Processing") => "running",
        Some("Failed") => "failed",
        _ => "completed",
    }
}

fn phase(map: &Value) -> &'static str {
    match map["status"].as_str() {
        Some("Queued") => "selecting",
        Some("Processing") => "fitting",
        Some("Failed") => "failed",
        _ => "done",
    }
}

fn outcome(map: &Value) -> &'static str {
    match map["status"].as_str() {
        Some("Insufficient data") => "insufficient_data",
        Some("Failed") => "failed",
        Some("Queued" | "Processing") => "pending",
        _ => "published",
    }
}

fn execution(project: &str, map: &Value, run: Option<&Value>, use_map_id: bool) -> Value {
    let record = if use_map_id { map } else { run.unwrap_or(map) };
    let id = &record["id"];
    let facets = string_list(&map["facetIds"]);
    let trace_count = run.and_then(|run| run["traceCount"].as_u64()).unwrap_or(0);
    let analysis = (map["mode"] == "snapshot").then(|| {
        json!({
            "label": map["name"], "filter": map_filters(map),
        })
    });
    json!({
        "id": id, "projectId": project, "status": status(&record["status"]), "phase": phase(record),
        "createdAt": record["createdAt"], "updatedAt": record["updatedAt"], "error": record["error"], "estimatedCost": null,
        "input": { "operation": "update", "projectId": project, "requestId": id, "timeRange": map_window(map),
            "analysis": analysis, "embeddingConfig": { "embeddingDimensions": map["dimensions"] },
            "facets": facets.iter().map(|facet| json!({"facetId":facet,"version":1})).collect::<Vec<_>>(),
        },
        "facets": facets.iter().map(|facet| json!({
            "facetId":facet,"facetVersion":1,"outcome":outcome(record), "runId":format!("{}/{facet}", map["id"].as_str().unwrap_or_default()),
            "counts":{"requested":trace_count,"complete":trace_count,"failed":0,"pending":0,"notApplicable":0,"insufficientInput":0},
        })).collect::<Vec<_>>(),
    })
}

fn preview(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    let now = now_ns();
    let from = parse_time(&input["timeRange"]["from"])
        .or_else(|| parse_time(&input["from"]))
        .unwrap_or(now - 30 * DAY);
    let to = parse_time(&input["timeRange"]["to"])
        .or_else(|| parse_time(&input["to"]))
        .unwrap_or(now);
    if to <= from {
        bail!("Choose an end time after the start time");
    }
    let results = store
        .traces(
            project,
            &TraceFilter {
                from: Some(from),
                to: Some(to),
                ..Default::default()
            },
            i64::MAX as usize,
            0,
        )?
        .0;
    let mut candidates = Vec::new();
    for trace in results {
        if matches_trace(store, project, &trace, &input["filter"])? {
            candidates.push(trace);
        }
    }
    Ok(match name {
        "topics.setupTraceCounts" => json!({
            "finished7d": candidates.iter().filter(|trace| trace.start_ns >= now - 7*DAY && trace.received_ns <= now-300*SECOND).count(),
            "finished30d": candidates.iter().filter(|trace| trace.received_ns <= now-300*SECOND).count(),
        }),
        "topics.setupTraceNames" => {
            let names = candidates
                .iter()
                .map(|trace| &trace.name)
                .collect::<BTreeSet<_>>();
            json!(names.into_iter().take(100).collect::<Vec<_>>())
        }
        "topics.previewAnalysis" => {
            let ids = candidates
                .iter()
                .map(|trace| trace.id.as_str())
                .collect::<BTreeSet<_>>();
            let mut counts = Vec::new();
            for facet in input["facets"].as_array().into_iter().flatten() {
                let facet = required(facet, "facetId")?;
                let present = store.access(|conn| {
                    let mut q = conn.prepare("SELECT DISTINCT trace FROM topic_summaries WHERE project=?1 AND facet=?2 AND embedding<>'[]'")?;
                    let rows = q.query_map(params![project, facet], |row| row.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(rows.iter().filter(|trace| ids.contains(trace.as_str())).count())
                })?;
                counts.push((facet, present));
            }
            let summarized = counts.iter().map(|(_, count)| *count).min().unwrap_or(0);
            json!({
                "matchedTraceCount": candidates.len(), "summarizedCount": summarized, "unsummarizedCount": candidates.len().saturating_sub(summarized),
                "summarized": counts.into_iter().map(|(facet,count)| json!({"facetId":facet,"count":count})).collect::<Vec<_>>(),
                "minimumTraceCount": 100, "smallSampleMinimumTraceCount": 10, "capped": false, "ready": summarized >= 100,
            })
        }
        _ => json!({
            "matchedTraceCount": candidates.len(),
            "traces": candidates.iter().take(10).map(Trace::public).collect::<Vec<_>>(),
        }),
    })
}
