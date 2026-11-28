use super::fail;
use crate::{
    desktop::required,
    model::now_ns,
    providers::{Completion, Provider, bounded_json, metrics, record_completion},
    resources::string_list,
    store::Store,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const MAX_SSE_BYTES: usize = 2_097_152;

pub(crate) async fn connection(
    store: &Store,
    project: &str,
    name: &str,
    input: &Value,
) -> Result<Value> {
    match name {
        "llmApiKey.effective" | "llmApiKey.all" => Ok(
            json!({ "data": store.resources(project, "llm-connections")?.into_iter().map(wire).collect::<Vec<_>>() }),
        ),
        "llmApiKey.inherited" => Ok(json!({ "data": [] })),
        "llmApiKey.create" | "llmApiKey.update" => {
            let (mut resource, id) = provider_resource(store, project, input)?;
            if name == "llmApiKey.update" {
                let id = id.context("Connection ID is required")?;
                resource["id"] = json!(id);
            }
            store.configure_provider(project, resource)?;
            Ok(Value::Null)
        }
        "llmApiKey.delete" => {
            if !store.remove_provider(project, required(input, "id")?)? {
                return Err(fail("NOT_FOUND", "Connection not found"));
            }
            Ok(json!({ "success": true }))
        }
        _ => Err(fail(
            "NOT_IMPLEMENTED",
            format!("Workspace procedure is unavailable: {name}"),
        )),
    }
}

fn provider_resource<'a>(
    store: &Store,
    project: &str,
    input: &'a Value,
) -> Result<(Value, Option<&'a str>)> {
    let adapter = required(input, "adapter")?;
    let (endpoint, model) = match adapter {
        "openai" => ("https://api.openai.com/v1", "gpt-5-mini"),
        "anthropic" => ("https://api.anthropic.com/v1", "claude-sonnet-4-6"),
        _ => bail!("Choose an OpenAI-compatible or Anthropic connection"),
    };
    let id = input["id"].as_str().filter(|id| !id.is_empty());
    let original = id
        .map(|id| store.resource(project, "llm-connections", id))
        .transpose()?
        .flatten();
    if id.is_some() && original.is_none() {
        return Err(fail("NOT_FOUND", "Connection not found"));
    }
    let provider = required(input, "provider")?.trim();
    if input["extraHeaders"]
        .as_object()
        .is_some_and(|headers| !headers.is_empty())
    {
        bail!("Additional provider headers are unavailable in this workspace");
    }
    if provider.is_empty() || provider.contains(':') {
        bail!("Invalid provider name");
    }
    if store
        .resources(project, "llm-connections")?
        .iter()
        .any(|row| row["name"] == provider && row["id"].as_str() != id)
    {
        return Err(fail("CONFLICT", "Connection name already exists"));
    }
    let custom_models = string_list(&input["customModels"]);
    let primary = custom_models
        .first()
        .map(String::as_str)
        .or_else(|| original.as_ref()?.get("model")?.as_str())
        .unwrap_or(model);
    Ok((
        json!({
            "name": provider, "format": adapter, "baseUrl": input["baseURL"].as_str().unwrap_or(endpoint),
            "model": primary, "customModels": custom_models, "apiKey": input["secretKey"],
            "withDefaultModels": input["withDefaultModels"],
            "embeddingModel": original.as_ref().and_then(|row| row["embeddingModel"].as_str()),
        }),
        id,
    ))
}

fn wire(row: Value) -> Value {
    let mut models = string_list(&row["customModels"]);
    for field in ["model", "embeddingModel"] {
        if let Some(model) = row[field].as_str().filter(|value| !value.is_empty())
            && !models.iter().any(|existing| existing == model)
        {
            models.push(model.to_owned());
        }
    }
    json!({
        "id": row["id"], "projectId": row["projectId"], "provider": row["name"], "adapter": row["format"],
        "baseURL": row["baseUrl"], "customModels": models, "withDefaultModels": row["withDefaultModels"].as_bool().unwrap_or(false),
        "createdAt": row["createdAt"], "updatedAt": row["updatedAt"], "displaySecretKey": if row["hasKey"] == true { "••••" } else { "" },
        "extraHeaderKeys": row["extraHeaders"].as_object().into_iter().flatten().map(|(name,_)|name).collect::<Vec<_>>(),
        "scope": "project",
    })
}

pub(crate) async fn run(store: Store, id: u64, input: Value, outbound: mpsc::Sender<Value>) {
    let mut response_started = false;
    let result = stream(&store, id, &input, &outbound, &mut response_started).await;
    let terminal = match result {
        Ok(()) => json!({"id":id,"stream":"end"}),
        Err(error) if response_started => {
            json!({"id":id,"stream":"error","message":error.to_string()})
        }
        Err(error) => {
            let _ = outbound
                .send(
                    json!({"id":id,"stream":"start","status":400,"contentType":"application/json"}),
                )
                .await;
            let _ = outbound.send(json!({"id":id,"stream":"chunk","chunk":json!({"message":error.to_string()}).to_string()})).await;
            json!({"id":id,"stream":"end"})
        }
    };
    let _ = outbound.send(terminal).await;
}

async fn stream(
    store: &Store,
    id: u64,
    input: &Value,
    outbound: &mpsc::Sender<Value>,
    started: &mut bool,
) -> Result<()> {
    let project = required(input, "projectId")?;
    let params = &input["modelParams"];
    let provider_name = required(params, "provider")?;
    let model = required(params, "model")?;
    let selected = store
        .resources(project, "llm-connections")?
        .into_iter()
        .find(|row| row["name"] == provider_name || row["id"] == provider_name)
        .context("Choose a model connection in this project")?;
    let provider = Provider::from_value(&selected)?;
    let messages = input["messages"]
        .as_array()
        .filter(|items| !items.is_empty() && items.len() <= 500)
        .context("Provide 1 to 500 messages")?;
    let tools = input["tools"].as_array().filter(|tools| !tools.is_empty());
    let schema = input["structuredOutputSchema"].as_object();
    if tools.is_some() && schema.is_some() {
        bail!("Select tools or structured output");
    }
    let anthropic = provider.format == "anthropic";
    let upstream_stream = input["streaming"] == true && tools.is_none() && schema.is_none();
    let mut data = json!({"model":model, "stream": upstream_stream, "messages": []});
    let mut normalized = Vec::new();
    let mut system = Vec::new();
    for message in messages {
        let role = required(message, "role")?;
        if !matches!(role, "system" | "developer" | "user" | "assistant" | "tool") {
            bail!("Invalid message role");
        }
        let wire_role = if role == "developer" { "system" } else { role };
        if anthropic && wire_role == "system" {
            system.push(
                message["content"]
                    .as_str()
                    .context("System messages must contain text")?,
            );
            continue;
        }
        let mut wire = json!({"role":wire_role,"content":message["content"]});
        if let Some(calls) = message["toolCalls"].as_array() {
            wire["tool_calls"] = json!(calls.iter().map(|call| json!({
                "id": call["id"], "type":"function", "function":{"name":call["name"],"arguments":call["args"].to_string()},
            })).collect::<Vec<_>>());
        }
        if let Some(call_id) = message["toolCallId"].as_str() {
            wire["tool_call_id"] = json!(call_id);
        }
        normalized.push(wire);
    }
    data["messages"] = json!(normalized);
    for (camel, key) in [
        ("temperature", "temperature"),
        ("maxTokens", "max_tokens"),
        ("topP", "top_p"),
    ] {
        if let Some(value) = params[camel].as_f64() {
            data[key] = json!(value);
        }
    }
    if anthropic {
        if tools.is_some()
            || schema.is_some()
            || messages.iter().any(|message| message["role"] == "tool")
        {
            bail!("This Anthropic connection supports text Playground requests");
        }
        if data["max_tokens"].is_null() {
            data["max_tokens"] = json!(2048);
        }
        if !system.is_empty() {
            data["system"] = json!(system.join("\n"));
        }
    } else {
        if upstream_stream {
            data["stream_options"] = json!({"include_usage":true});
        }
        if let Some(tools) = tools {
            data["tools"] = json!(
                tools
                    .iter()
                    .map(|tool| {
                        if tool["type"] == "function" {
                            tool.clone()
                        } else {
                            json!({"type":"function","function":tool})
                        }
                    })
                    .collect::<Vec<_>>()
            );
        }
        if let Some(schema) = schema {
            data["response_format"] =
                json!({"type":"json_schema","json_schema":{"name":"playground","schema":schema}});
        }
    }
    let start = now_ns();
    let response = provider
        .send_request(
            project,
            if anthropic {
                "messages"
            } else {
                "chat/completions"
            },
            &data,
        )
        .await?;
    let mut text = String::new();
    let mut usage = json!({});
    let mut result = Value::Null;
    if upstream_stream {
        if !response
            .headers()
            .get("content-type")
            .and_then(|header| header.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"))
        {
            bail!("The model did not return an event stream");
        }
        send_start(outbound, id, "text/plain; charset=utf-8").await?;
        *started = true;
        read_sse(response, outbound, id, &mut text, &mut usage).await?;
    } else {
        let completion = bounded_json(response).await?;
        let message = &completion["choices"][0]["message"];
        if anthropic {
            text = completion["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| item["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
        } else {
            text = message["content"].as_str().unwrap_or_default().to_owned();
        }
        usage = metrics(&completion["usage"]);
        if tools.is_some() {
            let calls: Vec<_> = message["tool_calls"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|call| {
                    let arguments: Value =
                        serde_json::from_str(call["function"]["arguments"].as_str()?).ok()?;
                    Some(json!({"id":call["id"],"name":call["function"]["name"],"args":arguments}))
                })
                .collect();
            result =
                json!({"content":text,"tool_calls":calls,"reasoning":message["reasoning_content"]});
        } else if schema.is_none() {
            result = json!({"content":text,"reasoning":message["reasoning_content"]});
        }
    }
    let saved_prompt = input["sourcePrompt"]["name"]
        .as_str()
        .zip(input["sourcePrompt"]["version"].as_i64())
        .map(|(name, version)| store.versioned(project, "prompts", name, Some(version), None))
        .transpose()?
        .flatten();
    record_completion(
        store,
        project,
        Completion {
            source: "Playground",
            name: "Playground".to_owned(),
            model: Some(model.to_owned()),
            input: input["messages"].clone(),
            output: json!(text),
            usage,
            start,
            prompt_name: saved_prompt
                .as_ref()
                .and_then(|row| row["name"].as_str().map(str::to_owned)),
            prompt_version: saved_prompt
                .as_ref()
                .and_then(|row| row["version"].as_i64()),
            ..Default::default()
        },
    )?;
    if !upstream_stream {
        let (content_type, output) = if result.is_null() {
            ("text/plain; charset=utf-8", text)
        } else {
            ("application/json", result.to_string())
        };
        send_start(outbound, id, content_type).await?;
        *started = true;
        outbound
            .send(json!({"id":id,"stream":"chunk","chunk":output}))
            .await?;
    }
    Ok(())
}

async fn send_start(outbound: &mpsc::Sender<Value>, id: u64, content_type: &str) -> Result<()> {
    outbound
        .send(json!({"id":id,"stream":"start","status":200,"contentType":content_type}))
        .await?;
    Ok(())
}

async fn read_sse(
    mut response: reqwest::Response,
    outbound: &mpsc::Sender<Value>,
    id: u64,
    text: &mut String,
    usage: &mut Value,
) -> Result<()> {
    let mut received = 0;
    let mut buffer = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        received += chunk.len();
        if received > MAX_SSE_BYTES {
            bail!("Model response is too large");
        }
        buffer.extend_from_slice(&chunk);
        let Some(last) = buffer.iter().rposition(|byte| *byte == b'\n') else {
            continue;
        };
        let tail = buffer.split_off(last + 1);
        for line in buffer.split(|byte| *byte == b'\n') {
            let Some(data) = line.strip_prefix(b"data:") else {
                continue;
            };
            let data = std::str::from_utf8(data)?.trim();
            if data == "[DONE]" {
                return Ok(());
            }
            let event: Value = serde_json::from_str(data)?;
            if let Some(error) = event["error"]["message"].as_str() {
                bail!("Model stream: {error}");
            }
            let mapped = metrics(event.get("usage").unwrap_or(&event["message"]["usage"]));
            if let Some(next) = mapped.as_object()
                && let Some(current) = usage.as_object_mut()
            {
                current.extend(next.clone());
            }
            if let Some(token) = event["choices"][0]["delta"]["content"]
                .as_str()
                .or_else(|| event["delta"]["text"].as_str())
            {
                text.push_str(token);
                outbound
                    .send(json!({"id":id,"stream":"chunk","chunk":token}))
                    .await?;
            }
        }
        buffer = tail;
    }
    if !buffer.is_empty() {
        bail!("Model stream finished mid-event");
    }
    Ok(())
}
