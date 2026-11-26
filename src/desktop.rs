use crate::{
    agents::{Agent, Collector, Discovery, Selection},
    api::Server,
    model::{TraceFilter, cost_total, usage_total},
    store::{DEFAULT_PROJECT, Store},
};
use anyhow::{Context, Result, bail};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, env, path::PathBuf, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

#[derive(Deserialize)]
struct Request {
    id: u64,
    channel: String,
    #[serde(default)]
    payload: Value,
}

pub async fn run(store: Store, home: PathBuf) -> Result<()> {
    let mut service = Desktop::open(store.clone(), home).await?;
    let topic_task = tokio::spawn(crate::topics::worker(store.clone()));
    let workspace_task = tokio::spawn(crate::workspace::worker(store.clone()));
    let collector_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let store = store.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let collector = Collector::new(store.clone());
                if let Ok(projects) = store.projects() {
                    for project in projects {
                        let _ = collector.update(&project.id);
                    }
                }
            })
            .await;
        }
    });

    let mut requests = BufReader::new(tokio::io::stdin()).lines();
    let mut output = tokio::io::stdout();
    send(&mut output, json!({"ready":true})).await?;
    while let Some(line) = requests.next_line().await? {
        if line.len() > MAX_REQUEST_BYTES {
            break;
        }
        let Ok(request) = serde_json::from_str::<Request>(&line) else {
            continue;
        };
        if request.channel == "desktop.quit" {
            collector_task.abort();
            topic_task.abort();
            workspace_task.abort();
            if let Some(server) = service.server.take() {
                server.stop().await;
            }
            send(&mut output, json!({"id":request.id,"result":true})).await?;
            return Ok(());
        }
        let value = match service.handle(&request.channel, &request.payload).await {
            Ok(result) => json!({"id":request.id,"result":result}),
            Err(error) => json!({"id":request.id,"error":error.to_string()}),
        };
        send(&mut output, value).await?;
    }
    collector_task.abort();
    topic_task.abort();
    workspace_task.abort();
    if let Some(server) = service.server.take() {
        server.stop().await;
    }
    Ok(())
}

async fn send(output: &mut tokio::io::Stdout, value: Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    output.write_all(&bytes).await?;
    output.flush().await?;
    Ok(())
}

struct Desktop {
    store: Store,
    discovery: Discovery,
    server: Option<Server>,
    port: u16,
    listener_error: Option<String>,
}

impl Desktop {
    async fn open(store: Store, home: PathBuf) -> Result<Self> {
        let port = env::var("CONFUSE_PORT")
            .ok()
            .map(|v| v.parse::<u16>())
            .transpose()?
            .unwrap_or(store.endpoint_port()?);
        let (server, listener_error) = match Server::start(store.clone(), port).await {
            Ok(server) => (Some(server), None),
            Err(error) => (None, Some(format!("SDK port {port}: {error}"))),
        };
        Ok(Self {
            store,
            discovery: Discovery::from_environment(home),
            server,
            port,
            listener_error,
        })
    }

    async fn handle(&mut self, channel: &str, payload: &Value) -> Result<Value> {
        let project = payload
            .get("project")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_PROJECT);
        if !self.store.projects()?.iter().any(|item| item.id == project) {
            bail!("Project not found");
        }
        let collector = Collector::new(self.store.clone());
        let result = match channel {
            "desktop.get-state" => {
                let setup = self.store.access(|conn| {
                    Ok(conn
                        .query_row(
                            "SELECT value FROM settings WHERE key='onboarded'",
                            [],
                            |row| row.get::<_, bool>(0),
                        )
                        .optional()?
                        .unwrap_or(false))
                })?;
                json!({
                    "onboarded":setup, "projects":self.store.projects()?, "port":self.port,
                    "sdkEndpoint":self.server.as_ref().map(|s|format!("http://{}",s.address)),
                    "listenerError":self.listener_error, "version":env!("CARGO_PKG_VERSION")
                })
            }
            "desktop.complete-onboarding" | "agents.connect" => {
                let selections: Vec<Selection> = serde_json::from_value(
                    payload.get("selections").cloned().unwrap_or(json!([])),
                )?;
                if selections.len() > Agent::ALL.len() {
                    bail!("Too many Agents selected");
                }
                collector.select(project, &selections)?;
                if channel == "desktop.complete-onboarding" {
                    self.store.access(|conn| {
                        conn.execute(
                            "INSERT OR REPLACE INTO settings(key,value) VALUES('onboarded',1)",
                            [],
                        )?;
                        Ok(())
                    })?;
                }
                json!(true)
            }
            "agents.scan" => {
                json!({"installations":self.discovery.scan(),"sources":collector.sources(project)?})
            }
            "agents.disable" => {
                collector.disable(project, serde_json::from_value(payload["agent"].clone())?)?;
                json!(true)
            }
            "agents.retry" => {
                let agent: Agent = serde_json::from_value(payload["agent"].clone())?;
                let selection = collector
                    .sources(project)?
                    .into_iter()
                    .find(|s| s.selection.agent == agent && s.active)
                    .map(|source| source.selection)
                    .context("Select a session directory first")?;
                collector.select(project, &[selection])?;
                json!(true)
            }
            "notifications.list" => json!(self.store.notifications(project)?),
            "notifications.mark-read" => {
                self.store.mark_notifications_read(project)?;
                json!(true)
            }
            "projects.create" => json!(self.store.create_project(required(payload, "name")?)?),
            "keys.create" => json!(self.store.create_key(
                project,
                if payload["kind"] == "gateway" {
                    "gateway"
                } else {
                    "sdk"
                }
            )?),
            "keys.list" => json!(self.store.keys(project)?),
            "keys.revoke" => json!(self.store.revoke_key(project, required(payload, "id")?)?),
            "desktop.set-port" => {
                let port = u16::try_from(payload["port"].as_u64().context("Enter a port number")?)?;
                if port == 0 {
                    bail!("Enter a port between 1 and 65535");
                }
                if self.server.is_none() || self.port != port {
                    let server = Server::start(self.store.clone(), port)
                        .await
                        .context("SDK listener could not bind this port")?;
                    self.store.set_endpoint_port(port)?;
                    if let Some(previous) = self.server.replace(server) {
                        previous.stop().await;
                    }
                }
                self.port = port;
                self.listener_error = None;
                json!(format!("http://127.0.0.1:{port}"))
            }
            "desktop.cleanup" => {
                self.store.remove_provider_keys()?;
                json!(true)
            }
            "dashboard.summary" => self.summary(project)?,
            "traces.list" => {
                let filter = TraceFilter {
                    query: payload["query"].as_str().filter(|s| !s.is_empty()),
                    source: payload["source"].as_str().filter(|s| !s.is_empty()),
                    session: payload["session"].as_str().filter(|s| !s.is_empty()),
                    topic: payload["topic"].as_str(),
                    ..Default::default()
                };
                let (traces, total) = self.store.traces(project, &filter, 100, offset(payload))?;
                json!({"data":traces.iter().map(|trace|trace.public()).collect::<Vec<_>>(),"total":total})
            }
            "traces.get" => {
                let id = required(payload, "id")?;
                let trace = self.store.trace(project, id)?.context("Trace not found")?;
                let observations = self
                    .store
                    .observations(project, Some(id), None, 10000, 0)?
                    .0;
                let scores = self.store.scores(project, Some(id), None, 1000, 0)?.0;
                json!({"trace":trace.public(),"observations":observations.iter().map(|span|span.public(project)).collect::<Vec<_>>(),"scores":scores})
            }
            "traces.delete" => json!(self.store.delete_trace(project, required(payload, "id")?)?),
            "sessions.list" => {
                let (data, total) = self.store.sessions(project, 100, offset(payload))?;
                json!({"data":data,"total":total})
            }
            "scores.list" => {
                let (data, total) = self
                    .store
                    .scores(project, None, None, 100, offset(payload))?;
                json!({"data":data,"total":total})
            }
            "scores.put" => self.store.put_score(project, payload["score"].clone())?,
            _ => crate::workspace::handle(&self.store, project, channel, payload)
                .await?
                .context("Unsupported desktop action")?,
        };
        Ok(result)
    }

    fn summary(&self, project: &str) -> Result<Value> {
        let (traces, total) = self.store.traces(project, &TraceFilter::default(), 10, 0)?;
        let (_, sessions) = self.store.sessions(project, 0, 0)?;
        let mut tokens = 0_i64;
        let mut cost = 0_f64;
        let mut errors = 0;
        let mut source_counts = BTreeMap::new();
        let mut offset = 0;
        loop {
            let (items, total) = self.store.observations(project, None, None, 1000, offset)?;
            for item in &items {
                tokens = tokens.saturating_add(usage_total(&item.usage).unwrap_or_default());
                cost += cost_total(&item.cost).unwrap_or_default();
                errors += usize::from(item.level == "ERROR");
                if let Some(agent) = item.metadata["agent"].as_str() {
                    *source_counts.entry(agent.to_owned()).or_insert(0_i64) += 1;
                }
            }
            offset += items.len();
            if items.is_empty() || offset >= total {
                break;
            }
        }
        Ok(
            json!({"traces":total,"sessions":sessions,"tokens":tokens,"cost":cost,"errors":errors,
            "recent":traces.iter().map(|trace|trace.public()).collect::<Vec<_>>(),"sources":source_counts}),
        )
    }
}

pub(crate) fn required<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .with_context(|| format!("{field} is required"))
}
fn offset(value: &Value) -> usize {
    value["offset"]
        .as_u64()
        .unwrap_or_default()
        .min(i64::MAX as u64) as usize
}
