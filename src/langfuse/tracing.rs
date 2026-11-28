use super::fail;
use crate::{
    desktop::required,
    model::{Observation, TraceFilter, cost_total, iso, parse_time, usage_total},
    resources::matches_filters,
    store::{Store, decode},
};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Map, Value, json};
use std::{cmp::Ordering, collections::BTreeMap};

pub(super) fn request(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    let result = match name {
        "traces.hasTracingConfigured" => {
            json!(store.traces(project, &TraceFilter::default(), 0, 0)?.1 > 0)
        }
        "traces.all" | "traces.countAll" | "traces.filterOptions" => {
            let rows = trace_rows(store, project, input)?;
            match name {
                "traces.countAll" => json!({ "totalCount": rows.len() }),
                "traces.filterOptions" => trace_filter_options(store, project, &rows)?,
                _ => json!({ "traces": paginate(rows, input)? }),
            }
        }
        "traces.metrics" => {
            let ids = input["traceIds"]
                .as_array()
                .context("Trace IDs are required")?;
            if ids.len() > 500 {
                bail!("Too many trace IDs");
            }
            let mut result = Vec::with_capacity(ids.len());
            for id in ids {
                let id = id.as_str().context("Invalid trace ID")?;
                if let Some(trace) = store.trace(project, id)? {
                    let observations = store
                        .observations(project, Some(id), None, i64::MAX as usize, 0)?
                        .0;
                    let scores = store
                        .scores(project, Some(id), None, i64::MAX as usize, 0)?
                        .0;
                    let mut metrics = observation_metrics(&observations);
                    metrics["id"] = json!(id);
                    metrics["latency"] = json!(crate::model::seconds(trace.start_ns, trace.end_ns));
                    metrics["scores"] = aggregate_scores(scores);
                    result.push(metrics);
                }
            }
            json!(result)
        }
        "traces.byId" | "traces.byIdWithObservationsAndScores" => {
            let id = required(input, "traceId")?;
            let trace = store
                .trace(project, id)?
                .ok_or_else(|| fail("NOT_FOUND", "Trace not found"))?;
            let mut wire = trace.public();
            wire["projectId"] = json!(project);
            wire["createdAt"] = json!(iso(trace.start_ns));
            compact_io(&mut wire, input["verbosity"].as_str());
            if name == "traces.byIdWithObservationsAndScores" {
                let observations = store.observations(project, Some(id), None, 10_000, 0)?.0;
                wire["observations"] = json!(
                    observations
                        .iter()
                        .map(|item| observation_wire(item, project, false))
                        .collect::<Vec<_>>()
                );
                let (corrections, scores): (Vec<_>, Vec<_>) = store
                    .scores(project, Some(id), None, i64::MAX as usize, 0)?
                    .0
                    .into_iter()
                    .partition(|row| row["dataType"] == "CORRECTION");
                wire["scores"] = json!(scores);
                wire["corrections"] = json!(corrections);
            }
            wire
        }
        "traces.deleteMany" => {
            let ids = input["traceIds"]
                .as_array()
                .context("Trace IDs are required")?;
            if ids.is_empty() || ids.len() > 500 {
                bail!("Select 1 to 500 traces");
            }
            let ids = ids
                .iter()
                .map(|id| id.as_str().context("Invalid trace ID"))
                .collect::<Result<Vec<_>>>()?;
            store.transaction(|tx| {
                for id in ids {
                    tx.execute(
                        "DELETE FROM traces WHERE project=?1 AND id=?2",
                        params![project, id],
                    )?;
                }
                Ok(())
            })?;
            Value::Null
        }
        "events.all" | "events.countAll" | "events.filterOptions" => {
            let rows = event_rows(store, project, input)?;
            match name {
                "events.countAll" => json!({ "totalCount": rows.len() }),
                "events.filterOptions" => event_filter_options(&rows),
                _ => {
                    let (limit, offset) = pagination(input)?;
                    let more = rows.len() > offset.saturating_add(limit);
                    json!({ "observations": paginate(rows, input)?, "hasMore": more })
                }
            }
        }
        "events.byTraceId" => {
            let id = required(input, "traceId")?;
            store
                .trace(project, id)?
                .ok_or_else(|| fail("NOT_FOUND", "Trace not found"))?;
            const LIMIT: usize = 10_000;
            let (rows, total) = store.observations(project, Some(id), None, LIMIT, 0)?;
            json!({
                "observations": rows.iter().map(|row| observation_wire(row, project, false)).collect::<Vec<_>>(),
                "cutoffObservationsAfterMaxCount": total > LIMIT, "maxObservationsPerTrace": LIMIT,
            })
        }
        "events.scoresForTrace" => json!(
            store
                .scores(
                    project,
                    Some(required(input, "traceId")?),
                    None,
                    i64::MAX as usize,
                    0
                )?
                .0
        ),
        "events.batchIO" => {
            let ids = input["observations"]
                .as_array()
                .context("Observations are required")?;
            if ids.len() > 500 {
                bail!("Too many observations");
            }
            let requested_trace = input["traceId"].as_str();
            let mut result = Vec::with_capacity(ids.len());
            for id in ids {
                let trace = required(id, "traceId")?;
                if requested_trace.is_some_and(|expected| expected != trace) {
                    bail!("Observation trace does not match");
                }
                if let Some(observation) = observation(store, project, trace, required(id, "id")?)?
                {
                    let mut value = json!({"id": observation.id, "input": observation.input, "output": observation.output, "metadata": observation.metadata});
                    compact_io(
                        &mut value,
                        input["truncated"]
                            .as_bool()
                            .unwrap_or(true)
                            .then_some("truncated"),
                    );
                    result.push(value);
                }
            }
            json!(result)
        }
        "observations.byId" => {
            let id = required(input, "observationId")?;
            let trace = required(input, "traceId")?;
            let row = observation(store, project, trace, id)?
                .ok_or_else(|| fail("NOT_FOUND", "Observation not found"))?;
            let mut data = observation_wire(&row, project, true);
            compact_io(&mut data, input["verbosity"].as_str());
            data
        }
        "sessions.hasAny" | "sessions.hasAnyFromEvents" => {
            json!(store.sessions(project, 0, 0)?.1 > 0)
        }
        "sessions.all"
        | "sessions.allFromEvents"
        | "sessions.countAll"
        | "sessions.countAllFromEvents" => {
            let rows = session_rows(store, project, input)?;
            if name.contains("countAll") {
                json!({ "totalCount": rows.len() })
            } else {
                json!({ "sessions": paginate(rows, input)? })
            }
        }
        _ => {
            return Err(fail(
                "NOT_IMPLEMENTED",
                format!("Workspace procedure is unavailable: {name}"),
            ));
        }
    };
    Ok(result)
}

fn observation(store: &Store, project: &str, trace: &str, id: &str) -> Result<Option<Observation>> {
    store.access(|conn| {
        let raw = conn
            .query_row(
                "SELECT data FROM observations WHERE project=?1 AND trace=?2 AND id=?3",
                params![project, trace, id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        raw.as_deref().map(decode).transpose()
    })
}

fn observation_wire(item: &Observation, project: &str, with_io: bool) -> Value {
    let mut data = item.public(project);
    data["internalModel"] = json!(item.model);
    data["isRootObservation"] = json!(item.parent_id.is_none());
    data["totalTokens"] = json!(usage_total(&item.usage));
    data["inputUsage"] = item.usage["input"].clone();
    data["outputUsage"] = item.usage["output"].clone();
    if !with_io {
        data.as_object_mut()
            .expect("observation object")
            .remove("input");
        data.as_object_mut()
            .expect("observation object")
            .remove("output");
    }
    compact_io(&mut data, None);
    data
}

fn compact_io(data: &mut Value, verbosity: Option<&str>) {
    for key in ["input", "output", "metadata"] {
        if let Some(value) = data.get_mut(key) {
            if value.is_null() {
                continue;
            }
            let source = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            *value = match verbosity {
                Some("compact") => Value::Null,
                Some("truncated") => json!(source.chars().take(10_000).collect::<String>()),
                _ => json!(source),
            };
        }
    }
}

fn trace_rows(store: &Store, project: &str, input: &Value) -> Result<Vec<Value>> {
    // Language-aware search and the original filter builder also operate on
    // serialized I/O/metadata. Keep the persisted data tied to this project.
    let (traces, _) = store.traces(project, &TraceFilter::default(), i64::MAX as usize, 0)?;
    Ok(traces
        .into_iter()
        .map(|trace| {
            let mut row = trace.public();
            row["projectId"] = json!(project);
            row
        })
        .filter(|row| {
            matches_search(row, input)
                && includes(row, &input["filter"])
                && includes(row, &input["timestampFilter"])
        })
        .collect())
}

fn event_rows(store: &Store, project: &str, input: &Value) -> Result<Vec<Value>> {
    let (observations, _) = store.observations(
        project,
        input["traceId"].as_str(),
        None,
        i64::MAX as usize,
        0,
    )?;
    let (traces, _) = store.traces(project, &TraceFilter::default(), i64::MAX as usize, 0)?;
    let traces: BTreeMap<_, _> = traces
        .iter()
        .map(|trace| (trace.id.as_str(), trace))
        .collect();
    Ok(observations
        .into_iter()
        .rev()
        .filter_map(|item| {
            let trace = traces.get(item.trace_id.as_str())?;
            let mut row = item.public(project);
            row["traceName"] = json!(trace.name);
            row["traceTags"] = json!(trace.tags);
            row["sessionId"] = json!(trace.session_id);
            row["userId"] = json!(trace.user_id);
            row["release"] = json!(trace.release);
            row["isRootObservation"] = json!(item.parent_id.is_none());
            row["hasParentObservation"] = json!(item.parent_id.is_some());
            if matches_search(&row, input)
                && includes(&row, &input["filter"])
                && includes(&row, &input["startTimeFilter"])
                && input["isRootObservation"]
                    .as_bool()
                    .is_none_or(|expected| row["isRootObservation"] == expected)
                && input["hasParentObservation"]
                    .as_bool()
                    .is_none_or(|expected| row["hasParentObservation"] == expected)
            {
                row.as_object_mut()
                    .expect("observation object")
                    .remove("input");
                row.as_object_mut()
                    .expect("observation object")
                    .remove("output");
                Some(row)
            } else {
                None
            }
        })
        .collect())
}

fn session_rows(store: &Store, project: &str, input: &Value) -> Result<Vec<Value>> {
    let (traces, _) = store.traces(project, &TraceFilter::default(), i64::MAX as usize, 0)?;
    let (sessions, _) = store.sessions(project, i64::MAX as usize, 0)?;
    let by_id = traces
        .iter()
        .filter_map(|trace| trace.session_id.as_deref().map(|id| (id, trace)))
        .fold(BTreeMap::<_, Vec<_>>::new(), |mut groups, (id, trace)| {
            groups.entry(id).or_default().push(trace);
            groups
        });
    Ok(sessions
        .into_iter()
        .filter_map(|mut row| {
            let group = by_id.get(row["id"].as_str()?)?;
            row["environment"] = json!(group[0].environment);
            row["countTraces"] = row["traceCount"].clone();
            row["userIds"] = json!(
                group
                    .iter()
                    .filter_map(|trace| trace.user_id.as_deref())
                    .collect::<std::collections::BTreeSet<_>>()
            );
            row["traceTags"] = json!(
                group
                    .iter()
                    .flat_map(|trace| trace.tags.iter())
                    .collect::<std::collections::BTreeSet<_>>()
            );
            includes(&row, &input["filter"]).then_some(row)
        })
        .collect())
}

fn trace_filter_options(store: &Store, project: &str, rows: &[Value]) -> Result<Value> {
    let (scores, _) = store.scores(project, None, None, i64::MAX as usize, 0)?;
    let groups = |kind: &str| {
        scores
            .iter()
            .filter(|row| row["dataType"] == kind)
            .filter_map(|row| row["name"].as_str())
            .collect::<std::collections::BTreeSet<_>>()
    };
    Ok(json!({
        "name": options(rows, "name"), "tags": options(rows, "tags"), "users": options(rows, "userId"),
        "sessions": options(rows, "sessionId"), "scores_avg": groups("NUMERIC"), "score_booleans": groups("BOOLEAN"),
        "score_categories": groups("CATEGORICAL"),
    }))
}

fn event_filter_options(rows: &[Value]) -> Value {
    let mut values = Map::new();
    for column in [
        "name",
        "traceName",
        "model",
        "level",
        "type",
        "environment",
        "userId",
        "sessionId",
        "traceTags",
        "promptName",
        "version",
        "release",
    ] {
        values.insert(column.to_owned(), json!(options(rows, column)));
    }
    values.insert("approximateCount".to_owned(), json!(rows.len()));
    Value::Object(values)
}

fn options(rows: &[Value], column: &str) -> Vec<Value> {
    let mut counts = BTreeMap::new();
    for row in rows {
        let value = &row[column];
        if let Some(list) = value.as_array() {
            for text in list.iter().filter_map(Value::as_str) {
                *counts.entry(text).or_insert(0) += 1;
            }
        } else if let Some(text) = value.as_str() {
            *counts.entry(text).or_insert(0) += 1;
        }
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by_key(|(text, count)| (std::cmp::Reverse(*count), *text));
    counts
        .into_iter()
        .take(500)
        .map(|(value, count)| json!({ "value": value, "count": count }))
        .collect()
}

fn matches_search(row: &Value, input: &Value) -> bool {
    let Some(query) = input["searchQuery"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
    else {
        return true;
    };
    let query = query.to_lowercase();
    let search = input["searchType"].as_array();
    if search.is_none_or(Vec::is_empty) {
        return row.to_string().to_lowercase().contains(&query);
    }
    let search = search.expect("checked nonempty");
    [
        ["id", "traceId"].as_slice(),
        &["input", "output", "metadata"],
        &["name", "traceName", "userId", "sessionId", "tags"],
    ]
    .into_iter()
    .zip(["id", "io", "name"])
    .any(|(keys, kind)| {
        search.iter().any(|mode| {
            mode.as_str()
                .is_some_and(|text| text.eq_ignore_ascii_case(kind))
        }) && keys
            .iter()
            .any(|key| row[*key].to_string().to_lowercase().contains(&query))
    })
}

pub(super) fn includes(row: &Value, conditions: &Value) -> bool {
    conditions.as_array().is_none_or(|filters| {
        filters.iter().all(|filter| {
            let column = filter["column"].as_str().unwrap_or_default();
            let key = match column {
                "Timestamp" => "timestamp",
                "Start Time" => "startTime",
                "Created At" => "createdAt",
                "Name" => "name",
                "Tags" => "tags",
                "Environment" => "environment",
                "User ID" => "userId",
                "Session ID" => "sessionId",
                "Trace ID" => "traceId",
                other => other,
            };
            let operand = &filter["value"];
            let value = &row[key];
            let operator = filter["operator"].as_str().unwrap_or("=");
            if filter["type"] == "datetime" {
                return parse_time(value)
                    .zip(parse_time(operand))
                    .is_some_and(|(left, right)| compare(left.cmp(&right), operator));
            }
            if let (Some(values), Some(desired)) = (value.as_array(), operand.as_array()) {
                return match operator {
                    "any of" | "in" => desired.iter().any(|part| values.contains(part)),
                    "all of" => desired.iter().all(|part| values.contains(part)),
                    "none of" | "not in" => desired.iter().all(|part| !values.contains(part)),
                    _ => value == operand,
                };
            }
            if operator == "is null" {
                return value.is_null();
            }
            if operator == "is not null" {
                return !value.is_null();
            }
            let mut normalized = filter.clone();
            normalized["column"] = json!(key);
            matches_filters(row, &json!([normalized]))
        })
    })
}

fn compare(order: Ordering, operator: &str) -> bool {
    match operator {
        "=" => order.is_eq(),
        "!=" => !order.is_eq(),
        ">" => order.is_gt(),
        ">=" => order.is_ge(),
        "<" => order.is_lt(),
        "<=" => order.is_le(),
        _ => false,
    }
}

fn pagination(input: &Value) -> Result<(usize, usize)> {
    let page = input["page"].as_u64().unwrap_or(0);
    let limit = input["limit"].as_u64().unwrap_or(50);
    if limit == 0 || limit > 500 || page > 1_000_000 {
        bail!("Invalid pagination");
    }
    Ok((limit as usize, (page * limit) as usize))
}

pub(super) fn paginate(mut rows: Vec<Value>, input: &Value) -> Result<Vec<Value>> {
    let (limit, offset) = pagination(input)?;
    if let Some(column) = input["orderBy"]["column"].as_str() {
        let descending = input["orderBy"]["order"] == "DESC";
        rows.sort_by(|a, b| {
            let left = &a[column];
            let right = &b[column];
            let order = left
                .as_f64()
                .zip(right.as_f64())
                .map(|(a, b)| a.total_cmp(&b))
                .unwrap_or_else(|| left.as_str().cmp(&right.as_str()));
            if descending { order.reverse() } else { order }
        });
    }
    Ok(rows.into_iter().skip(offset).take(limit).collect())
}

fn observation_metrics(rows: &[Observation]) -> Value {
    let sum_token = |key: &str| {
        rows.iter()
            .filter_map(|item| item.usage[key].as_i64())
            .sum::<i64>()
    };
    json!({
        "observationCount": rows.len(), "inputUsage": sum_token("input"), "outputUsage": sum_token("output"),
        "totalUsage": rows.iter().filter_map(|item| usage_total(&item.usage)).sum::<i64>(),
        "totalCost": rows.iter().filter_map(|item| cost_total(&item.cost)).sum::<f64>(),
    })
}

fn aggregate_scores(scores: Vec<Value>) -> Value {
    let mut buckets = BTreeMap::<String, Vec<_>>::new();
    for row in scores {
        if let (Some(name), Some(source), Some(kind)) = (
            row["name"].as_str(),
            row["source"].as_str(),
            row["dataType"].as_str(),
        ) {
            let name = name.replace(['-', '.'], "_");
            buckets
                .entry(format!("{name}-{source}-{kind}"))
                .or_default()
                .push(row);
        }
    }
    json!(
        buckets
            .into_iter()
            .filter_map(|(key, rows)| {
                let values: Vec<Value> = match rows[0]["dataType"].as_str() {
                    Some("NUMERIC") => rows
                        .iter()
                        .filter_map(|row| row["value"].as_f64().map(|number| json!(number)))
                        .collect(),
                    Some("CATEGORICAL" | "BOOLEAN") => rows
                        .iter()
                        .filter_map(|row| row["stringValue"].as_str().map(|text| json!(text)))
                        .collect(),
                    _ => return None,
                };
                if values.is_empty() {
                    return None;
                }
                let data = if rows[0]["dataType"] == "NUMERIC" {
                    let average =
                        values.iter().filter_map(Value::as_f64).sum::<f64>() / values.len() as f64;
                    json!({ "type": "NUMERIC", "values": values, "average": average })
                } else {
                    json!({ "type": "CATEGORICAL", "values": values })
                };
                Some((key, data))
            })
            .collect::<Map<String, Value>>()
    )
}
