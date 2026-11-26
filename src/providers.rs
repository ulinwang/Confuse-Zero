use crate::{
    api::{Failure, read_body},
    model::{Observation, Record, Trace, now_ns, stable_id},
    store::Store,
};
use anyhow::{Context, Result, bail};
use axum::{
    body::Body,
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use reqwest::{Client, Url};
use serde_json::{Map, Value, json};
use std::{net::IpAddr, time::Duration};
use uuid::Uuid;

const SERVICE: &str = "io.github.ulinwang.confuse-zero";
const RESPONSE_CAP: usize = 1_048_576;

#[derive(Clone, Debug)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub format: String,
    base: Url,
    pub model: String,
    pub embedding_model: String,
    has_key: bool,
}

impl Provider {
    pub fn from_value(data: &Value) -> Result<Self> {
        let base = validate_endpoint(field(data, "baseUrl")?)?;
        let format = field(data, "format")?;
        if !matches!(format, "openai" | "anthropic") {
            bail!("Choose an OpenAI-compatible or Anthropic endpoint");
        }
        Ok(Self {
            id: field(data, "id")?.to_owned(),
            name: field(data, "name")?.to_owned(),
            format: format.to_owned(),
            base,
            model: field(data, "model")?.to_owned(),
            embedding_model: data["embeddingModel"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            has_key: data["hasKey"].as_bool().unwrap_or_default(),
        })
    }

    pub fn endpoint(&self, relative: &str) -> Result<Url> {
        Ok(self.base.join(relative.trim_start_matches('/'))?)
    }

    fn with_key(
        &self,
        project: &str,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder> {
        let key = if self.has_key {
            let data = security_framework::passwords::get_generic_password(
                SERVICE,
                &account(project, &self.id),
            )
            .context("Model API key could not be read from Keychain")?;
            Some(String::from_utf8(data).context("Invalid model API key")?)
        } else {
            None
        };
        let result = if self.format == "anthropic" {
            let builder = builder.header("anthropic-version", "2023-06-01");
            if let Some(key) = key {
                builder.header("x-api-key", key)
            } else {
                builder
            }
        } else if let Some(key) = key {
            builder.bearer_auth(key)
        } else {
            builder
        };
        Ok(result)
    }

    async fn send(&self, project: &str, route: &str, value: &Value) -> Result<Value> {
        let request = self.with_key(project, client()?.post(self.endpoint(route)?).json(value))?;
        let response = request.send().await?;
        if !response.status().is_success() {
            bail!("{} returned HTTP {}", self.name, response.status().as_u16());
        }
        bounded_json(response).await
    }

    pub async fn complete(
        &self,
        project: &str,
        model: Option<&str>,
        prompt: &str,
    ) -> Result<(String, Value)> {
        let model = model.filter(|s| !s.is_empty()).unwrap_or(&self.model);
        let messages = json!([{"role":"user","content":prompt}]);
        let value = if self.format == "anthropic" {
            self.send(
                project,
                "messages",
                &json!({"model":model,"messages":messages,"max_tokens":2048}),
            )
            .await?
        } else {
            self.send(
                project,
                "chat/completions",
                &json!({"model":model,"messages":messages}),
            )
            .await?
        };
        let output = if self.format == "anthropic" {
            value["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            value["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        };
        if output.trim().is_empty() {
            bail!("{} returned no text", self.name);
        }
        Ok((output, metrics(&value["usage"])))
    }

    pub async fn embed(&self, project: &str, text: &str, dimensions: usize) -> Result<Vec<f64>> {
        if self.format != "openai" || self.embedding_model.is_empty() {
            bail!("Choose a configured embeddings model");
        }
        let value = self
            .send(
                project,
                "embeddings",
                &json!({"model":self.embedding_model,"input":text,"dimensions":dimensions}),
            )
            .await?;
        let raw = value["data"][0]["embedding"]
            .as_array()
            .context("Embedding response has no vector")?;
        let vector = raw
            .iter()
            .map(|v| {
                v.as_f64()
                    .filter(|f| f.is_finite())
                    .context("Invalid embedding component")
            })
            .collect::<Result<Vec<_>>>()?;
        if vector.len() != dimensions || vector.iter().all(|value| *value == 0.) {
            bail!(
                "Embedding model returned {} dimensions, expected {dimensions}",
                vector.len()
            );
        }
        Ok(vector)
    }
}

impl Store {
    pub fn provider(&self, project: &str, id: &str) -> Result<Provider> {
        Provider::from_value(
            &self
                .resource(project, "llm-connections", id)?
                .context("Model connection is unavailable")?,
        )
    }

    pub fn configure_provider(&self, project: &str, mut data: Value) -> Result<Value> {
        let id = data["id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let previous = self.resource(project, "llm-connections", &id)?;
        let secret = data
            .as_object_mut()
            .context("Invalid connection")?
            .remove("apiKey");
        let key = secret
            .as_ref()
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        data["id"] = json!(id);
        data["hasKey"] =
            json!(key.is_some() || previous.as_ref().is_some_and(|obj| obj["hasKey"] == true));
        Provider::from_value(&data)?;
        if let Some(key) = key {
            security_framework::passwords::set_generic_password(
                SERVICE,
                &account(project, &id),
                key.as_bytes(),
            )
            .context("Could not store model API key in Keychain")?;
        }
        self.put_resource(project, "llm-connections", data)
    }

    pub fn remove_provider(&self, project: &str, id: &str) -> Result<bool> {
        let provider = self.resource(project, "llm-connections", id)?;
        if let Some(data) = &provider
            && data["hasKey"] == true
        {
            let id = field(data, "id")?;
            delete_key(project, id)?;
        }
        self.delete_resource(project, "llm-connections", id)
    }

    pub fn remove_provider_keys(&self) -> Result<()> {
        for project in self.projects()? {
            for item in self.resources(&project.id, "llm-connections")? {
                if item["hasKey"] == true {
                    delete_key(&project.id, field(&item, "id")?)?;
                }
            }
        }
        Ok(())
    }
}

fn delete_key(project: &str, id: &str) -> Result<()> {
    match security_framework::passwords::delete_generic_password(SERVICE, &account(project, id)) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()),
        Err(error) => Err(error).context("Could not remove model API key from Keychain"),
    }
}

fn account(project: &str, id: &str) -> String {
    format!("{project}:{id}")
}

fn validate_endpoint(value: &str) -> Result<Url> {
    let mut url = Url::parse(value).context("Enter a model endpoint URL")?;
    let host = url.host_str().context("Model endpoint requires a host")?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Use an HTTPS endpoint or an HTTP loopback endpoint");
    }
    if !url.path().ends_with('/') {
        let p = format!("{}/", url.path());
        url.set_path(&p);
    }
    Ok(url)
}

fn field<'a>(data: &'a Value, key: &str) -> Result<&'a str> {
    data[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("{key} is required"))
}

fn client() -> Result<Client> {
    static CLIENT: std::sync::LazyLock<Result<Client, reqwest::Error>> =
        std::sync::LazyLock::new(|| {
            Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .timeout(Duration::from_secs(120))
                .connect_timeout(Duration::from_secs(10))
                .build()
        });
    CLIENT
        .as_ref()
        .cloned()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

async fn bounded_json(mut response: reqwest::Response) -> Result<Value> {
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if data.len() + chunk.len() > RESPONSE_CAP {
            bail!("Model response is too large");
        }
        data.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&data)?)
}

pub fn record_completion(store: &Store, project: &str, record: Completion) -> Result<String> {
    let id = stable_id(&[&Uuid::new_v4().to_string()], 16);
    let span = stable_id(&[&id, "completion"], 8);
    let end = now_ns();
    let trace = Trace {
        id: id.clone(),
        name: record.name.clone(),
        source: record.source.to_owned(),
        environment: "default".to_owned(),
        input: record.input.clone(),
        output: record.output.clone(),
        start_ns: record.start,
        end_ns: end,
        received_ns: end,
        ..Default::default()
    };
    let obs = Observation {
        id: span,
        trace_id: id.clone(),
        kind: if record.name == "embeddings" {
            "EMBEDDING"
        } else {
            "GENERATION"
        }
        .to_owned(),
        name: record.name,
        model: record.model,
        level: if record.error.is_some() {
            "ERROR"
        } else {
            "DEFAULT"
        }
        .to_owned(),
        status_message: record.error,
        environment: "default".to_owned(),
        input: record.input,
        output: record.output,
        usage: record.usage,
        prompt_name: record.prompt_name,
        prompt_version: record.prompt_version,
        start_ns: record.start,
        end_ns: end,
        ..Default::default()
    };
    store.ingest(
        project,
        &[Record {
            trace,
            observation: Some(obs),
            root: true,
        }],
    )?;
    Ok(id)
}

#[derive(Default)]
pub struct Completion {
    pub source: &'static str,
    pub name: String,
    pub model: Option<String>,
    pub input: Value,
    pub output: Value,
    pub usage: Value,
    pub error: Option<String>,
    pub start: i64,
    pub prompt_name: Option<String>,
    pub prompt_version: Option<i64>,
}

pub(crate) async fn gateway(
    store: &Store,
    method: Method,
    format: &str,
    route: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, Failure> {
    let accepted = match format {
        "openai" => matches!(
            route,
            "responses"
                | "responses/compact"
                | "chat/completions"
                | "decisions"
                | "embeddings"
                | "models"
        ),
        "anthropic" => matches!(route, "messages" | "messages/count_tokens" | "models"),
        _ => false,
    };
    if !accepted
        || (route == "models" && method != Method::GET)
        || (route != "models" && method != Method::POST)
    {
        return Err(Failure::missing());
    }
    let key = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()))
        .ok_or_else(Failure::unauthorized)?;
    let project = store
        .project_for_gateway_key(key)?
        .ok_or_else(Failure::unauthorized)?;
    let providers = store.resources(&project, "llm-connections")?;
    let bytes = read_body(body, headers).await?;
    let mut payload: Value = if method == Method::POST {
        serde_json::from_slice(&bytes).map_err(|_| Failure::bad("Expected JSON request"))?
    } else {
        Value::Null
    };
    let model = payload["model"].as_str().map(str::to_owned);
    let provider = providers
        .iter()
        .filter(|item| item["format"] == format)
        .find(|item| {
            model
                .as_ref()
                .is_some_and(|model| item["model"] == *model || item["embeddingModel"] == *model)
        })
        .or_else(|| providers.iter().find(|item| item["format"] == format))
        .ok_or_else(|| Failure::bad("Configure a model connection for this gateway route"))?;
    let provider = Provider::from_value(provider)?;
    let url = provider.endpoint(route)?;
    let mut request = client()?.request(method.clone(), url);
    if method == Method::POST {
        if route == "chat/completions" && payload["stream"] == true {
            if !payload["stream_options"].is_object() {
                payload["stream_options"] = json!({});
            }
            payload["stream_options"]["include_usage"] = json!(true);
        }
        request = request.json(&payload);
    }
    let start = now_ns();
    let reply = provider
        .with_key(&project, request)?
        .send()
        .await
        .map_err(|_| Failure::bad("Model provider could not be reached"))?;
    let status = reply.status();
    let content_type = reply
        .headers()
        .get(header::CONTENT_TYPE)
        .cloned()
        .unwrap_or(header::HeaderValue::from_static("application/json"));
    let shape = content_type.to_str().unwrap_or_default().to_owned();
    let capture = !matches!(route, "models" | "messages/count_tokens");
    let start_record = Completion {
        source: "Gateway",
        name: route.to_owned(),
        model,
        input: payload,
        start,
        error: (!status.is_success()).then(|| format!("Provider HTTP {}", status.as_u16())),
        ..Default::default()
    };
    let project_id = project.clone();
    let store = store.clone();
    let mut upstream = reply.bytes_stream();
    let stream = async_stream::stream! {
        let mut collected=Vec::new();
        let mut broken=false;
        while let Some(next)=upstream.next().await {
            match next {
                Ok(bytes)=>{
                    if capture && collected.len()<RESPONSE_CAP {
                        let keep=(RESPONSE_CAP-collected.len()).min(bytes.len());
                        collected.extend_from_slice(&bytes[..keep]);
                    }
                    yield Ok::<_,std::io::Error>(bytes);
                }
                Err(error)=>{broken=true;yield Err(std::io::Error::other(error));break;}
            }
        }
        if capture {
            let (output,usage)=if shape.starts_with("text/event-stream") {
                capture_sse(&collected)
            }else{
                let data:Value=serde_json::from_slice(&collected).unwrap_or_default();
                let use_metrics=metrics(&data["usage"]);
                (data,use_metrics)
            };
            let error=if broken{Some("Provider response interrupted".to_owned())}else{start_record.error};
            let _=record_completion(&store,&project_id,Completion{output,usage,error,..start_record});
        }
    };
    Ok((
        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        [(header::CONTENT_TYPE, content_type)],
        Body::from_stream(stream),
    )
        .into_response())
}

fn capture_sse(bytes: &[u8]) -> (Value, Value) {
    let data = String::from_utf8_lossy(bytes);
    let mut text = String::new();
    let mut usage = Map::new();
    for line in data.lines().filter_map(|line| line.strip_prefix("data:")) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(delta) = event["choices"][0]["delta"]["content"]
            .as_str()
            .or_else(|| event["delta"]["text"].as_str())
            .or_else(|| event["delta"].as_str())
        {
            text.push_str(delta);
        }
        for item in [
            &event["usage"],
            &event["message"]["usage"],
            &event["response"]["usage"],
        ] {
            if let Value::Object(value) = metrics(item) {
                usage.extend(value);
            }
        }
    }
    (json!(text), Value::Object(usage))
}

fn metrics(data: &Value) -> Value {
    let mut result = Map::new();
    for (name, keys) in [
        ("input", &["prompt_tokens", "input_tokens"][..]),
        ("output", &["completion_tokens", "output_tokens"][..]),
        ("total", &["total_tokens"][..]),
        ("input_cached", &["cache_read_input_tokens"][..]),
        ("input_cache_write", &["cache_creation_input_tokens"][..]),
    ] {
        if let Some(number) = keys.iter().find_map(|name| data[*name].as_i64()) {
            result.insert(name.to_owned(), json!(number));
        }
    }
    Value::Object(result)
}
