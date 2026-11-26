use crate::{
    desktop::required,
    model::{TraceFilter, iso, now_ns, parse_time, stable_id},
    providers::{Completion, record_completion},
    resources::{KINDS, matches_filters, string_list},
    store::Store,
    topics,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
use uuid::Uuid;

pub(crate) async fn handle(
    store: &Store,
    project: &str,
    channel: &str,
    payload: &Value,
) -> Result<Option<Value>> {
    let result = match channel {
        "workspace.list" => {
            let kind = required(payload, "kind")?;
            if !KINDS.contains(&kind) {
                bail!("Unknown workspace resource");
            }
            let data = store
                .resources(project, kind)?
                .into_iter()
                .filter(|item| matches_filters(item, &payload["filters"]))
                .collect::<Vec<_>>();
            json!(data)
        }
        "workspace.save" => {
            let kind = required(payload, "kind")?;
            let data = payload["data"].clone();
            match kind {
                "llm-connections" => store.configure_provider(project, data)?,
                "exports" => configure_export(store, project, data)?,
                _ => store.put_resource(project, kind, data)?,
            }
        }
        "workspace.delete" => {
            let kind = required(payload, "kind")?;
            let id = required(payload, "id")?;
            json!(if kind == "llm-connections" {
                store.remove_provider(project, id)?
            } else {
                store.delete_resource(project, kind, id)?
            })
        }
        "workspace.labels" => store.set_version_labels(
            project,
            required(payload, "kind")?,
            required(payload, "name")?,
            payload["version"].as_i64().context("version required")?,
            &string_list(&payload["labels"]),
        )?,
        "workspace.metrics" => store.metrics(project, &payload["query"])?,
        "workspace.export" => write_export(store, project, &payload["data"])?,
        "workspace.playground" => playground(store, project, payload).await?,
        "workspace.run-experiment" => queue_experiment(store, project, payload["data"].clone())?,
        "workspace.evaluate" => evaluate_trace(store, project, payload).await?,
        "topics.estimate" => topics::estimate(store, project, &payload["map"], now_ns())?,
        "topics.list" => {
            json!({"maps":store.resources(project,"topic-maps")?,"facets":topics::facets()})
        }
        "topics.save" => topics::save(store, project, payload["map"].clone())?,
        "topics.change" => topics::change(
            store,
            project,
            required(payload, "id")?,
            required(payload, "action")?,
        )?,
        "topics.delete" => json!(topics::delete(store, project, required(payload, "id")?)?),
        "topics.result" => topics::result(
            store,
            project,
            required(payload, "id")?,
            payload["facet"].as_str(),
        )?,
        "topics.history" => json!(
            store
                .resources(project, "topic-runs")?
                .into_iter()
                .filter(|item| payload["id"].as_str().is_none_or(|id| item["mapId"] == id))
                .collect::<Vec<_>>()
        ),
        _ => return Ok(None),
    };
    Ok(Some(result))
}

async fn playground(store: &Store, project: &str, payload: &Value) -> Result<Value> {
    let provider = store.provider(project, required(payload, "provider")?)?;
    let mut prompt = payload["prompt"].as_str().unwrap_or_default().to_owned();
    let name = payload["promptName"]
        .as_str()
        .filter(|name| !name.is_empty());
    let mut version = None;
    if let Some(name) = name {
        let saved = store
            .versioned(
                project,
                "prompts",
                name,
                payload["promptVersion"].as_i64(),
                payload["promptVersion"]
                    .as_i64()
                    .is_none()
                    .then_some("latest"),
            )?
            .context("Prompt version unavailable")?;
        version = saved["version"].as_i64();
        prompt = saved["prompt"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| saved["prompt"].to_string());
    }
    if let Some(variables) = payload["variables"].as_object() {
        for (key, value) in variables {
            let replacement = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            prompt = prompt.replace(&format!("{{{{{key}}}}}"), &replacement);
        }
    }
    if prompt.trim().is_empty() {
        bail!("Enter a prompt");
    }
    let start = now_ns();
    let model = payload["model"]
        .as_str()
        .unwrap_or(&provider.model)
        .to_owned();
    let (output, usage) = provider.complete(project, Some(&model), &prompt).await?;
    let trace = record_completion(
        store,
        project,
        Completion {
            source: "Playground",
            name: "Playground".to_owned(),
            model: Some(model),
            input: json!(prompt),
            output: json!(output),
            usage: usage.clone(),
            start,
            prompt_name: name.map(str::to_owned),
            prompt_version: version,
            ..Default::default()
        },
    )?;
    Ok(json!({"output":output,"traceId":trace,"usage":usage}))
}

fn queue_experiment(store: &Store, project: &str, mut data: Value) -> Result<Value> {
    let _ = required(&data, "name")?;
    let _ = store
        .resource(project, "datasets", required(&data, "datasetId")?)?
        .context("Choose a dataset")?;
    let _ = store.provider(project, required(&data, "provider")?)?;
    data["status"] = json!("Queued");
    store.put_resource(project, "experiments", data)
}

async fn experiment(store: &Store, project: &str, job: &Value) -> Result<()> {
    let id = required(job, "id")?;
    let dataset = required(job, "datasetId")?;
    let provider = store.provider(project, required(job, "provider")?)?;
    store.put_resource(project, "experiments", json!({"id":id,"status":"Running"}))?;
    let mut complete = 0;
    let mut failures = 0;
    for item in store
        .resources(project, "dataset-items")?
        .into_iter()
        .filter(|item| item["datasetId"] == dataset && item["status"] != "ARCHIVED")
    {
        let item_id = required(&item, "id")?;
        let run_id = stable_id(&[id, item_id], 16);
        if store
            .resource(project, "experiment-items", &run_id)?
            .is_some()
        {
            complete += 1;
            continue;
        }
        let input = item["input"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| item["input"].to_string());
        let start = now_ns();
        match provider.complete(project, None, &input).await {
            Ok((output, usage)) => {
                let trace = record_completion(
                    store,
                    project,
                    Completion {
                        source: "Experiment",
                        name: job["name"].as_str().unwrap_or("Experiment").to_owned(),
                        model: Some(provider.model.clone()),
                        input: item["input"].clone(),
                        output: json!(output),
                        usage,
                        start,
                        ..Default::default()
                    },
                )?;
                let evaluation = if let Some(evaluator) =
                    job["evaluator"].as_str().filter(|s| !s.is_empty())
                {
                    evaluate_trace(store,project,&json!({"evaluator":evaluator,"traceId":trace,"expectedOutput":item["expectedOutput"]})).await.ok()
                } else {
                    None
                };
                store.put_resource(
                    project,
                    "experiment-items",
                    json!({"id":run_id,"experimentId":id,"datasetItemId":item_id,
                    "traceId":trace,"output":output,"score":evaluation,"status":"Complete"}),
                )?;
                complete += 1;
            }
            Err(error) => {
                store.put_resource(
                    project,
                    "experiment-items",
                    json!({"id":run_id,"experimentId":id,"datasetItemId":item_id,
                    "error":error.to_string(),"status":"Failed"}),
                )?;
                failures += 1;
            }
        }
    }
    let status = if failures == 0 {
        "Complete"
    } else if complete == 0 {
        "Failed"
    } else {
        "Partial"
    };
    store.put_resource(
        project,
        "experiments",
        json!({"id":id,"status":status,"finishedAt":iso(now_ns()),
        "completed":complete,"failed":failures}),
    )?;
    store.notify(
        project,
        "Experiment",
        &format!("{complete} completed · {failures} failed"),
        if failures == 0 { "success" } else { "warning" },
        Some(id),
        true,
    )?;
    Ok(())
}

async fn evaluate_trace(store: &Store, project: &str, payload: &Value) -> Result<Value> {
    let trace_id = required(payload, "traceId")?;
    let trace = store.trace(project, trace_id)?.context("Trace not found")?;
    let evaluator = store
        .resource(project, "evaluators", required(payload, "evaluator")?)?
        .context("Evaluator not found")?;
    let inputs = json!({"input":trace.input,"output":trace.output,"expectedOutput":payload["expectedOutput"]});
    let value = if evaluator["type"] == "code" {
        let source = required(&evaluator, "code")?.to_owned();
        tokio::task::spawn_blocking(move || code_score(&source, &inputs)).await??
    } else {
        let provider = store.provider(project, required(&evaluator, "provider")?)?;
        let prompt = evaluator["prompt"]
            .as_str()
            .unwrap_or("Score the output between 0 and 1.");
        let request = format!(
            "{prompt}\nRespond with a JSON object containing value (a number) and comment.\n{inputs}"
        );
        let (value, _) = provider.complete(project, None, &request).await?;
        let value = value
            .trim()
            .trim_start_matches("```json")
            .trim_end_matches("```")
            .trim();
        serde_json::from_str::<Value>(value).context("Evaluator returned an invalid score")?
    };
    let score = value
        .get("value")
        .filter(|value| value.is_number() || value.is_string() || value.is_boolean())
        .context("Evaluator did not return a score")?;
    let evaluator_id = evaluator["id"].as_str().unwrap_or_default();
    store.put_score(
        project,
        json!({"id":stable_id(&["evaluator",trace_id,evaluator_id],16),"traceId":trace_id,
        "name":evaluator["name"],"value":score,"comment":value["comment"],"source":"EVAL"}),
    )
}

fn code_score(source: &str, inputs: &Value) -> Result<Value> {
    if source.len() > 64 * 1024 {
        bail!("Evaluator code is too large");
    }
    let runtime = rquickjs::Runtime::new()?;
    runtime.set_memory_limit(24 * 1024 * 1024);
    runtime.set_max_stack_size(256 * 1024);
    let deadline = Instant::now() + Duration::from_millis(750);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = rquickjs::Context::full(&runtime)?;
    let program = format!("JSON.stringify((({source}))({inputs}))");
    let raw = context
        .with(|ctx| ctx.eval::<String, _>(program.as_bytes()))
        .context("Evaluator did not finish or returned an unsupported value")?;
    if raw.len() > 128 * 1024 {
        bail!("Evaluator response is too large");
    }
    Ok(serde_json::from_str(&raw)?)
}

fn configure_export(store: &Store, project: &str, mut data: Value) -> Result<Value> {
    let directory = PathBuf::from(required(&data, "directory")?);
    if !directory.is_absolute() || !directory.is_dir() {
        bail!("Choose an export directory");
    }
    let frequency = data["intervalMinutes"].as_u64().unwrap_or(60);
    if !(1..=43200).contains(&frequency) {
        bail!("Choose an interval from 1 to 43200 minutes");
    }
    data["nextRunNs"] = json!(now_ns() + i64::try_from(frequency)? * 60_000_000_000);
    data["intervalMinutes"] = json!(frequency);
    store.put_resource(project, "exports", data)
}

fn write_export(store: &Store, project: &str, data: &Value) -> Result<Value> {
    let directory = PathBuf::from(required(data, "directory")?);
    if !directory.is_absolute() || !directory.is_dir() {
        bail!("Choose an export directory");
    }
    let is_csv = data["format"] == "csv";
    let suffix = if is_csv { "csv" } else { "jsonl" };
    let name = format!(
        "Confuse-{}-{}.{suffix}",
        now_ns() / 1_000_000_000,
        Uuid::new_v4().simple()
    );
    let destination = directory.join(name);
    let temporary = destination.with_extension("tmp");
    let result = (|| {
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let mut writer = BufWriter::new(file);
        let filter = TraceFilter {
            from: data.get("fromTimestamp").and_then(parse_time),
            to: data.get("toTimestamp").and_then(parse_time),
            source: data["source"].as_str().filter(|value| !value.is_empty()),
            ..Default::default()
        };
        if is_csv {
            writeln!(writer, "id,name,source,timestamp,sessionId")?;
        }
        let mut offset = 0;
        let mut count = 0;
        loop {
            let (traces, total) = store.traces(project, &filter, 500, offset)?;
            offset += traces.len();
            for trace in traces {
                if !matches_filters(&trace.public(), &data["filters"]) {
                    continue;
                }
                if is_csv {
                    let fields = [
                        trace.id,
                        trace.name,
                        trace.source,
                        iso(trace.start_ns),
                        trace.session_id.unwrap_or_default(),
                    ];
                    let fields = fields
                        .iter()
                        .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
                        .collect::<Vec<_>>();
                    writeln!(writer, "{}", fields.join(","))?;
                } else {
                    writeln!(writer, "{}", trace.public())?;
                }
                count += 1;
            }
            if offset >= total {
                break;
            }
        }
        writer.flush()?;
        fs::rename(&temporary, &destination)?;
        Ok(json!({"path":destination,"count":count}))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub async fn worker(store: Store) {
    let mut timer = tokio::time::interval(Duration::from_secs(3));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        let Ok(projects) = store.projects() else {
            continue;
        };
        for project in projects {
            let id = &project.id;
            if let Ok(runs) = store.resources(id, "experiments") {
                for run in runs
                    .into_iter()
                    .filter(|run| matches!(run["status"].as_str(), Some("Queued" | "Running")))
                {
                    if let Err(error) = experiment(&store, id, &run).await {
                        let _ = store.put_resource(
                            id,
                            "experiments",
                            json!({"id":run["id"],"status":"Failed","error":error.to_string()}),
                        );
                    }
                }
            }
            let _ = poll_alerts(&store, id);
            let _ = poll_evaluators(&store, id).await;
            if let Ok(schedules) = store.resources(id, "exports") {
                for schedule in schedules {
                    let now = now_ns();
                    if schedule["nextRunNs"]
                        .as_i64()
                        .is_some_and(|next| next <= now)
                        && schedule["active"] != false
                    {
                        let output = write_export(&store, id, &schedule);
                        let period = schedule["intervalMinutes"].as_i64().unwrap_or(60);
                        let _=store.put_resource(id,"exports",json!({"id":schedule["id"],"nextRunNs":now+period*60_000_000_000,
                            "lastResult":output.as_ref().ok(),"error":output.as_ref().err().map(|error|error.to_string())}));
                        let _ = store.notify(
                            id,
                            "Scheduled export",
                            if output.is_ok() {
                                "Export completed"
                            } else {
                                "Export failed"
                            },
                            if output.is_ok() { "success" } else { "warning" },
                            schedule["id"].as_str(),
                            true,
                        );
                    }
                }
            }
        }
    }
}

fn poll_alerts(store: &Store, project: &str) -> Result<()> {
    for item in store
        .resources(project, "alerts")?
        .into_iter()
        .filter(|row| row["active"] != false)
    {
        let now = now_ns();
        if item["nextCheckNs"].as_i64().is_some_and(|due| due > now) {
            continue;
        }
        let window = item["windowMinutes"].as_i64().unwrap_or(60).clamp(1, 43200);
        let measure = item["measure"].as_str().unwrap_or("count");
        let aggregation = item["aggregation"].as_str().unwrap_or("count");
        let value = store.metrics(
            project,
            &json!({"view":"observations",
            "fromTimestamp":iso(now-window*60_000_000_000),"toTimestamp":iso(now),
            "metrics":[{"measure":measure,"aggregation":aggregation}]}),
        )?;
        let key = format!("{aggregation}_{measure}");
        let value = value["data"][0][key.as_str()].as_f64().unwrap_or(0.0);
        let threshold = item["threshold"].as_f64().unwrap_or_default();
        let triggered = if item["comparator"] == "below" {
            value < threshold
        } else {
            value > threshold
        };
        if triggered && item["triggered"] != true {
            store.notify(
                project,
                item["name"].as_str().unwrap_or("Metric alert"),
                &format!("{measure} · {value:.2}"),
                "warning",
                item["id"].as_str(),
                true,
            )?;
        }
        store.put_resource(
            project,
            "alerts",
            json!({"id":item["id"],"triggered":triggered,
            "nextCheckNs":now+60_000_000_000_i64,"lastValue":value}),
        )?;
    }
    Ok(())
}

async fn poll_evaluators(store: &Store, project: &str) -> Result<()> {
    for rule in store
        .resources(project, "evaluation-rules")?
        .into_iter()
        .filter(|row| row["active"] != false)
    {
        let now = now_ns();
        let cursor = rule["cursorNs"]
            .as_i64()
            .or_else(|| parse_time(&rule["createdAt"]))
            .unwrap_or(now);
        let filter = TraceFilter {
            from: Some(cursor),
            to: Some(now - 5_000_000_000_i64),
            source: rule["source"].as_str().filter(|source| !source.is_empty()),
            ..Default::default()
        };
        let (rows, _) = store.traces(project, &filter, 50, 0)?;
        for row in rows {
            let id = rule["evaluator"].as_str().unwrap_or_default();
            let score = stable_id(&["evaluator", &row.id, id], 16);
            if store.score(project, &score)?.is_some() {
                continue;
            }
            if let Err(error) =
                evaluate_trace(store, project, &json!({"traceId":row.id,"evaluator":id})).await
            {
                store.put_resource(project,"evaluation-errors",json!({"evaluationRuleId":rule["id"],"traceId":row.id,"error":error.to_string()}))?;
            }
        }
        store.put_resource(
            project,
            "evaluation-rules",
            json!({"id":rule["id"],"cursorNs":now-5_000_000_000_i64}),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn code_evaluation_has_no_filesystem_or_network_bindings_and_times_out() {
        let value=code_score("data => ({value: data.input === 'example' && typeof fetch === 'undefined' && typeof require === 'undefined' ? 1 : 0})",
            &json!({"input":"example"})).unwrap();
        assert_eq!(value["value"], 1);
        assert!(code_score("() => { while (true) {} }", &json!({})).is_err());
    }
}
