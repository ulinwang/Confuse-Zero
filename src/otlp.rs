use crate::model::{Observation, Record, Trace, parse_time};
use anyhow::Result;
use opentelemetry_proto::tonic::{
    collector::trace::v1::{
        ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse,
    },
    common::v1::{AnyValue, KeyValue, any_value},
    trace::v1::{Span, Status},
};
use prost::Message;
use serde_json::{Map, Value, json};

pub struct Batch {
    pub records: Vec<Record>,
    pub rejected: i64,
}

impl Batch {
    pub fn decode(bytes: &[u8], is_json: bool, received_ns: i64) -> Result<Self> {
        let request = if is_json {
            serde_json::from_slice(bytes)?
        } else {
            ExportTraceServiceRequest::decode(bytes)?
        };
        let mut result = Self {
            records: Vec::new(),
            rejected: 0,
        };
        for resource_spans in request.resource_spans {
            let resource = resource_spans
                .resource
                .map(|r| attributes(&r.attributes))
                .unwrap_or_default();
            for scope in resource_spans.scope_spans {
                for span in scope.spans {
                    match record(span, &resource, received_ns) {
                        Some(record) => result.records.push(record),
                        None => result.rejected += 1,
                    }
                }
            }
        }
        Ok(result)
    }

    pub fn response(&self) -> ExportTraceServiceResponse {
        ExportTraceServiceResponse {
            partial_success: (self.rejected > 0).then(|| ExportTracePartialSuccess {
                rejected_spans: self.rejected,
                error_message: "Invalid trace or span ID".to_owned(),
            }),
        }
    }
}

fn record(span: Span, resource: &Map<String, Value>, received_ns: i64) -> Option<Record> {
    if span.trace_id.len() != 16
        || span.span_id.len() != 8
        || span.trace_id.iter().all(|&b| b == 0)
        || span.span_id.iter().all(|&b| b == 0)
        || (!span.parent_span_id.is_empty() && span.parent_span_id.len() != 8)
    {
        return None;
    }
    let trace_id = hex::encode(&span.trace_id);
    let id = hex::encode(&span.span_id);
    let parent_id = (!span.parent_span_id.is_empty()
        && span.parent_span_id.iter().any(|&b| b != 0))
    .then(|| hex::encode(&span.parent_span_id));
    let root = parent_id.is_none();
    let attrs = attributes(&span.attributes);
    let model = text(
        &attrs,
        &[
            "langfuse.observation.model.name",
            "gen_ai.request.model",
            "gen_ai.response.model",
            "llm.model_name",
            "model",
        ],
    );
    let known = [
        "SPAN",
        "GENERATION",
        "EVENT",
        "EMBEDDING",
        "AGENT",
        "TOOL",
        "CHAIN",
        "RETRIEVER",
        "GUARDRAIL",
        "EVALUATOR",
    ];
    let kind = text(&attrs, &["langfuse.observation.type"])
        .map(|kind| kind.to_ascii_uppercase())
        .filter(|kind| known.contains(&kind.as_str()))
        .or_else(|| infer_kind(&attrs))
        .unwrap_or_else(|| {
            if model.is_some() {
                "GENERATION"
            } else {
                "SPAN"
            }
            .to_owned()
        });
    let input = structured(
        &attrs,
        &[
            "langfuse.observation.input",
            "gen_ai.input.messages",
            "gen_ai.prompt",
            "input.value",
            "gen_ai.tool.call.arguments",
        ],
    );
    let output = structured(
        &attrs,
        &[
            "langfuse.observation.output",
            "gen_ai.output.messages",
            "gen_ai.completion",
            "output.value",
            "gen_ai.tool.call.result",
        ],
    );
    let trace_input = structured(&attrs, &["langfuse.trace.input"]);
    let trace_output = structured(&attrs, &["langfuse.trace.output"]);
    let environment = text(
        &attrs,
        &[
            "langfuse.environment",
            "deployment.environment.name",
            "deployment.environment",
        ],
    )
    .or_else(|| {
        text(
            resource,
            &["deployment.environment.name", "deployment.environment"],
        )
    })
    .unwrap_or_else(|| "default".to_owned());
    let start = i64::try_from(span.start_time_unix_nano).unwrap_or_default();
    let end = i64::try_from(span.end_time_unix_nano).unwrap_or_default();
    let start = if start == 0 {
        if end == 0 { received_ns } else { end }
    } else {
        start
    };
    let end = if end == 0 { start } else { end.max(start) };
    let mut usage = structured(&attrs, &["langfuse.observation.usage_details"]);
    if !usage.is_object() {
        usage = Value::Object(Map::new());
        for (name, keys) in [
            (
                "input",
                &[
                    "gen_ai.usage.input_tokens",
                    "gen_ai.usage.prompt_tokens",
                    "llm.token_count.prompt",
                ][..],
            ),
            (
                "output",
                &[
                    "gen_ai.usage.output_tokens",
                    "gen_ai.usage.completion_tokens",
                    "llm.token_count.completion",
                ][..],
            ),
            (
                "total",
                &["gen_ai.usage.total_tokens", "llm.token_count.total"][..],
            ),
            (
                "input_cached",
                &[
                    "gen_ai.usage.cache_read.input_tokens",
                    "gen_ai.usage.cached_tokens",
                ][..],
            ),
        ] {
            if let Some(value) = integer(&attrs, keys) {
                usage[name] = json!(value);
            }
        }
    }
    let mut cost = structured(&attrs, &["langfuse.observation.cost_details"]);
    if !cost.is_object() {
        cost = number(&attrs, &["gen_ai.usage.cost"])
            .map(|value| json!({"total":value}))
            .unwrap_or_else(|| json!({}));
    }
    let Status { code, message } = span.status.unwrap_or_default();
    let tags = structured(&attrs, &["langfuse.trace.tags"])
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let trace = Trace {
        id: trace_id.clone(),
        name: text(&attrs, &["langfuse.trace.name"]).unwrap_or_else(|| span.name.clone()),
        source: "SDK".to_owned(),
        session_id: text(
            &attrs,
            &[
                "langfuse.trace.session_id",
                "session.id",
                "gen_ai.conversation.id",
            ],
        ),
        user_id: text(&attrs, &["langfuse.trace.user_id", "user.id", "enduser.id"]),
        environment: environment.clone(),
        input: if root && trace_input.is_null() {
            input.clone()
        } else {
            trace_input
        },
        output: if root && trace_output.is_null() {
            output.clone()
        } else {
            trace_output
        },
        metadata: Value::Object(metadata(&attrs, "langfuse.trace.metadata.")),
        tags,
        version: text(&attrs, &["langfuse.trace.version"]),
        release: text(&attrs, &["langfuse.release", "langfuse.trace.release"]),
        start_ns: start,
        end_ns: end,
        received_ns,
    };
    let mut meta = metadata(&attrs, "langfuse.observation.metadata.");
    meta.insert("attributes".to_owned(), json!(attrs));
    if !resource.is_empty() {
        meta.insert("resourceAttributes".to_owned(), json!(resource));
    }
    if !span.events.is_empty() {
        meta.insert("events".to_owned(), Value::Array(span.events.iter().map(|event|json!({
            "name":event.name, "timeUnixNano":event.time_unix_nano,"attributes":attributes(&event.attributes)
        })).collect()));
    }
    let observation = Observation {
        id,
        trace_id,
        parent_id,
        kind,

        name: span.name,
        model,
        level: text(&attrs, &["langfuse.observation.level"])
            .unwrap_or_else(|| if code == 2 { "ERROR" } else { "DEFAULT" }.to_owned()),
        status_message: text(&attrs, &["langfuse.observation.status_message"])
            .or_else(|| (!message.is_empty()).then_some(message)),
        environment,
        input,
        output,
        metadata: Value::Object(meta),
        model_parameters: structured(&attrs, &["langfuse.observation.model.parameters"]),
        usage,
        cost,
        prompt_name: text(
            &attrs,
            &["langfuse.observation.prompt.name", "langfuse.prompt.name"],
        ),
        prompt_version: integer(
            &attrs,
            &[
                "langfuse.observation.prompt.version",
                "langfuse.prompt.version",
            ],
        ),
        version: text(
            &attrs,
            &["langfuse.version", "langfuse.observation.version"],
        ),
        start_ns: start,
        end_ns: end,
        completion_start_ns: attrs
            .get("langfuse.observation.completion_start_time")
            .and_then(parse_time),
    };
    Some(Record {
        trace,
        observation: Some(observation),
        root,
    })
}

fn attributes(values: &[KeyValue]) -> Map<String, Value> {
    values
        .iter()
        .filter_map(|item| Some((item.key.clone(), attribute(item.value.as_ref()?)?)))
        .collect()
}

fn attribute(value: &AnyValue) -> Option<Value> {
    Some(match value.value.as_ref()? {
        any_value::Value::StringValue(value) => json!(value),
        any_value::Value::BoolValue(value) => json!(value),
        any_value::Value::IntValue(value) => json!(value),
        any_value::Value::DoubleValue(value) => json!(value),
        any_value::Value::ArrayValue(value) => {
            Value::Array(value.values.iter().filter_map(attribute).collect())
        }
        any_value::Value::KvlistValue(value) => Value::Object(attributes(&value.values)),
        any_value::Value::BytesValue(value) => json!(hex::encode(value)),
        _ => return None,
    })
}

fn infer_kind(attrs: &Map<String, Value>) -> Option<String> {
    Some(
        match text(attrs, &["gen_ai.operation.name", "openinference.span.kind"])?
            .to_ascii_lowercase()
            .as_str()
        {
            "chat" | "text_completion" | "generate_content" | "llm" => "GENERATION",
            "embeddings" | "embedding" => "EMBEDDING",
            "invoke_agent" | "agent" | "create_agent" => "AGENT",
            "execute_tool" | "tool" => "TOOL",
            "retrieval" | "retriever" => "RETRIEVER",
            "chain" => "CHAIN",
            _ => return None,
        }
        .to_owned(),
    )
}

fn text(attrs: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| {
            attrs
                .get(*key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
        })
        .map(str::to_owned)
}

fn number(attrs: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        attrs
            .get(*key)
            .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse::<f64>().ok()))
    })
}

fn integer(attrs: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        attrs
            .get(*key)
            .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse::<i64>().ok()))
    })
}

fn structured(attrs: &Map<String, Value>, keys: &[&str]) -> Value {
    keys.iter()
        .find_map(|key| attrs.get(*key))
        .map(|value| match value.as_str() {
            Some(s) => serde_json::from_str(s).unwrap_or_else(|_| value.clone()),
            None => value.clone(),
        })
        .unwrap_or(Value::Null)
}

fn metadata(attrs: &Map<String, Value>, prefix: &str) -> Map<String, Value> {
    let mut result = structured(attrs, &[prefix.trim_end_matches('.')])
        .as_object()
        .cloned()
        .unwrap_or_default();
    result.extend(
        attrs
            .iter()
            .filter_map(|(key, value)| Some((key.strip_prefix(prefix)?.to_owned(), value.clone()))),
    );
    result
}
