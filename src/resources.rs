use crate::{
    model::{TraceFilter, cost_total, iso, now_ns, usage_total},
    store::{Store, decode},
};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const KINDS: &[&str] = &[
    "prompts",
    "skills",
    "datasets",
    "dataset-items",
    "experiments",
    "experiment-items",
    "evaluators",
    "evaluation-rules",
    "evaluation-errors",
    "annotation-queues",
    "annotation-items",
    "comments",
    "models",
    "llm-connections",
    "score-configs",
    "dashboards",
    "dashboard-widgets",
    "alerts",
    "exports",
    "topic-maps",
    "topic-runs",
    "topic-publications",
];

impl Store {
    pub fn resources(&self, project: &str, kind: &str) -> Result<Vec<Value>> {
        validate_kind(kind)?;
        self.access(|conn|{
            let mut stmt=conn.prepare("SELECT data FROM resources WHERE project=?1 AND kind=?2 ORDER BY created_ns DESC,id")?;
            stmt.query_map(params![project,kind],|r|r.get::<_,String>(0))?
                .map(|row|decode(&row?)).collect()
        })
    }

    pub fn resource(&self, project: &str, kind: &str, id: &str) -> Result<Option<Value>> {
        validate_kind(kind)?;
        self.access(|conn| {
            let row: Option<String> = conn
                .query_row(
                    "SELECT data FROM resources WHERE project=?1 AND kind=?2 AND (id=?3 OR name=?3)
                ORDER BY id=?3 DESC,created_ns DESC LIMIT 1",
                    params![project, kind, id],
                    |r| r.get(0),
                )
                .optional()?;
            row.map(|value| decode(&value)).transpose()
        })
    }

    pub fn put_resource(&self, project: &str, kind: &str, mut data: Value) -> Result<Value> {
        validate_kind(kind)?;
        if matches!(kind, "prompts" | "skills") {
            return self.new_version(project, kind, data);
        }
        let fields = data.as_object_mut().context("Expected a JSON object")?;
        let id = fields
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        self.transaction(|tx|{
            let raw:Option<String>=tx.query_row("SELECT data FROM resources WHERE project=?1 AND kind=?2 AND id=?3",
                params![project,kind,id],|r|r.get(0)).optional()?;
            let mut value=raw.as_deref().map(decode::<Value>).transpose()?.unwrap_or_else(||json!({}));
            let object=value.as_object_mut().context("Resource is invalid")?;
            object.extend(std::mem::take(fields));
            let now=now_ns();
            object.insert("id".to_owned(),json!(id));
            object.insert("projectId".to_owned(),json!(project));
            object.insert("updatedAt".to_owned(),json!(iso(now)));
            object.entry("createdAt").or_insert(json!(iso(now)));
            if kind=="datasets" {
                object.entry("metadata").or_insert(json!(null));
                object.entry("description").or_insert(json!(null));
            }
            if kind=="dataset-items" {
                let dataset=object.get("datasetName").or_else(||object.get("datasetId"))
                    .and_then(Value::as_str).context("Choose a dataset")?;
                let exist:Option<String>=tx.query_row("SELECT data FROM resources WHERE project=?1 AND kind='datasets' AND (id=?2 OR name=?2)",
                    params![project,dataset],|r|r.get(0)).optional()?;
                let existing:Value=decode(exist.as_deref().context("Dataset not found")?)?;
                object.insert("datasetId".to_owned(),existing["id"].clone());
                object.insert("datasetName".to_owned(),existing["name"].clone());
                if object.get("status").is_none_or(Value::is_null) {
                    object.insert("status".to_owned(),json!("ACTIVE"));
                }
                object.entry("mediaReferences").or_insert(json!([]));
                for key in ["input","expectedOutput","metadata","sourceTraceId","sourceObservationId"] {
                    object.entry(key).or_insert(Value::Null);
                }
            }
            let name=value.get("name").and_then(Value::as_str).unwrap_or_default();
            if kind=="datasets" && name.is_empty() {bail!("Dataset name is required");}
            tx.execute("INSERT INTO resources(project,kind,id,name,created_ns,data) VALUES(?1,?2,?3,?4,?5,?6)
                ON CONFLICT(project,kind,id) DO UPDATE SET name=excluded.name,data=excluded.data",
                params![project,kind,id,name,now,serde_json::to_string(&value)?])?;
            Ok(value)
        })
    }

    pub fn delete_resource(&self, project: &str, kind: &str, id: &str) -> Result<bool> {
        validate_kind(kind)?;
        self.transaction(|conn|{
            let changed=conn.execute("DELETE FROM resources WHERE project=?1 AND kind=?2 AND (id=?3 OR name=?3)",
                params![project,kind,id])?;
            if kind=="datasets" {
                conn.execute("DELETE FROM resources WHERE project=?1 AND kind IN ('dataset-items','experiments') AND (
                    json_extract(data,'$.datasetId')=?2 OR json_extract(data,'$.datasetName')=?2)",params![project,id])?;
            }
            Ok(changed>0)
        })
    }

    pub fn versioned(
        &self,
        project: &str,
        kind: &str,
        name: &str,
        version: Option<i64>,
        label: Option<&str>,
    ) -> Result<Option<Value>> {
        validate_kind(kind)?;
        let label = label.or_else(|| version.is_none().then_some("production"));
        self.access(|conn| {
            let row: Option<String> = conn
                .query_row(
                    "SELECT data FROM resources WHERE project=?1 AND kind=?2 AND name=?3
                AND (?4 IS NULL OR json_extract(data,'$.version')=?4) AND (?5 IS NULL OR EXISTS(
                    SELECT 1 FROM json_each(resources.data, '$.labels') WHERE value=?5))
                ORDER BY json_extract(data,'$.version') DESC LIMIT 1",
                    params![project, kind, name, version, label],
                    |r| r.get(0),
                )
                .optional()?;
            row.map(|data| decode(&data)).transpose()
        })
    }

    pub fn set_version_labels(
        &self,
        project: &str,
        kind: &str,
        name: &str,
        version: i64,
        labels: &[String],
    ) -> Result<Value> {
        self.transaction(|tx| {
            let mut rows = tx.prepare(
                "SELECT id,data FROM resources WHERE project=?1 AND kind=?2 AND name=?3",
            )?;
            let existing = rows
                .query_map(params![project, kind, name], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut selected = None;
            for (id, raw) in existing {
                let mut item: Value = decode(&raw)?;
                if item["version"] == version {
                    let mut next = string_list(&item["labels"]);
                    if kind == "skills" {
                        next.retain(|label| label == "latest");
                    }
                    for label in labels {
                        if !next.contains(label) {
                            next.push(label.clone());
                        }
                    }
                    item["labels"] = json!(next);
                    selected = Some(item.clone());
                } else {
                    let mut previous = string_list(&item["labels"]);
                    previous.retain(|label| !labels.contains(label));
                    item["labels"] = json!(previous);
                }
                tx.execute(
                    "UPDATE resources SET data=?4 WHERE project=?1 AND kind=?2 AND id=?3",
                    params![project, kind, id, serde_json::to_string(&item)?],
                )?;
            }
            selected.context("Version not found")
        })
    }

    fn new_version(&self, project: &str, kind: &str, mut value: Value) -> Result<Value> {
        if kind == "skills" {
            crate::skills::normalize(self, project, &mut value)?;
        }
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .context("Enter a name")?
            .to_owned();
        if kind == "prompts" && !(value["prompt"].is_string() || value["prompt"].is_array()) {
            bail!("Provide a text or chat prompt");
        }
        if kind == "skills" && !(value["content"].is_string() || value["files"].is_array()) {
            bail!("Provide skill content or files");
        }
        let mut labels = string_list(&value["labels"]);
        labels.sort();
        labels.dedup();
        let now = now_ns();
        self.transaction(|tx|{
            let mut stmt=tx.prepare("SELECT id,data FROM resources WHERE project=?1 AND kind=?2 AND name=?3")?;
            let existing=stmt.query_map(params![project,kind,name],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut version=0;
            for (id,data) in existing {
                let mut item:Value=decode(&data)?;
                version=version.max(item["version"].as_i64().unwrap_or(0));
                let mut previous=string_list(&item["labels"]);
                previous.retain(|label|!labels.contains(label)&&label!="latest");
                item["labels"]=json!(previous);
                tx.execute("UPDATE resources SET data=?4 WHERE project=?1 AND kind=?2 AND id=?3",
                    params![project,kind,id,serde_json::to_string(&item)?])?;
            }
            if !labels.iter().any(|s|s=="latest") {labels.push("latest".to_owned());}
            let id=Uuid::new_v4().to_string();
            value["id"]=json!(id);
            value["version"]=json!(version+1);
            value["labels"]=json!(labels);
            value["tags"]=json!(string_list(&value["tags"]));
            value["createdAt"]=json!(iso(now));
            value["updatedAt"]=json!(iso(now));
            value["projectId"]=json!(project);
            if kind=="prompts" {
                value["type"]=json!(if value["prompt"].is_array() {"chat"}else{"text"});
                if value["config"].is_null(){value["config"]=json!({});}
            }
            tx.execute("INSERT INTO resources(project,kind,id,name,created_ns,data) VALUES(?1,?2,?3,?4,?5,?6)",
                params![project,kind,id,name,now,serde_json::to_string(&value)?])?;
            Ok(value)
        })
    }

    pub fn metrics(&self, project: &str, query: &Value) -> Result<Value> {
        let view = query["view"].as_str().unwrap_or("observations");
        let dimensions: Vec<_> = query["dimensions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["field"].as_str().or_else(|| item.as_str()))
            .collect();
        let requests: Vec<(&str, &str)> = query["metrics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| Some((item["measure"].as_str()?, item["aggregation"].as_str()?)))
            .collect();
        let from = query
            .get("fromTimestamp")
            .and_then(crate::model::parse_time);
        let to = query.get("toTimestamp").and_then(crate::model::parse_time);
        let allowed_time =
            |time: i64| from.is_none_or(|from| time >= from) && to.is_none_or(|to| time < to);
        let mut grouped: BTreeMap<Vec<String>, Vec<Value>> = BTreeMap::new();
        let mut offset = 0;
        loop {
            let data = if view == "traces" {
                self.traces(
                    project,
                    &TraceFilter {
                        from,
                        to,
                        ..Default::default()
                    },
                    1000,
                    offset,
                )?
                .0
                .into_iter()
                .map(|row| row.public())
                .collect::<Vec<_>>()
            } else if matches!(view, "scores-numeric" | "scores-categorical" | "scores") {
                self.scores(project, None, None, 1000, offset)?.0
            } else {
                let rows = self.observations(project, None, None, 1000, offset)?.0;
                if rows.is_empty() {
                    break;
                }
                offset += rows.len();
                let rows = rows
                    .into_iter()
                    .filter(|row| allowed_time(row.start_ns))
                    .map(|row| {
                        let mut value = row.public(project);
                        value["totalTokens"] = json!(usage_total(&row.usage));
                        value["totalCost"] = json!(cost_total(&row.cost));
                        value["inputTokens"] = json!(row.usage["input"]);
                        value["outputTokens"] = json!(row.usage["output"]);
                        value
                    });
                for value in rows {
                    group(&mut grouped, &dimensions, query, value);
                }
                continue;
            };
            if data.is_empty() {
                break;
            }
            offset += data.len();
            for row in data {
                if row
                    .get("timestamp")
                    .and_then(crate::model::parse_time)
                    .is_none_or(allowed_time)
                {
                    group(&mut grouped, &dimensions, query, row);
                }
            }
        }
        if grouped.is_empty() && dimensions.is_empty() {
            grouped.insert(Vec::new(), Vec::new());
        }
        let mut data = Vec::new();
        for (keys, rows) in grouped {
            let mut item = serde_json::Map::new();
            for (name, value) in dimensions.iter().zip(keys) {
                item.insert((*name).to_owned(), json!(value));
            }
            for (measure, aggregation) in &requests {
                let mut values = rows
                    .iter()
                    .filter_map(|row| row[*measure].as_f64())
                    .collect::<Vec<_>>();
                let value = match *aggregation {
                    "count" => json!(rows.len()),
                    "sum" => json!(values.iter().sum::<f64>()),
                    "avg" => {
                        if values.is_empty() {
                            Value::Null
                        } else {
                            json!(values.iter().sum::<f64>() / values.len() as f64)
                        }
                    }
                    "min" => json!(values.iter().copied().reduce(f64::min)),
                    "max" => json!(values.iter().copied().reduce(f64::max)),
                    "p50" | "p90" | "p95" | "p99" => {
                        values.sort_by(f64::total_cmp);
                        let percentile = aggregation
                            .trim_start_matches('p')
                            .parse::<f64>()
                            .unwrap_or(50.)
                            / 100.;
                        json!(values.get(
                            (percentile * (values.len().saturating_sub(1) as f64)).round() as usize
                        ))
                    }
                    _ => Value::Null,
                };
                item.insert(format!("{aggregation}_{measure}"), value);
            }
            data.push(Value::Object(item));
        }
        Ok(json!({"data":data}))
    }
}

fn group(
    grouped: &mut BTreeMap<Vec<String>, Vec<Value>>,
    dimensions: &[&str],
    query: &Value,
    row: Value,
) {
    if !matches_filters(&row, &query["filters"]) {
        return;
    }
    let keys = dimensions
        .iter()
        .map(|field| row[*field].as_str().unwrap_or("-").to_owned())
        .collect();
    grouped.entry(keys).or_default().push(row);
}

pub fn matches_filters(value: &Value, filters: &Value) -> bool {
    filters.as_array().is_none_or(|conditions| {
        conditions.iter().all(|filter| {
            let column = filter["column"].as_str().unwrap_or_default();
            let compare = &filter["value"];
            let field = &value[column];
            let text = field.as_str().unwrap_or_default().to_lowercase();
            match filter["operator"].as_str().unwrap_or("=") {
                "=" | "is" => field == compare,
                "!=" | "is not" => field != compare,
                "contains" => text.contains(&compare.as_str().unwrap_or_default().to_lowercase()),
                "does not contain" => {
                    !text.contains(&compare.as_str().unwrap_or_default().to_lowercase())
                }
                "any of" | "in" => compare.as_array().is_some_and(|list| list.contains(field)),
                "none of" => compare.as_array().is_some_and(|list| !list.contains(field)),
                ">" => field
                    .as_f64()
                    .zip(compare.as_f64())
                    .is_some_and(|(a, b)| a > b),
                ">=" => field
                    .as_f64()
                    .zip(compare.as_f64())
                    .is_some_and(|(a, b)| a >= b),
                "<" => field
                    .as_f64()
                    .zip(compare.as_f64())
                    .is_some_and(|(a, b)| a < b),
                "<=" => field
                    .as_f64()
                    .zip(compare.as_f64())
                    .is_some_and(|(a, b)| a <= b),
                _ => false,
            }
        })
    })
}

pub fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn validate_kind(kind: &str) -> Result<()> {
    if !KINDS.contains(&kind) {
        bail!("Unknown workspace resource");
    }
    Ok(())
}
