use super::{
    fail,
    tracing::{includes, paginate},
};
use crate::{desktop::required, resources::string_list, store::Store, workspace::queue_experiment};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn request(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    let result = match name {
        "datasets.hasAny" => json!(!store.resources(project, "datasets")?.is_empty()),
        "datasets.allDatasetMeta" | "datasets.allNamesAndIds" => {
            json!(store.resources(project, "datasets")?)
        }
        "datasets.allDatasets" => {
            let rows = dataset_list(store, project, input)?;
            let count = rows.len();
            json!({ "datasets": paginate(rows, input)?, "totalDatasets": count })
        }
        "datasets.byId" => store
            .resource(project, "datasets", required(input, "datasetId")?)?
            .unwrap_or(Value::Null),
        "datasets.allDatasetsMetrics" => {
            let ids = string_list(&input["datasetIds"]);
            let items = store.resources(project, "dataset-items")?;
            let runs = store.resources(project, "experiments")?;
            json!({ "metrics": ids.iter().map(|id| {
                let dataset_runs: Vec<_> = runs.iter().filter(|run| run["datasetId"] == **id).collect();
                json!({
                    "id": id, "countDatasetItems": items.iter().filter(|item| item["datasetId"] == **id && item["status"] != "ARCHIVED").count(),
                    "countDatasetRuns": dataset_runs.len(),
                    "lastRunAt": dataset_runs.iter().filter_map(|run|run["createdAt"].as_str()).max(),
                })
            }).collect::<Vec<_>>() })
        }
        "datasets.createDataset" | "datasets.updateDataset" => {
            let id = if name == "datasets.updateDataset" {
                Some(required(input, "datasetId")?)
            } else {
                None
            };
            if let Some(id) = id {
                dataset(store, project, id)?;
            }
            let requested = required(input, "name")?.trim();
            if requested.len() > 512
                || requested.contains(['<', '>', '\0'])
                || requested.split('/').any(|part| part.is_empty())
            {
                bail!("Invalid dataset name");
            }
            if store
                .resources(project, "datasets")?
                .iter()
                .any(|dataset| dataset["name"] == requested && dataset["id"].as_str() != id)
            {
                return Err(fail("CONFLICT", "A dataset with this name already exists"));
            }
            let mut value = json!({ "id":id,"name":requested });
            for field in [
                "description",
                "inputSchema",
                "expectedOutputSchema",
                "metadata",
            ] {
                if let Some(supplied) = input.get(field) {
                    value[field] = json_field(supplied)?;
                }
            }
            json!({ "success":true,"dataset":store.put_resource(project,"datasets",value)? })
        }
        "datasets.countItemsByDatasetId" | "datasets.itemsByDatasetId" => {
            let id = required(input, "datasetId")?;
            dataset(store, project, id)?;
            reject_snapshot(input, "version")?;
            let rows = items(store, project, id, input)?;
            let count = rows.len();
            if name == "datasets.countItemsByDatasetId" {
                json!(count)
            } else {
                json!({ "datasetItems": paginate(rows, input)?, "totalDatasetItems": count })
            }
        }
        "datasets.listDatasetVersions" => {
            dataset(store, project, required(input, "datasetId")?)?;
            // The imported SQLite schema has no point-in-time dataset versions.
            json!([])
        }
        "datasets.itemById" | "datasets.itemByIdAtVersion" => {
            reject_snapshot(input, "version")?;
            let id = input["datasetItemId"]
                .as_str()
                .or(input["itemId"].as_str())
                .context("Dataset item ID is required")?;
            store
                .resource(project, "dataset-items", id)?
                .filter(|item| {
                    input["datasetId"]
                        .as_str()
                        .is_none_or(|id| item["datasetId"] == id)
                })
                .unwrap_or(Value::Null)
        }
        "datasets.createDatasetItem" | "datasets.updateDatasetItem" => {
            let id = required(input, "datasetId")?;
            dataset(store, project, id)?;
            let mut data = item_fields(input)?;
            if name == "datasets.updateDatasetItem" {
                let previous = store
                    .resource(project, "dataset-items", required(input, "datasetItemId")?)?
                    .filter(|row| row["datasetId"] == id)
                    .ok_or_else(|| fail("NOT_FOUND", "Dataset item not found"))?;
                data["id"] = previous["id"].clone();
            }
            store.put_resource(project, "dataset-items", data)?
        }
        "datasets.deleteDatasetItem" => {
            let id = required(input, "datasetItemId")?;
            let existing = store
                .resource(project, "dataset-items", id)?
                .filter(|row| {
                    input["datasetId"]
                        .as_str()
                        .is_none_or(|id| row["datasetId"] == id)
                })
                .ok_or_else(|| fail("NOT_FOUND", "Dataset item not found"))?;
            store.delete_resource(project, "dataset-items", required(&existing, "id")?)?;
            Value::Null
        }
        "datasets.runsByDatasetId" | "datasets.baseRunDataByDatasetId" => {
            let id = required(input, "datasetId")?;
            let runs = runs(store, project, Some(id), input)?;
            if name == "datasets.baseRunDataByDatasetId" {
                json!(runs)
            } else {
                let count = runs.len();
                json!({ "runs": paginate(runs, input)?, "totalRuns": count })
            }
        }
        "datasets.runById" | "experiments.byId" => {
            let id = input["runId"]
                .as_str()
                .or(input["experimentId"].as_str())
                .context("Run ID is required")?;
            store
                .resource(project, "experiments", id)?
                .filter(|row| {
                    input["datasetId"]
                        .as_str()
                        .is_none_or(|id| row["datasetId"] == id)
                })
                .map(experiment)
                .unwrap_or(Value::Null)
        }
        "experiments.createExperiment" => {
            reject_snapshot(input, "datasetVersion")?;
            if !input["structuredOutputSchema"].is_null() {
                bail!("Structured output is unavailable for local dataset runs");
            }
            let dataset_id = required(input, "datasetId")?;
            let prompt_id = required(input, "promptId")?;
            store
                .resource(project, "prompts", prompt_id)?
                .ok_or_else(|| fail("NOT_FOUND", "Prompt version not found"))?;
            let model = &input["modelConfig"];
            let provider = required(model, "provider")?;
            let connection = store
                .resources(project, "llm-connections")?
                .into_iter()
                .find(|item| item["name"] == provider || item["id"] == provider)
                .context("Choose a local model connection")?;
            let run_name = required(input, "runName")?;
            let job = queue_experiment(
                store,
                project,
                json!({
                    "name":run_name,"datasetId":dataset_id,"promptId":prompt_id,"provider":connection["id"],
                    "model":required(model,"model")?,"description":input["description"],
                    "metadata":{"prompt_id":prompt_id,"provider":provider,"model":model["model"],
                        "experiment_name":required(input,"name")?,"experiment_run_name":run_name},
                }),
            )?;
            json!({ "success":true,"datasetId":dataset_id,"runId":job["id"],"runName":run_name })
        }
        "experiments.validateConfig" => validation(store, project, input)?,
        "experiments.all" | "experiments.mostRecent" | "experiments.countAll" => {
            let runs = runs(store, project, input["datasetId"].as_str(), input)?;
            if name == "experiments.countAll" {
                json!({ "totalCount":runs.len() })
            } else {
                json!({ "data":paginate(runs,input)? })
            }
        }
        "datasets.runItemsByRunId" | "datasets.runItemsByItemId" => {
            let mut rows = store.resources(project, "experiment-items")?;
            rows.retain(|row| {
                input["runId"]
                    .as_str()
                    .is_none_or(|id| row["experimentId"] == id)
                    && input["datasetItemId"]
                        .as_str()
                        .is_none_or(|id| row["datasetItemId"] == id)
                    && includes(row, &input["filter"])
            });
            let total = rows.len();
            for row in &mut rows {
                row["datasetRunId"] = row["experimentId"].clone();
            }
            let items = paginate(rows, input)?;
            json!({ "runItems":items,"totalCount":total })
        }
        _ => {
            return Err(fail(
                "NOT_IMPLEMENTED",
                format!("Workspace procedure is unavailable: {name}"),
            ));
        }
    };
    Ok(result)
}

fn dataset(store: &Store, project: &str, id: &str) -> Result<Value> {
    store
        .resource(project, "datasets", id)?
        .ok_or_else(|| fail("NOT_FOUND", "Dataset not found"))
}

fn json_field(value: &Value) -> Result<Value> {
    match value.as_str() {
        Some(text) if text.trim().is_empty() => Ok(Value::Null),
        Some(text) => Ok(serde_json::from_str(text).context("Invalid JSON")?),
        None => Ok(value.clone()),
    }
}

fn item_fields(input: &Value) -> Result<Value> {
    let mut data = json!({ "datasetId":required(input,"datasetId")? });
    for key in ["input", "expectedOutput", "metadata"] {
        if let Some(value) = input.get(key) {
            data[key] = json_field(value)?;
        }
    }
    for key in ["status", "sourceTraceId", "sourceObservationId"] {
        if let Some(value) = input.get(key) {
            data[key] = value.clone();
        }
    }
    if !data["status"].is_null() && !matches!(data["status"].as_str(), Some("ACTIVE" | "ARCHIVED"))
    {
        bail!("Invalid item status");
    }
    Ok(data)
}

fn reject_snapshot(input: &Value, field: &str) -> Result<()> {
    if !input[field].is_null() {
        bail!("Historical dataset versions are unavailable");
    }
    Ok(())
}

fn dataset_list(store: &Store, project: &str, input: &Value) -> Result<Vec<Value>> {
    let query = input["searchQuery"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    let prefix = input["pathPrefix"]
        .as_str()
        .unwrap_or_default()
        .trim_matches('/');
    let prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/")
    };
    let mut folders = BTreeSet::new();
    let mut rows = Vec::new();
    for mut row in store.resources(project, "datasets")? {
        let name = required(&row, "name")?;
        let Some(relative) = name.strip_prefix(&prefix) else {
            continue;
        };
        if !name.to_lowercase().contains(&query) {
            continue;
        }
        if let Some((section, _)) = relative.split_once('/') {
            let path = format!("{prefix}{section}");
            if folders.insert(path.clone()) {
                rows.push(json!({ "id":path,"name":path,"row_type":"folder","updatedAt":row["updatedAt"] }));
            }
        } else {
            row["row_type"] = json!("dataset");
            rows.push(row);
        }
    }
    Ok(rows)
}

fn items(store: &Store, project: &str, id: &str, input: &Value) -> Result<Vec<Value>> {
    let search = input["searchQuery"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    Ok(store
        .resources(project, "dataset-items")?
        .into_iter()
        .filter(|row| {
            row["datasetId"] == id
                && includes(row, &input["filter"])
                && (search.is_empty() || row.to_string().to_lowercase().contains(&search))
        })
        .collect())
}

fn experiment(mut job: Value) -> Value {
    job["experimentId"] = job["id"].clone();
    job["experimentName"] = job["metadata"]["experiment_name"].clone();
    job["experimentRunName"] = job["name"].clone();
    job["datasetName"] = job["metadata"]["dataset_name"].clone();
    job["startTime"] = job["createdAt"].clone();
    job["endTime"] = job["finishedAt"].clone();
    job
}

fn runs(
    store: &Store,
    project: &str,
    dataset_id: Option<&str>,
    input: &Value,
) -> Result<Vec<Value>> {
    Ok(store
        .resources(project, "experiments")?
        .into_iter()
        .map(experiment)
        .filter(|row| {
            dataset_id.is_none_or(|id| row["datasetId"] == id) && includes(row, &input["filter"])
        })
        .collect())
}

fn validation(store: &Store, project: &str, input: &Value) -> Result<Value> {
    reject_snapshot(input, "datasetVersion")?;
    let id = required(input, "datasetId")?;
    dataset(store, project, id)?;
    let prompt = store
        .resource(project, "prompts", required(input, "promptId")?)?
        .ok_or_else(|| fail("NOT_FOUND", "Prompt version not found"))?;
    let rows = items(store, project, id, &Value::Null)?
        .into_iter()
        .filter(|row| row["status"] != "ARCHIVED")
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Ok(json!({"isValid":false,"message":"The dataset has no active items"}));
    }
    let encoded = prompt["prompt"].to_string();
    let mut variables = BTreeMap::new();
    for (offset, _) in encoded.match_indices("{{") {
        let start = offset + 2;
        if let Some((name, _)) = encoded[start..].split_once("}}") {
            let name = name.trim();
            let count = rows
                .iter()
                .filter(|row| row["input"].get(name).is_some())
                .count();
            if count != rows.len() {
                return Ok(
                    json!({"isValid":false,"message":format!("Some items have no {name} value")}),
                );
            }
            variables.insert(name.to_owned(), count);
        }
    }
    Ok(json!({"isValid":true,"totalItems":rows.len(),"variablesMap":variables}))
}
