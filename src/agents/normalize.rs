use super::Agent;
use crate::model::{Observation, Record, Trace, parse_time, stable_id};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Session {
    pub id: String,
    turn: String,
    model: Option<String>,
    inputs: BTreeMap<String, (Value, i64)>,
    entry_turns: BTreeMap<String, String>,
    kimi_step: u64,
    kimi_output: String,
    kimi_usage: Value,
    codex_usage_total: Value,
}

#[derive(Default)]
struct Event {
    role: &'static str,
    id: Option<String>,
    parent: Option<String>,
    turn: Option<String>,
    label: Option<String>,
    model: Option<String>,
    content: Value,
    usage: Value,
    cost: Value,
    metadata: Value,
    error: Option<String>,
}

impl Session {
    pub fn learn_header(&mut self, line: &Value) {
        match line.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if let Some(id) = string(&line["payload"], "id") {
                    self.id = id;
                }
            }
            Some("session" | "session_header") => {
                if let Some(id) = string(line, "id").or_else(|| string(line, "sessionId")) {
                    self.id = id;
                }
            }
            _ => {}
        }
        if let Some(id) = string(line, "sessionId") {
            self.id = id;
        }
    }

    pub fn normalize(
        &mut self,
        agent: Agent,
        line: &Value,
        index: u64,
        fallback_id: &str,
        received: i64,
    ) -> Option<Record> {
        self.learn_header(line);
        let time = line
            .get("timestamp")
            .or_else(|| line.get("time"))
            .or_else(|| line["message"].get("timestamp"))
            .and_then(parse_time)
            .unwrap_or(received);
        let event = match agent {
            Agent::Pi | Agent::ClaudeCode => self.message(agent, line)?,
            Agent::Codex => self.codex(line)?,
            Agent::KimiCode => self.kimi(line)?,
            Agent::DeepSeekHarness => self.dsh(line)?,
        };
        let id = event.id.unwrap_or_else(|| index.to_string());
        let kind = match event.role {
            "user" => "EVENT",
            "assistant" => "GENERATION",
            "tool" => "TOOL",
            "usage" => "GENERATION",
            _ => return None,
        };
        let turn = event
            .turn
            .or_else(|| {
                if event.role == "user" {
                    None
                } else {
                    event
                        .parent
                        .as_ref()
                        .and_then(|parent| self.entry_turns.get(parent))
                        .cloned()
                }
            })
            .unwrap_or_else(|| {
                if event.role == "user" || self.turn.is_empty() {
                    id.clone()
                } else {
                    self.turn.clone()
                }
            });
        self.turn = turn.clone();
        self.entry_turns.insert(id.clone(), turn.clone());
        if event.role == "user" {
            self.inputs
                .insert(turn.clone(), (event.content.clone(), time));
        }
        let (input, start) = self
            .inputs
            .get(&turn)
            .cloned()
            .unwrap_or((Value::Null, time));
        let session = if self.id.is_empty() {
            fallback_id
        } else {
            &self.id
        };
        let session_id = format!("{}:{session}", agent.id());
        let trace_id = stable_id(&[agent.id(), session, &turn], 16);
        let observation_id = stable_id(&[agent.id(), session, &id], 8);
        let parent = event
            .parent
            .map(|value| stable_id(&[agent.id(), session, &value], 8));
        let label = event.label.unwrap_or_else(|| match event.role {
            "user" => "User message".to_owned(),
            "tool" => "Tool".to_owned(),
            "usage" => "Token usage".to_owned(),
            _ => "Model response".to_owned(),
        });
        let output = if matches!(event.role, "user" | "usage") {
            Value::Null
        } else {
            event.content
        };
        let trace = Trace {
            id: trace_id.clone(),
            name: agent.label().to_owned(),
            source: agent.label().to_owned(),
            session_id: Some(session_id),
            environment: "default".to_owned(),
            input: input.clone(),
            output: if event.role == "assistant" {
                output.clone()
            } else {
                Value::Null
            },
            metadata: json!({"agent":agent.id()}),
            tags: vec![agent.id().to_owned()],
            start_ns: start,
            end_ns: time.max(start),
            received_ns: received,
            ..Default::default()
        };
        let observation = Observation {
            id: observation_id,
            trace_id,
            parent_id: parent,
            kind: kind.to_owned(),
            name: label,
            model: event.model.or_else(|| self.model.clone()),
            level: if event.error.is_some() {
                "ERROR"
            } else {
                "DEFAULT"
            }
            .to_owned(),
            status_message: event.error,
            environment: "default".to_owned(),
            input: if event.role == "assistant" || event.role == "user" {
                input
            } else {
                Value::Null
            },
            output,
            metadata: event.metadata,
            usage: event.usage,
            cost: event.cost,
            start_ns: if event.role == "assistant" {
                start
            } else {
                time
            },
            end_ns: time.max(start),
            ..Default::default()
        };
        Some(Record {
            trace,
            observation: Some(observation),
            root: matches!(event.role, "user" | "assistant"),
        })
    }

    fn message(&mut self, agent: Agent, line: &Value) -> Option<Event> {
        let message = &line["message"];
        let record_type = line["type"].as_str().unwrap_or_default();
        if record_type == "model_change" {
            self.model = string(line, "modelId");
            return None;
        }
        let role = match message["role"].as_str().unwrap_or(record_type) {
            "user" => {
                if has_tool_result(&message["content"]) {
                    "tool"
                } else {
                    "user"
                }
            }
            "assistant" => "assistant",
            "toolResult" | "tool" | "tool_result" => "tool",
            _ => return None,
        };
        let token_usage = &message["usage"];
        let identifier = if agent == Agent::ClaudeCode && role == "assistant" {
            string(message, "id").or_else(|| string(line, "uuid"))
        } else {
            string(line, "id").or_else(|| string(line, "uuid"))
        };
        let parent = string(line, "parentId").or_else(|| string(line, "parentUuid"));
        let tool_call = string(message, "toolCallId").or_else(|| {
            message["content"]
                .as_array()?
                .iter()
                .find_map(|part| string(part, "tool_use_id"))
        });
        if agent == Agent::ClaudeCode
            && let (Some(uuid), Some(preferred)) = (string(line, "uuid"), identifier.as_ref())
            && uuid != *preferred
        {
            let turn = parent
                .as_ref()
                .and_then(|p| self.entry_turns.get(p))
                .unwrap_or(&self.turn)
                .clone();
            self.entry_turns.insert(uuid, turn);
        }
        Some(Event {
            role,
            id: identifier,
            parent,
            label: string(message, "toolName"),
            model: string(message, "model"),
            content: visible_content(&message["content"]),
            usage: usage(token_usage),
            cost: token_usage.get("cost").cloned().unwrap_or_default(),
            metadata: json!({"toolCallId":tool_call,"entryId":line.get("id").or_else(||line.get("uuid")),"provider":message.get("provider")}),
            error: message
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                .then(|| "Tool failed".to_owned()),
            ..Default::default()
        })
    }

    fn codex(&mut self, line: &Value) -> Option<Event> {
        let payload = &line["payload"];
        let kind = payload["type"].as_str().unwrap_or_default();
        match line["type"].as_str()? {
            "turn_context" => {
                self.model = string(payload, "model").or_else(|| self.model.clone());
                if let Some(turn) = string(payload, "turn_id") {
                    self.turn = turn;
                }
                None
            }
            "event_msg" => match kind {
                "task_started" | "turn_started" => {
                    if let Some(turn) = string(payload, "turn_id") {
                        self.turn = turn;
                    }
                    None
                }
                "token_count" => {
                    let info = &payload["info"];
                    if info.is_null() {
                        return None;
                    }
                    let total = usage(&info["total_token_usage"]);
                    let increment = if total.as_object().is_some_and(|u| !u.is_empty()) {
                        let previous =
                            std::mem::replace(&mut self.codex_usage_total, total.clone());
                        let mut increment = Map::new();
                        if let Some(entries) = total.as_object() {
                            for (name, value) in entries {
                                if let Some(value) = value.as_i64() {
                                    let old = previous[name].as_i64().unwrap_or(0);
                                    increment.insert(
                                        name.clone(),
                                        json!(if value >= old { value - old } else { value }),
                                    );
                                }
                            }
                        }
                        Value::Object(increment)
                    } else {
                        usage(&info["last_token_usage"])
                    };
                    Some(Event {
                        role: "usage",
                        usage: increment,
                        ..Default::default()
                    })
                }
                _ => None,
            },
            "response_item" => {
                let role = match kind {
                    "message" => match payload["role"].as_str()? {
                        "user" => "user",
                        "assistant" => "assistant",
                        _ => return None,
                    },
                    "function_call"
                    | "custom_tool_call"
                    | "function_call_output"
                    | "custom_tool_call_output"
                    | "web_search_call" => "tool",
                    _ => return None,
                };
                let content = visible_content(
                    payload
                        .get("content")
                        .or_else(|| payload.get("output"))
                        .or_else(|| payload.get("arguments"))
                        .unwrap_or(&Value::Null),
                );
                let id = string(payload, "id")
                    .or_else(|| string(payload, "call_id").map(|call| format!("{kind}:{call}")));
                let parent = kind
                    .ends_with("_output")
                    .then(|| {
                        string(payload, "call_id")
                            .map(|id| format!("{}:{id}", kind.trim_end_matches("_output")))
                    })
                    .flatten();
                Some(Event {
                    role,
                    id,
                    parent,
                    turn: (!self.turn.is_empty()).then(|| self.turn.clone()),
                    label: string(payload, "name"),
                    content,
                    metadata: json!({"toolCallId":payload.get("call_id")}),
                    ..Default::default()
                })
            }
            _ => None,
        }
    }

    fn kimi(&mut self, line: &Value) -> Option<Event> {
        let wire = line
            .get("message")
            .filter(|v| v.get("type").is_some())
            .unwrap_or(line);
        let payload = wire.get("payload").unwrap_or(wire);
        let kind = wire["type"].as_str()?;
        match kind {
            "TurnBegin" | "turn_begin" | "user" => {
                self.kimi_step = 0;
                self.kimi_output.clear();
                self.kimi_usage = Value::Null;
                Some(Event {
                    role: "user",
                    id: string(payload, "id"),
                    content: visible_content(
                        payload
                            .get("user_input")
                            .or_else(|| payload.get("content"))
                            .unwrap_or(&Value::Null),
                    ),
                    ..Default::default()
                })
            }
            "StepBegin" => {
                self.kimi_step += 1;
                self.kimi_output.clear();
                self.kimi_usage = Value::Null;
                None
            }
            "StatusUpdate" => {
                if let Some(tokens) = payload.get("token_usage").or_else(|| payload.get("usage")) {
                    self.kimi_usage = usage(tokens);
                }
                if let Some(model) = string(payload, "model") {
                    self.model = Some(model);
                }
                self.kimi_generation()
            }
            "ContentPart" | "TextPart" => {
                if payload["type"]
                    .as_str()
                    .is_some_and(|s| !matches!(s, "text" | "text-delta"))
                {
                    return None;
                }
                if let Some(delta) = payload
                    .get("text")
                    .or_else(|| payload.get("content"))
                    .and_then(Value::as_str)
                    && self.kimi_output.len() + delta.len() < 512_000
                {
                    self.kimi_output.push_str(delta);
                }
                self.kimi_generation()
            }
            "StepEnd" | "TurnEnd" => self.kimi_generation(),
            "ToolCall" | "ToolResult" => {
                let id = string(payload, "tool_call_id").or_else(|| string(payload, "id"));
                Some(Event {
                    role: "tool",
                    id: id.as_ref().map(|id| format!("{kind}:{id}")),
                    parent: if kind == "ToolResult" {
                        id.map(|id| format!("ToolCall:{id}"))
                    } else {
                        None
                    },
                    label: string(payload, "name").or_else(|| string(&payload["function"], "name")),
                    content: visible_content(
                        payload
                            .get("result")
                            .or_else(|| payload.get("return_value"))
                            .or_else(|| payload.get("function"))
                            .unwrap_or(payload),
                    ),
                    ..Default::default()
                })
            }
            "assistant" => Some(Event {
                role: "assistant",
                content: visible_content(&payload["content"]),
                model: string(payload, "model"),
                usage: usage(&payload["usage"]),
                ..Default::default()
            }),
            _ => None,
        }
    }

    fn kimi_generation(&self) -> Option<Event> {
        if self.kimi_output.is_empty() && self.kimi_usage.is_null() {
            return None;
        }
        Some(Event {
            role: "assistant",
            id: Some(format!("{}:step:{}", self.turn, self.kimi_step)),
            content: json!(self.kimi_output),
            usage: self.kimi_usage.clone(),
            ..Default::default()
        })
    }

    fn dsh(&mut self, line: &Value) -> Option<Event> {
        let kind = line["type"].as_str()?;
        let data = line
            .get("data")
            .or_else(|| line.get("payload"))
            .unwrap_or(line);
        let message = data.get("message").unwrap_or(data);
        if kind == "turn/start" {
            if let Some(turn) = identifier(data, "turn") {
                self.turn = turn;
            }
            return None;
        }
        let role = match kind {
            "user/message" if !line.get("surfaceOp").is_some_and(Value::is_object) => "user",
            "assistant/message" => "assistant",
            "tool/start" | "tool/end" | "tool/result" | "tool/message" => "tool",
            _ => return None,
        };
        Some(Event {
            role,
            id: identifier(line, "seq").or_else(|| string(message, "id")),
            turn: identifier(data, "turn")
                .or_else(|| (!self.turn.is_empty()).then(|| self.turn.clone())),
            model: string(&message["source"], "model").or_else(|| string(message, "model")),
            label: string(data, "toolName").or_else(|| string(data, "name")),
            content: visible_content(
                message
                    .get("content")
                    .or_else(|| data.get("result"))
                    .unwrap_or(&Value::Null),
            ),
            usage: usage(data.get("usage").unwrap_or(&message["usage"])),
            metadata: json!({"toolCallId":data.get("toolCallId")}),
            ..Default::default()
        })
    }
}

fn string(data: &Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn identifier(data: &Value, key: &str) -> Option<String> {
    string(data, key).or_else(|| data.get(key)?.as_u64().map(|n| n.to_string()))
}

fn has_tool_result(content: &Value) -> bool {
    content
        .as_array()
        .is_some_and(|parts| parts.iter().any(|part| part["type"] == "tool_result"))
}

fn visible_content(value: &Value) -> Value {
    match value {
        Value::Array(parts) => {
            let visible: Vec<_> = parts
                .iter()
                .filter(|part| {
                    !matches!(
                        part.get("type").and_then(Value::as_str),
                        Some(
                            "thinking"
                                | "redacted_thinking"
                                | "reasoning"
                                | "input_image"
                                | "image"
                        )
                    )
                })
                .cloned()
                .collect();
            if visible.is_empty() {
                Value::Null
            } else {
                Value::Array(visible)
            }
        }
        other => other.clone(),
    }
}

fn usage(value: &Value) -> Value {
    let mut result = Map::new();
    for (key, names) in [
        (
            "input",
            &[
                "input",
                "input_tokens",
                "inputTokens",
                "input_other",
                "prompt_tokens",
            ][..],
        ),
        (
            "output",
            &[
                "output",
                "output_tokens",
                "outputTokens",
                "completion_tokens",
            ][..],
        ),
        ("total", &["total", "total_tokens", "totalTokens"][..]),
        (
            "input_cached",
            &[
                "cacheRead",
                "cache_read_input_tokens",
                "cached_input_tokens",
                "cachedInputTokens",
                "input_cache_read",
            ][..],
        ),
        (
            "input_cache_write",
            &[
                "cacheWrite",
                "cache_creation_input_tokens",
                "cacheWriteTokens",
                "input_cache_creation",
            ][..],
        ),
        (
            "reasoning",
            &["reasoning_output_tokens", "reasoningOutputTokens"][..],
        ),
    ] {
        if let Some(amount) = names
            .iter()
            .find_map(|name| value.get(*name).and_then(Value::as_i64))
        {
            result.insert(key.to_owned(), json!(amount));
        }
    }
    Value::Object(result)
}
