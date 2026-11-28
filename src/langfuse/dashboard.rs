use super::{
    fail,
    tracing::{includes, paginate},
};
use crate::{
    desktop::required,
    model::{TraceFilter, iso, now_ns, parse_time},
    store::Store,
};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const HOME: &str = "langfuse-home-dashboard";

pub(super) fn request(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    let result = match name {
        "dashboard.executeQuery" => {
            let query = &input["query"];
            if !query.is_object() {
                bail!("Metrics query is required");
            }
            store.metrics(project, query)?["data"].clone()
        }
        "dashboard.chart" => chart(store, project, input)?,
        "dashboard.allDashboards" => {
            let mut data: Vec<_> = store
                .resources(project, "dashboards")?
                .into_iter()
                .map(dashboard)
                .collect();
            data.push(home());
            let total = data.len();
            let mut page = input.clone();
            // This list is one-based upstream, unlike most table procedures.
            page["page"] = json!(input["page"].as_u64().unwrap_or(1).saturating_sub(1));
            json!({ "dashboards": paginate(data, &page)?, "totalCount": total })
        }
        "dashboard.getDashboard" => get(store, project, required(input, "dashboardId")?)?
            .ok_or_else(|| fail("NOT_FOUND", "Dashboard not found"))?,
        "dashboard.getHomeDashboard" => {
            let pointer: Option<String> = store.access(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT value FROM settings WHERE key=?1",
                        [home_key(project)],
                        |row| row.get(0),
                    )
                    .optional()?)
            })?;
            let selected = pointer
                .as_deref()
                .map(|id| get(store, project, id))
                .transpose()?
                .flatten();
            json!({ "dashboard": selected.clone().unwrap_or_else(home), "homeDashboardId": if selected.is_some() { pointer } else { None } })
        }
        "dashboard.setHomeDashboard" => {
            let id = input["dashboardId"].as_str();
            if let Some(id) = id {
                if get(store, project, id)?.is_none() {
                    return Err(fail("NOT_FOUND", "Dashboard not found"));
                }
            } else if !input["dashboardId"].is_null() {
                bail!("Invalid dashboard ID");
            }
            store.access(|conn| {
                let key = home_key(project);
                if let Some(id) = id {
                    conn.execute(
                        "INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)",
                        params![key, id],
                    )?;
                } else {
                    conn.execute("DELETE FROM settings WHERE key=?1", [key])?;
                }
                Ok(())
            })?;
            json!({ "success": true })
        }
        "dashboard.createDashboard" => {
            let name = required(input, "name")?;
            dashboard(store.put_resource(project, "dashboards", json!({
                "name": name, "description": input["description"], "definition": {"widgets": [] },
                "filters": [], "projectId": project,
            }))?)
        }
        "dashboard.updateDashboardMetadata"
        | "dashboard.updateDashboardDefinition"
        | "dashboard.updateDashboardFilters" => {
            let id = required(input, "dashboardId")?;
            editable(store, project, id)?;
            let mut data = json!({ "id": id });
            match name {
                "dashboard.updateDashboardMetadata" => {
                    data["name"] = json!(required(input, "name")?);
                    data["description"] = input["description"].clone();
                }
                "dashboard.updateDashboardDefinition" => {
                    if input["definition"]["widgets"].as_array().is_none() {
                        bail!("Invalid dashboard placements");
                    }
                    data["definition"] = input["definition"].clone();
                }
                _ => {
                    if input["filters"].as_array().is_none() {
                        bail!("Invalid dashboard filters");
                    }
                    data["filters"] = input["filters"].clone();
                }
            }
            dashboard(store.put_resource(project, "dashboards", data)?)
        }
        "dashboard.cloneDashboard" => {
            let source = get(store, project, required(input, "dashboardId")?)?
                .context("Dashboard not found")?;
            let data = dashboard(store.put_resource(project, "dashboards", json!({
                "name": required(input, "name")?, "description": input.get("description").unwrap_or(&source["description"]),
                "projectId": project, "filters": source["filters"], "definition": input.get("definition").unwrap_or(&source["definition"]),
            }))?);
            if input["setAsHome"] == true {
                request(
                    store,
                    project,
                    "dashboard.setHomeDashboard",
                    &json!({"dashboardId":data["id"]}),
                )?;
            }
            data
        }
        "dashboard.delete" => {
            let id = required(input, "dashboardId")?;
            editable(store, project, id)?;
            store.delete_resource(project, "dashboards", id)?;
            json!({ "success": true })
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

fn home_key(project: &str) -> String {
    format!("home-dashboard:{project}")
}

fn get(store: &Store, project: &str, id: &str) -> Result<Option<Value>> {
    if id == HOME {
        return Ok(Some(home()));
    }
    Ok(store.resource(project, "dashboards", id)?.map(dashboard))
}

fn editable(store: &Store, project: &str, id: &str) -> Result<()> {
    if id == HOME {
        return Err(fail("FORBIDDEN", "Clone the built-in dashboard to edit it"));
    }
    if store.resource(project, "dashboards", id)?.is_none() {
        return Err(fail("NOT_FOUND", "Dashboard not found"));
    }
    Ok(())
}

fn dashboard(mut data: Value) -> Value {
    data["owner"] = json!("PROJECT");
    if data["definition"]["widgets"].as_array().is_none() {
        data["definition"] = json!({ "widgets": [] });
    }
    if !data["filters"].is_array() {
        data["filters"] = json!([]);
    }
    data
}

fn home() -> Value {
    let mut home: Value =
        serde_json::from_str(include_str!("home-dashboard.json")).expect("pinned home dashboard");
    home["owner"] = json!("LANGFUSE");
    home["projectId"] = Value::Null;
    home
}

fn chart(store: &Store, project: &str, input: &Value) -> Result<Value> {
    match input["queryName"].as_str() {
        Some("score-aggregate") => {
            let scores = store.scores(project, None, None, i64::MAX as usize, 0)?.0;
            let mut groups = BTreeMap::<(String, String, String), (usize, usize, f64)>::new();
            for score in scores {
                if !includes(&score, &input["filter"]) {
                    continue;
                }
                let (Some(name), Some(source), Some(kind)) = (
                    score["name"].as_str(),
                    score["source"].as_str(),
                    score["dataType"].as_str(),
                ) else {
                    continue;
                };
                let item = groups
                    .entry((name.to_owned(), source.to_owned(), kind.to_owned()))
                    .or_default();
                item.0 += 1;
                if let Some(value) = score["value"].as_f64() {
                    item.1 += 1;
                    item.2 += value;
                }
            }
            Ok(json!(
                groups
                    .into_iter()
                    .map(|((name, source, kind), (count, n, sum))| json!({
                        "scoreName": name, "scoreSource": source, "scoreDataType": kind,
                        "countScoreId": count, "avgValue": (n > 0).then(|| sum / n as f64),
                    }))
                    .collect::<Vec<_>>()
            ))
        }
        Some(
            key
            @ ("observations-usage-by-type-timeseries" | "observations-cost-by-type-timeseries"),
        ) => {
            let cost = key.contains("-cost-");
            let rows = store
                .observations(project, None, None, i64::MAX as usize, 0)?
                .0;
            let filters = &input["filter"];
            let from = filters
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|filter| {
                    (filter["type"] == "datetime"
                        && matches!(filter["operator"].as_str(), Some(">" | ">=")))
                    .then(|| parse_time(&filter["value"]))
                    .flatten()
                })
                .min();
            let day = 86_400_000_000_000_i64;
            let interval = if from.is_some_and(|from| now_ns().saturating_sub(from) < 2 * day) {
                day / 24
            } else {
                day
            };
            let mut groups = BTreeMap::<(i64, String, String), f64>::new();
            for observation in rows {
                if !includes(&observation.public(project), filters) {
                    continue;
                }
                let Some(details) = (if cost {
                    &observation.cost
                } else {
                    &observation.usage
                })
                .as_object() else {
                    continue;
                };
                for (kind, value) in details {
                    if kind == "total" {
                        continue;
                    }
                    let Some(value) = value.as_f64() else {
                        continue;
                    };
                    let key = (
                        observation.start_ns.div_euclid(interval) * interval,
                        observation.model.clone().unwrap_or_default(),
                        kind.clone(),
                    );
                    *groups.entry(key).or_default() += value;
                }
            }
            Ok(json!(groups.into_iter().map(|((start, model, kind), sum)| {
                if cost { json!({"intervalStart":iso(start),"model":model,"costType":kind,"sumCost":sum}) }
                else { json!({"intervalStart":iso(start),"model":model,"usageType":kind,"sumTokens":sum}) }
            }).collect::<Vec<_>>()))
        }
        _ => Err(fail("NOT_FOUND", "Chart query not found")),
    }
}

pub(super) fn score_columns(store: &Store, project: &str, input: &Value) -> Result<Value> {
    let within = |row: &Value| {
        parse_time(&row["timestamp"]).is_some_and(|time| {
            parse_time(&input["fromTimestamp"]).is_none_or(|from| time >= from)
                && parse_time(&input["toTimestamp"]).is_none_or(|to| time < to)
        })
    };
    let mut columns = BTreeMap::new();
    for row in store.scores(project, None, None, i64::MAX as usize, 0)?.0 {
        if !includes(&row, &input["filter"]) || !within(&row) {
            continue;
        }
        let (Some(name), Some(source), Some(kind)) = (
            row["name"].as_str(),
            row["source"].as_str(),
            row["dataType"].as_str(),
        ) else {
            continue;
        };
        let key = format!("{}-{source}-{kind}", name.replace(['-', '.'], "_"));
        columns.insert(
            key.clone(),
            json!({ "key": key, "name": name, "source": source, "dataType": kind }),
        );
    }
    Ok(json!({ "scoreColumns": columns.into_values().collect::<Vec<_>>() }))
}

pub(super) fn last_trace_by_project(store: &Store) -> Result<Value> {
    let mut rows = BTreeMap::new();
    for project in store.projects()? {
        if let Some(trace) = store
            .traces(&project.id, &TraceFilter::default(), 1, 0)?
            .0
            .first()
        {
            rows.insert(project.id, iso(trace.start_ns));
        }
    }
    Ok(json!(rows))
}
