use super::{
    fail,
    tracing::{includes, paginate},
};
use crate::{desktop::required, model::TraceFilter, store::Store};
use anyhow::{Context, Result, bail};
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const CONFIGS: &str = "score-configs";

pub(super) fn request(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    if name.starts_with("scoreConfigs.") {
        return config(store, project, name, input);
    }
    let value = match name {
        "scores.hasAny" => json!(store.scores(project, None, None, 0, 0)?.1 > 0),
        "scores.all" | "scores.allFromEvents" | "scores.countAll" | "scores.countAllFromEvents" => {
            let data = list(store, project, input)?;
            if name.contains("countAll") {
                json!({"totalCount":data.len()})
            } else {
                json!({"scores":paginate(data,input)?})
            }
        }
        "scores.byId" | "scores.getScoreMetadataById" => {
            let id = input["scoreId"]
                .as_str()
                .or(input["id"].as_str())
                .context("Score ID is required")?;
            let row = store
                .score(project, id)?
                .ok_or_else(|| fail("NOT_FOUND", "Score not found"))?;
            if name == "scores.getScoreMetadataById" {
                row["metadata"].clone()
            } else {
                score_wire(store, project, row)?
            }
        }
        "scores.metricsFromEvents" => {
            let ids = input["traceIds"]
                .as_array()
                .filter(|ids| ids.len() <= 500)
                .context("Choose up to 500 traces")?;
            let mut rows = Vec::new();
            for id in ids {
                let id = id.as_str().context("Invalid trace ID")?;
                if let Some(trace) = store.trace(project, id)? {
                    rows.push(json!({"traceId":id,"traceName":trace.name,"userId":trace.user_id,"tags":trace.tags}));
                }
            }
            json!(rows)
        }
        "scores.filterOptions" | "scores.filterOptionsFromEvents" => {
            let rows = list(store, project, input)?;
            json!({
                "name": choices(&rows,"name"),"tags":choices(&rows,"traceTags"),
                "traceName":choices(&rows,"traceName"),"userId":choices(&rows,"traceUserId"),
                "stringValue":choices(&rows,"stringValue"),"booleanValue":[{"value":"True"},{"value":"False"}],
            })
        }
        "scores.createAnnotationScore" | "scores.updateAnnotationScore" => {
            annotation(store, project, name, input)?
        }
        "scores.deleteAnnotationScore" => {
            let id = required(input, "id")?;
            let row = store
                .score(project, id)?
                .filter(|row| row["source"] == "ANNOTATION" || row["dataType"] == "CORRECTION")
                .ok_or_else(|| fail("NOT_FOUND", "Annotation score not found"))?;
            store.delete_score(project, id)?;
            score_wire(store, project, row)?
        }
        "scores.deleteMany" => {
            let ids = input["scoreIds"]
                .as_array()
                .filter(|ids| !ids.is_empty() && ids.len() <= 500)
                .context("Select 1 to 500 scores")?;
            store.transaction(|tx| {
                for id in ids {
                    tx.execute(
                        "DELETE FROM scores WHERE project=?1 AND id=?2",
                        params![project, id.as_str().context("Invalid score ID")?],
                    )?;
                }
                Ok(Value::Null)
            })?
        }
        _ => {
            return Err(fail(
                "NOT_IMPLEMENTED",
                format!("Workspace procedure is unavailable: {name}"),
            ));
        }
    };
    Ok(value)
}

fn list(store: &Store, project: &str, input: &Value) -> Result<Vec<Value>> {
    let rows = store.scores(project, None, None, i64::MAX as usize, 0)?.0;
    let mut data = Vec::new();
    for row in rows {
        let row = score_wire(store, project, row)?;
        if row["dataType"] != "CORRECTION"
            && includes(&row, &input["filter"])
            && includes(&row, &input["timestampFilter"])
        {
            data.push(row);
        }
    }
    Ok(data)
}

fn score_wire(store: &Store, project: &str, mut row: Value) -> Result<Value> {
    let trace = row["traceId"]
        .as_str()
        .map(|id| store.trace(project, id))
        .transpose()?
        .flatten();
    row["traceName"] = json!(trace.as_ref().map(|trace| &trace.name));
    row["traceUserId"] = json!(trace.as_ref().and_then(|trace| trace.user_id.as_ref()));
    row["traceTags"] = json!(trace.as_ref().map(|trace| &trace.tags));
    row["hasMetadata"] = json!(
        row["metadata"]
            .as_object()
            .is_some_and(|metadata| !metadata.is_empty())
    );
    row["projectId"] = json!(project);
    if row["environment"].is_null() {
        row["environment"] = json!(
            trace
                .as_ref()
                .map(|trace| trace.environment.as_str())
                .unwrap_or("default")
        );
    }
    if row["dataType"] == "BOOLEAN" && row["stringValue"].is_null() {
        let enabled = row["value"] == true || row["value"] == 1;
        row["value"] = json!(if enabled { 1 } else { 0 });
        row["stringValue"] = json!(if enabled { "True" } else { "False" });
    }
    if !row["value"].is_number() && (row["dataType"] == "CATEGORICAL" || row["dataType"] == "TEXT")
    {
        row["value"] = json!(0);
    }
    if row["authorUserId"] == "confuse-local" {
        row["authorUserName"] = json!("Local");
    }
    Ok(row)
}

fn annotation(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    let config_id = required(input, "configId")?;
    let config = store
        .resource(project, CONFIGS, config_id)?
        .filter(|row| row["id"] == config_id)
        .ok_or_else(|| fail("NOT_FOUND", "Score configuration not found"))?;
    if config["isArchived"] == true || config["dataType"] != input["dataType"] {
        bail!("The score does not match an active configuration");
    }
    let mut row = input.clone();
    match input["scoreTarget"]["type"].as_str() {
        Some("trace") => {
            let target = &input["scoreTarget"];
            let id = required(target, "traceId")?;
            if store.trace(project, id)?.is_none() {
                return Err(fail("NOT_FOUND", "Trace not found"));
            }
            if let Some(obs) = target["observationId"].as_str() {
                let allowed = store.access(|conn| Ok(conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM observations WHERE project=?1 AND trace=?2 AND id=?3)",
                    params![project,id,obs],|row|row.get::<_,bool>(0),
                )?))?;
                if !allowed {
                    return Err(fail("NOT_FOUND", "Observation not found"));
                }
            }
            row["traceId"] = json!(id);
            row["observationId"] = target["observationId"].clone();
            row["sessionId"] = Value::Null;
        }
        Some("session") => {
            let id = required(&input["scoreTarget"], "sessionId")?;
            if store
                .traces(
                    project,
                    &TraceFilter {
                        session: Some(id),
                        ..Default::default()
                    },
                    0,
                    0,
                )?
                .1
                == 0
            {
                return Err(fail("NOT_FOUND", "Session not found"));
            }
            row["sessionId"] = json!(id);
            row["traceId"] = Value::Null;
            row["observationId"] = Value::Null;
        }
        _ => bail!("Choose a trace or a session"),
    }
    if let Some(id) = input["id"].as_str() {
        if let Some(previous) = store.score(project, id)? {
            if previous["source"] != "ANNOTATION"
                || previous["configId"] != config_id
                || ["traceId", "sessionId", "observationId"]
                    .iter()
                    .any(|key| previous[*key] != row[*key])
            {
                return Err(fail("FORBIDDEN", "Score belongs to another target"));
            }
        } else if name == "scores.updateAnnotationScore" {
            return Err(fail("NOT_FOUND", "Score not found"));
        }
    }
    let value = input["value"].as_f64().context("Score value is required")?;
    match config["dataType"].as_str() {
        Some("NUMERIC") => {
            if config["minValue"].as_f64().is_some_and(|min| value < min)
                || config["maxValue"].as_f64().is_some_and(|max| value > max)
            {
                bail!("Score is outside the configured range");
            }
        }
        Some("BOOLEAN") => {
            row["value"] = match input["stringValue"].as_str() {
                Some("True") => json!(1),
                Some("False") => json!(0),
                _ => bail!("Choose True or False"),
            };
        }
        Some("CATEGORICAL") => {
            let options = config["categories"]
                .as_array()
                .context("Score config has no categories")?;
            let selected = options
                .iter()
                .find(|category| category["label"] == input["stringValue"])
                .context("Choose a value from the score configuration")?;
            row["value"] = selected["value"].clone();
        }
        Some("TEXT") => {
            let _ = required(input, "stringValue")?;
        }
        _ => bail!("Unsupported annotation score type"),
    }
    row["name"] = config["name"].clone();
    row["source"] = json!("ANNOTATION");
    row["authorUserId"] = json!("confuse-local");
    if let Some(fields) = row.as_object_mut() {
        fields.remove("scoreTarget");
    }
    score_wire(store, project, store.put_score(project, row)?)
}

fn config(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    match name {
        "scoreConfigs.all" => {
            let rows = store.resources(project, CONFIGS)?;
            let count = rows.len();
            let rows = if input["limit"].is_u64() {
                paginate(rows, input)?
            } else {
                rows
            };
            Ok(json!({"configs":rows,"totalCount":count}))
        }
        "scoreConfigs.byId" => Ok(store
            .resource(project, CONFIGS, required(input, "id")?)?
            .unwrap_or(Value::Null)),
        "scoreConfigs.create" | "scoreConfigs.update" => {
            let id = if name == "scoreConfigs.update" {
                Some(required(input, "id")?)
            } else {
                None
            };
            let mut data = if let Some(id) = id {
                store
                    .resource(project, CONFIGS, id)?
                    .filter(|row| row["id"] == id)
                    .ok_or_else(|| fail("NOT_FOUND", "Score configuration not found"))?
            } else {
                json!({"name":required(input,"name")?,"dataType":required(input,"dataType")?,"isArchived":false})
            };
            for key in [
                "name",
                "description",
                "minValue",
                "maxValue",
                "categories",
                "isArchived",
            ] {
                if let Some(value) = input.get(key) {
                    data[key] = value.clone();
                }
            }
            let _ = required(&data, "name")?;
            if !matches!(
                data["dataType"].as_str(),
                Some("NUMERIC" | "CATEGORICAL" | "BOOLEAN" | "TEXT")
            ) {
                bail!("Invalid score type");
            }
            if data["minValue"]
                .as_f64()
                .zip(data["maxValue"].as_f64())
                .is_some_and(|(min, max)| min > max)
            {
                bail!("The minimum cannot exceed the maximum");
            }
            Ok(store.put_resource(project, CONFIGS, data)?)
        }
        _ => Err(fail(
            "NOT_IMPLEMENTED",
            format!("Workspace procedure is unavailable: {name}"),
        )),
    }
}

fn choices(rows: &[Value], field: &str) -> Vec<Value> {
    let mut options = BTreeMap::new();
    for row in rows {
        if let Some(values) = row[field].as_array() {
            for value in values.iter().filter_map(Value::as_str) {
                *options.entry(value).or_insert(0) += 1;
            }
        } else if let Some(value) = row[field].as_str() {
            *options.entry(value).or_insert(0) += 1;
        }
    }
    options
        .into_iter()
        .take(500)
        .map(|(value, count)| json!({"value":value,"count":count}))
        .collect()
}
