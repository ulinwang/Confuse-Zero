use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Trace {
    pub id: String,
    pub name: String,
    pub source: String,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub environment: String,
    pub input: Value,
    pub output: Value,
    pub metadata: Value,
    pub tags: Vec<String>,
    pub version: Option<String>,
    pub release: Option<String>,
    pub start_ns: i64,
    pub end_ns: i64,
    pub received_ns: i64,
}

impl Trace {
    pub fn public(&self) -> Value {
        json!({
            "id": self.id, "name": self.name, "timestamp": iso(self.start_ns),
            "sessionId": self.session_id, "userId": self.user_id, "source": self.source,
            "environment": self.environment, "input": self.input, "output": self.output,
            "metadata": self.metadata, "tags": self.tags, "version": self.version,
            "release": self.release, "public": false, "bookmarked": false,
            "updatedAt": iso(self.received_ns), "latency": seconds(self.start_ns, self.end_ns)
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Observation {
    pub id: String,
    pub trace_id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub name: String,
    pub model: Option<String>,
    pub level: String,
    pub status_message: Option<String>,
    pub environment: String,
    pub input: Value,
    pub output: Value,
    pub metadata: Value,
    pub model_parameters: Value,
    pub usage: Value,
    pub cost: Value,
    pub prompt_name: Option<String>,
    pub prompt_version: Option<i64>,
    pub version: Option<String>,
    pub start_ns: i64,
    pub end_ns: i64,
    pub completion_start_ns: Option<i64>,
}

impl Observation {
    pub fn public(&self, project: &str) -> Value {
        json!({
            "id": self.id, "traceId": self.trace_id, "parentObservationId": self.parent_id,
            "projectId": project, "type": self.kind, "name": self.name,
            "model": self.model, "providedModelName": self.model, "level": self.level,
            "statusMessage": self.status_message, "environment": self.environment,
            "input": self.input, "output": self.output, "metadata": self.metadata,
            "modelParameters": self.model_parameters, "usageDetails": self.usage,
            "costDetails": self.cost, "usage": {
                "input": self.usage.get("input"), "output": self.usage.get("output"),
                "total": usage_total(&self.usage), "unit": "TOKENS"
            },
            "promptName": self.prompt_name, "promptVersion": self.prompt_version,
            "version": self.version, "startTime": iso(self.start_ns), "endTime": iso(self.end_ns),
            "completionStartTime": self.completion_start_ns.map(iso),
            "latency": seconds(self.start_ns, self.end_ns),
            "timeToFirstToken": self.completion_start_ns.map(|ns| seconds(self.start_ns, ns)),
            "calculatedTotalCost": cost_total(&self.cost), "totalCost": cost_total(&self.cost)
        })
    }
}

#[derive(Clone, Debug)]
pub struct Record {
    pub trace: Trace,
    pub observation: Option<Observation>,
    pub root: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TraceFilter<'a> {
    pub query: Option<&'a str>,
    pub session: Option<&'a str>,
    pub name: Option<&'a str>,
    pub user: Option<&'a str>,
    pub source: Option<&'a str>,
    pub from: Option<i64>,
    pub to: Option<i64>,
}

pub fn now_ns() -> i64 {
    Utc::now().timestamp_nanos_opt().unwrap_or_default()
}

pub fn iso(ns: i64) -> String {
    DateTime::<Utc>::from_timestamp_nanos(ns).to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn parse_time(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_f64() {
        let scale = if number >= 1e16 {
            1.
        } else if number >= 1e11 {
            1e6
        } else {
            1e9
        };
        return Some((number * scale).round() as i64);
    }
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()?
        .timestamp_nanos_opt()
}

pub fn stable_id(parts: &[&str], bytes: usize) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.len().to_le_bytes());
        digest.update(part);
    }
    hex::encode(&digest.finalize()[..bytes])
}

pub fn seconds(start: i64, end: i64) -> f64 {
    end.saturating_sub(start).max(0) as f64 / 1e9
}

pub fn usage_total(usage: &Value) -> Option<i64> {
    usage.get("total").and_then(Value::as_i64).or_else(|| {
        let input = usage.get("input").and_then(Value::as_i64);
        let output = usage.get("output").and_then(Value::as_i64);
        (input.is_some() || output.is_some()).then(|| {
            input
                .unwrap_or_default()
                .saturating_add(output.unwrap_or_default())
        })
    })
}

pub fn cost_total(cost: &Value) -> Option<f64> {
    cost.get("total").and_then(Value::as_f64).or_else(|| {
        let object = cost.as_object()?;
        if object.is_empty() {
            return None;
        }
        Some(object.values().filter_map(Value::as_f64).sum())
    })
}
