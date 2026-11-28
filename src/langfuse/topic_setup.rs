use super::{
    fail,
    topic_maps::{get_map, rule, stored_map_models},
};
use crate::{
    desktop::required,
    model::{iso, now_ns, parse_time, stable_id},
    providers::Provider,
    resources::string_list,
    store::Store,
    topics,
};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const MAPS: &str = "topic-maps";
const SLOTS: [(&str, &str, &str); 3] = [
    ("summary", "summaryProvider", "summaryModel"),
    ("embedding", "embeddingProvider", "embeddingModel"),
    ("clustering", "namingProvider", "namingModel"),
];

pub(super) async fn request(
    store: &Store,
    project: &str,
    name: &str,
    input: &Value,
) -> Result<Value> {
    match name {
        "topics.saveFacet" => save_facet(store, project, input),
        "topics.saveSetup" => save_setup(store, project, input),
        "topics.testModelSettings" => test_models(store, project, input).await,
        "topics.saveRule" => {
            let settings = config(store, project)?;
            Ok(rule(&save_rule(store, project, input, &settings)?))
        }
        "topics.startAnalysis" => start_analysis(store, project, input),
        "topics.setAutomaticProcessing" => set_processing(store, project, input),
        "topics.startCostEstimate" | "topics.costEstimate" => {
            cost_estimate(store, project, name, input)
        }
        _ => Err(fail(
            "NOT_IMPLEMENTED",
            format!("Workspace procedure is unavailable: {name}"),
        )),
    }
}

fn config_key(project: &str) -> String {
    format!("topic-settings:{project}")
}

fn load(store: &Store, project: &str) -> Result<Option<Value>> {
    let text: Option<String> = store.access(|conn| {
        Ok(conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                [config_key(project)],
                |row| row.get(0),
            )
            .optional()?)
    })?;
    text.map(|text| serde_json::from_str(&text).map_err(Into::into))
        .transpose()
}

fn persist(store: &Store, project: &str, settings: &Value) -> Result<()> {
    store.access(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)",
            params![config_key(project), settings.to_string()],
        )?;
        Ok(())
    })
}

pub(super) fn config(store: &Store, project: &str) -> Result<Value> {
    let Some(settings) = load(store, project)? else {
        return stored_map_models(store, project);
    };
    Ok(settings)
}

pub(crate) fn processing_enabled(store: &Store, project: &str) -> Result<bool> {
    Ok(load(store, project)?.is_none_or(|settings| settings["enabled"] != false))
}

fn model_slot(store: &Store, project: &str, value: &Value) -> Result<Provider> {
    let name = required(value, "provider")?;
    let id = store
        .resources(project, "llm-connections")?
        .into_iter()
        .find(|item| item["name"] == name || item["id"] == name)
        .ok_or_else(|| fail("NOT_FOUND", "Model connection not found in this project"))?;
    store.provider(project, required(&id, "id")?)
}

fn map_models(store: &Store, project: &str, settings: &Value) -> Result<Value> {
    let dimensions = settings["embeddingDimensions"].as_u64().unwrap_or(1024);
    if !(64..=2048).contains(&dimensions) {
        bail!("Choose 64 to 2048 embedding dimensions");
    }
    let mut map = json!({"dimensions":dimensions,
        "readinessSeconds":settings["traceIdleMs"].as_u64().unwrap_or(300_000)/1_000});
    for (slot, provider_key, model_key) in SLOTS {
        let value = &settings[slot];
        if value.is_null() {
            bail!("Choose a model for {slot}");
        }
        let provider = model_slot(store, project, value)?;
        if slot == "embedding" && provider.format != "openai" {
            bail!("Choose an OpenAI-compatible embeddings connection");
        }
        map[provider_key] = json!(provider.id);
        map[model_key] = json!(required(value, "model")?);
    }
    Ok(map)
}

async fn test_models(store: &Store, project: &str, input: &Value) -> Result<Value> {
    let dimensions = input["embeddingDimensions"].as_u64().unwrap_or(1024);
    if !(64..=2048).contains(&dimensions) {
        bail!("Choose 64 to 2048 embedding dimensions");
    }
    let check = |slot: &'static str| async move {
        let value = &input[slot];
        if value.is_null() {
            return Ok(());
        }
        let model = required(value, "model")?;
        let mut provider = model_slot(store, project, value)?;
        if slot == "embedding" {
            provider.embedding_model = model.to_owned();
            provider
                .embed(project, "Confuse connection check", dimensions as usize)
                .await?;
        } else {
            provider
                .complete(project, Some(model), "Reply with OK.")
                .await?;
        }
        Ok::<(), anyhow::Error>(())
    };
    let (summary, embedding, clustering) =
        tokio::join!(check("summary"), check("embedding"), check("clustering"));
    Ok(json!(
        [
            ("summary", summary),
            ("embedding", embedding),
            ("clustering", clustering)
        ]
        .into_iter()
        .filter_map(|(slot, result)| result.err().map(|error| (slot, error.to_string())))
        .collect::<BTreeMap<_, _>>()
    ))
}

fn save_setup(store: &Store, project: &str, input: &Value) -> Result<Value> {
    let current = config(store, project)?;
    let mut settings = current.clone();
    let idle = input["traceIdleMs"]
        .as_u64()
        .context("Trace idle time is required")?;
    if !(60_000..=3_600_000).contains(&idle) || idle % 1_000 != 0 {
        bail!("Choose a whole-second idle time between 1 and 60 minutes");
    }
    let dimensions = input["embeddingDimensions"]
        .as_u64()
        .context("Embedding dimensions are required")?;
    if !(64..=2048).contains(&dimensions) {
        bail!("Choose 64 to 2048 embedding dimensions");
    }
    settings["traceIdleMs"] = json!(idle);
    settings["embeddingDimensions"] = json!(dimensions);
    if !settings["enabled"].is_boolean() {
        settings["enabled"] = json!(false);
    }
    for (slot, _, _) in SLOTS {
        if settings["enabled"] == true && input[slot].is_null() {
            bail!("Choose all models while automatic processing is enabled");
        }
        if !input[slot].is_null() {
            model_slot(store, project, &input[slot])?;
            let _ = required(&input[slot], "model")?;
        }
        settings[slot] = input[slot].clone();
    }
    let saved = if let Some(selected) = input["facets"].as_array() {
        let definitions = topics::facet_versions(store, project)?;
        let mut ids = string_list(&input["customFacetIds"]);
        for choice in selected.iter().filter(|choice| choice["enabled"] == true) {
            let name = required(choice, "name")?;
            let item = definitions
                .iter()
                .find(|facet| facet["isBuiltIn"] == true && facet["name"] == name)
                .context("Unknown built-in Topics facet")?;
            ids.push(required(item, "id")?.to_owned());
        }
        ids.sort();
        ids.dedup();
        let existing = input["ruleId"]
            .as_str()
            .map(|id| get_map(store, project, id))
            .transpose()?;
        let mut fields = json!({"name":input["name"].as_str().or_else(||existing.as_ref()?.get("name")?.as_str()).unwrap_or("Default"),
            "facetIds":ids,"id":input["ruleId"]});
        fields["filter"] = input
            .get("filter")
            .cloned()
            .or_else(|| existing.as_ref().map(|map| map["filters"].clone()))
            .unwrap_or(json!([]));
        rule(&save_rule(store, project, &fields, &settings)?)
    } else {
        if input["ruleId"].is_string() {
            bail!("Choose the facets for the Topics rule");
        }
        Value::Null
    };
    persist(store, project, &settings)?;
    Ok(saved)
}

fn save_facet(store: &Store, project: &str, input: &Value) -> Result<Value> {
    let name = required(input, "name")?.trim();
    let prompt = required(input, "prompt")?.trim();
    let description = input["description"].as_str().unwrap_or_default();
    if name.len() > 100 || !(10..=4_000).contains(&prompt.len()) || description.len() > 500 {
        bail!("Invalid topic facet name, description or prompt");
    }
    let existing = input["facetId"].as_str();
    let saved = topics::facet_versions(store, project)?;
    if saved
        .iter()
        .any(|facet| facet["name"] == name && facet["id"].as_str() != existing)
    {
        return Err(fail("CONFLICT", "A facet with this name already exists"));
    }
    let id = existing
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let previous = saved.iter().find(|facet| facet["id"] == id);
    if existing.is_some() && previous.is_none() {
        return Err(fail("NOT_FOUND", "Facet not found"));
    }
    let mut versions = previous
        .and_then(|facet| facet["versions"].as_array().cloned())
        .unwrap_or_default();
    let version = versions
        .first()
        .and_then(|version| version["version"].as_u64())
        .unwrap_or(0)
        + 1;
    versions.insert(0,json!({"projectId":project,"facetId":id,"version":version,"prompt":prompt,"createdAt":iso(now_ns())}));
    store.put_resource(
        project,
        "topic-facets",
        json!({"id":id,"name":name,"description":description,
        "isBuiltIn":previous.is_some_and(|facet|facet["isBuiltIn"] == true),"versions":versions}),
    )
}

fn save_rule(store: &Store, project: &str, input: &Value, settings: &Value) -> Result<Value> {
    let mut map = map_models(store, project, settings)?;
    let existing = input["id"]
        .as_str()
        .map(|id| get_map(store, project, id))
        .transpose()?;
    if existing.as_ref().is_some_and(|map| map["mode"] != "live") {
        bail!("Choose a live map");
    }
    if existing.is_none()
        && store
            .resources(project, MAPS)?
            .iter()
            .filter(|map| map["mode"] == "live")
            .count()
            >= 5
    {
        bail!("This project already has five live maps");
    }
    map["id"] = input["id"].clone();
    map["mode"] = json!("live");
    map["name"] = json!(required(input, "name")?);
    map["facetIds"] = input["facetIds"].clone();
    map["filters"] = valid_filters(&input["filter"])?;
    map["sample"] = json!(
        sample(input, "sampling")?
            .or_else(|| existing.as_ref()?.get("sample")?.as_u64())
            .unwrap_or(100)
    );
    let mut saved = topics::save(store, project, map)?;
    if existing.as_ref().is_some_and(|map| map["enabled"] == false) {
        saved = topics::change(store, project, required(&saved, "id")?, "pause")?;
    }
    Ok(saved)
}

fn valid_filters(value: &Value) -> Result<Value> {
    if value.is_null() {
        return Ok(json!([]));
    }
    let filters = value
        .as_array()
        .filter(|filters| filters.len() <= 100)
        .context("Use at most 100 filters")?;
    if filters
        .iter()
        .any(|filter| filter["type"] == "positionInTrace")
    {
        bail!("Position filters are unavailable for Topics");
    }
    Ok(value.clone())
}

fn sample(input: &Value, field: &str) -> Result<Option<u64>> {
    if input[field].is_null() {
        return Ok(None);
    }
    let rate = input[field].as_f64().context("Sampling must be a number")?;
    if !(0.01..=1.0).contains(&rate) {
        bail!("Choose a sampling rate from 1 to 100 percent");
    }
    Ok(Some((rate * 100.).round() as u64))
}

fn set_processing(store: &Store, project: &str, input: &Value) -> Result<Value> {
    let enabled = input["enabled"].as_bool().context("Enabled is required")?;
    let mut config = config(store, project)?;
    if enabled {
        map_models(store, project, &config)?;
    }
    let sampling = sample(input, "sampling")?;
    if let (Some(id), Some(rate)) = (input["ruleId"].as_str(), sampling) {
        let map = get_map(store, project, id)?;
        if map["mode"] != "live" {
            bail!("Select a live map");
        }
        store.put_resource(project, MAPS, json!({"id":id,"sample":rate}))?;
    }
    config["enabled"] = json!(enabled);
    persist(store, project, &config)?;
    Ok(json!({"enabled":enabled,"sampling":sampling.map(|rate|rate as f64/100.)}))
}

fn start_analysis(store: &Store, project: &str, input: &Value) -> Result<Value> {
    let config = config(store, project)?;
    let mut map = map_models(store, project, &config)?;
    let range = &input["timeRange"];
    let from = parse_time(&range["from"]).context("Choose a snapshot start time")?;
    let to = parse_time(&range["to"]).context("Choose a snapshot end time")?;
    if from >= to || to.saturating_sub(from) > 30 * 86_400_000_000_000 {
        bail!("Choose a snapshot interval of up to 30 days");
    }
    map["mode"] = json!("snapshot");
    map["name"] = json!(required(input, "label")?);
    map["fromTimestamp"] = json!(iso(from));
    map["toTimestamp"] = json!(iso(to));
    map["filters"] = valid_filters(&input["filter"])?;
    let facets = input["facets"]
        .as_array()
        .context("Choose snapshot facets")?;
    let definitions = topics::facet_versions(store, project)?;
    for facet in facets {
        let id = required(facet, "facetId")?;
        if !definitions
            .iter()
            .any(|item| item["id"] == id && item["versions"][0]["version"] == facet["version"])
        {
            bail!("Choose the active version of each facet");
        }
    }
    map["facetIds"] = json!(
        facets
            .iter()
            .map(|facet| &facet["facetId"])
            .collect::<Vec<_>>()
    );
    map["reuseSummariesOnly"] = json!(input["summarize"].is_null());
    if !input["summarize"].is_null() {
        let limit = input["summarize"]["limit"]
            .as_u64()
            .filter(|limit| (1..=20_000).contains(limit))
            .context("Invalid snapshot summary limit")?;
        map["summaryLimit"] = json!(limit);
    }
    let saved = topics::save(store, project, map)?;
    Ok(json!({"id":saved["id"],"status":"queued"}))
}

fn cost_estimate(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    let mut config = config(store, project)?;
    for (slot, _, _) in SLOTS {
        if let Some(value) = input.get(slot) {
            config[slot] = value.clone();
        }
    }
    if config["summary"].is_null() {
        return Ok(Value::Null);
    }
    let key = stable_id(&[&input.to_string(), &config.to_string()], 16);
    if name == "topics.costEstimate" {
        return Ok(store
            .resource(project, "topic-cost-estimates", &key)?
            .unwrap_or(Value::Null));
    }
    let mut map = map_models(store, project, &config)?;
    map["facetIds"] = input["facetIds"].clone();
    map["filters"] = valid_filters(&input["filter"])?;
    let now = now_ns();
    let estimate = topics::estimate(store, project, &map, now)?;
    let count = estimate["eligibleTraces"].as_u64().unwrap_or(0);
    let per_trace = estimate["estimatedUsd"]
        .as_f64()
        .filter(|_| count > 0)
        .map(|cost| cost / count as f64);
    store.put_resource(project,"topic-cost-estimates",json!({
        "id":key,"key":key,"runId":key,"status":"done",
        "startedAt":now/1_000_000,"finishedAt":now_ns()/1_000_000,"error":null,
        "inputs":{"filter":map["filters"],"summaryModel":config["summary"]["model"],"embeddingModel":config["embedding"]["model"],
            "facets":string_list(&map["facetIds"]).iter().map(|id|json!({"facetId":id,"version":1})).collect::<Vec<_>>()},
        "sampled":count,"summarizable":count,"skipped":{"tooLarge":0,"noContent":0},
        "tokens":{"inputTokens":estimate["inputTokens"],"outputTokens":estimate["outputTokens"],"embeddingTokens":estimate["embeddingTokens"]},
        "prices":{"perTraceUsd":per_trace,"summaryPriced":estimate["summaryPriced"],"embeddingPriced":estimate["embeddingPriced"]},
    }))?;
    Ok(json!({"key":key}))
}
