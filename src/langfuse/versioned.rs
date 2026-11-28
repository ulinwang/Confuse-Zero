use super::{
    fail,
    tracing::{includes, paginate},
};
use crate::{desktop::required, model::parse_time, resources::string_list, skills, store::Store};
use anyhow::{Context, Result, bail};
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn request(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    let result = match name {
        "prompts.hasAny" => json!(!store.resources(project, "prompts")?.is_empty()),
        "prompts.byId" => store
            .resource(project, "prompts", required(input, "id")?)?
            .unwrap_or(Value::Null),
        "prompts.resolvePromptGraph" => {
            let id = required(input, "promptId")?;
            let parent = store
                .resource(project, "prompts", id)?
                .filter(|row| row["id"] == id)
                .ok_or_else(|| fail("NOT_FOUND", "Prompt version not found"))?;
            let mut graph = PromptGraph {
                root_name: required(&parent, "name")?,
                active: BTreeSet::new(),
                dependencies: BTreeMap::new(),
                visited: 0,
                size: 0,
            };
            let replacement = resolve_prompt(store, project, &parent, &mut graph, 0)?;
            json!({"resolvedPrompt":replacement,"graph":(!graph.dependencies.is_empty()).then(||json!({
                "root":prompt_ref(&parent),"dependencies":graph.dependencies,
            }))})
        }
        "prompts.metrics" => {
            let names = string_list(&input["promptNames"]);
            if names.len() > 50 {
                bail!("Choose up to 50 prompts");
            }
            let mut counts = BTreeMap::new();
            let from = parse_time(&input["fromTimestamp"]);
            let to = parse_time(&input["toTimestamp"]);
            for obs in store
                .observations(project, None, None, i64::MAX as usize, 0)?
                .0
            {
                if let Some(name) = obs.prompt_name.filter(|name| names.contains(name))
                    && from.is_none_or(|from| obs.start_ns >= from)
                    && to.is_none_or(|to| obs.start_ns < to)
                {
                    *counts.entry(name).or_insert(0_u64) += 1;
                }
            }
            json!(
                counts
                    .into_iter()
                    .map(|(name, count)| json!({"promptName":name,"observationCount":count}))
                    .collect::<Vec<_>>()
            )
        }
        "prompts.all" | "prompts.count" => {
            let rows = store.resources(project, "prompts")?;
            let rows = prompt_list(rows, input)?;
            let total = rows.len();
            if name == "prompts.count" {
                json!({ "totalCount": total })
            } else {
                json!({ "prompts": paginate(rows, input)?, "totalCount": total })
            }
        }
        "prompts.allVersions" => {
            let name = required(input, "name")?;
            let mut rows: Vec<_> = store
                .resources(project, "prompts")?
                .into_iter()
                .filter(|item| item["name"] == name)
                .map(|mut item| {
                    item["creator"] = json!(if item["createdBy"] == "API" {
                        "API"
                    } else {
                        "Local"
                    });
                    item
                })
                .collect();
            rows.sort_by_key(|item| std::cmp::Reverse(item["version"].as_i64()));
            let total = rows.len();
            if input["limit"].is_number() {
                rows = paginate(rows, input)?;
            }
            let mut result = json!({ "promptVersions": rows, "totalCount": total });
            if input["includeCommentCounts"] == true {
                result["commentCounts"] = json!({});
            }
            result
        }
        "prompts.filterOptions" => {
            let rows = store.resources(project, "prompts")?;
            json!({ "name": labeled(values(&rows, "name")), "labels": labeled(values(&rows, "labels")),
                "tags": labeled(values(&rows, "tags")) })
        }
        "prompts.allLabels" => json!(values(&store.resources(project, "prompts")?, "labels")),
        "prompts.allNames" => {
            let mut names = BTreeMap::new();
            for row in store.resources(project, "prompts")? {
                if input["type"]
                    .as_str()
                    .is_some_and(|kind| row["type"] != kind)
                {
                    continue;
                }
                let name = required(&row, "name")?.to_owned();
                let id = row["id"].clone();
                names
                    .entry(name.clone())
                    .or_insert(json!({ "id": id, "name": name }));
            }
            json!(names.into_values().take(1_000).collect::<Vec<_>>())
        }
        "prompts.getPromptLinkOptions" => {
            let mut names = BTreeMap::<String, (BTreeSet<i64>, BTreeSet<String>)>::new();
            for item in store
                .resources(project, "prompts")?
                .into_iter()
                .filter(|row| row["type"] == "text")
            {
                let entry = names
                    .entry(required(&item, "name")?.to_owned())
                    .or_default();
                if let Some(version) = item["version"].as_i64() {
                    entry.0.insert(version);
                }
                entry.1.extend(string_list(&item["labels"]));
            }
            json!(
                names
                    .into_iter()
                    .map(|(name, (versions, labels))| json!({
                        "name": name, "versions": versions, "labels": labels
                    }))
                    .collect::<Vec<_>>()
            )
        }
        "prompts.create" | "skills.createVersion" => {
            let kind = if name.starts_with("prompts.") {
                "prompts"
            } else {
                "skills"
            };
            let mut data = input.clone();
            data["createdBy"] = json!("confuse-local");
            if kind == "prompts" {
                validate_prompt_name(required(input, "name")?)?;
            }
            if let Some(obj) = data.as_object_mut() {
                obj.remove("id");
            }
            let saved = store.put_resource(project, kind, data)?;
            if kind == "skills" {
                skills::manifest(saved, false)
            } else {
                saved
            }
        }
        "prompts.setLabels" => {
            let row = store
                .resource(project, "prompts", required(input, "promptId")?)?
                .ok_or_else(|| fail("NOT_FOUND", "Prompt version not found"))?;
            store.set_version_labels(
                project,
                "prompts",
                required(&row, "name")?,
                row["version"].as_i64().context("Missing prompt version")?,
                &string_list(&input["labels"]),
            )?
        }
        "prompts.getProtectedLabels" => json!([]),
        "prompts.deleteVersion" => {
            let id = input["promptVersionId"]
                .as_str()
                .or(input["promptId"].as_str())
                .or_else(|| input["id"].as_str())
                .context("Prompt version ID is required")?;
            if !delete_version(store, project, "prompts", id)? {
                return Err(fail("NOT_FOUND", "Prompt version not found"));
            }
            json!({ "success": true })
        }
        "prompts.duplicatePrompt" => {
            let original = store
                .resource(project, "prompts", required(input, "promptId")?)?
                .ok_or_else(|| fail("NOT_FOUND", "Prompt version not found"))?;
            let new_name = required(input, "name")?;
            validate_prompt_name(new_name)?;
            if store
                .versioned(project, "prompts", new_name, None, Some("latest"))?
                .is_some()
            {
                return Err(fail("CONFLICT", "Prompt name already exists"));
            }
            let mut versions = if input["isSingleVersion"] == true {
                vec![original]
            } else {
                store
                    .resources(project, "prompts")?
                    .into_iter()
                    .filter(|row| row["name"] == original["name"])
                    .collect()
            };
            versions.sort_by_key(|row| row["version"].as_i64());
            let mut latest = Value::Null;
            for mut row in versions {
                row["name"] = json!(new_name);
                row["labels"] = json!([]);
                row["createdBy"] = json!("confuse-local");
                if let Some(fields) = row.as_object_mut() {
                    fields.remove("id");
                }
                latest = store.put_resource(project, "prompts", row)?;
            }
            latest
        }
        "prompts.delete" => delete_prompts(store, project, input)?,
        "skills.byName" => {
            let row = store
                .versioned(
                    project,
                    "skills",
                    required(input, "name")?,
                    input["version"].as_i64(),
                    input["label"].as_str(),
                )?
                .ok_or_else(|| fail("NOT_FOUND", "Skill version not found"))?;
            skills::manifest(row, input["includeContent"] == true)
        }
        "skills.fileContents" => {
            let hashes = if let Some(hashes) = input["sha256Hashes"].as_str() {
                hashes.to_owned()
            } else {
                string_list(&input["sha256Hashes"]).join(",")
            };
            json!({ "data": skills::contents(store, project, &hashes)? })
        }
        "skills.all" => {
            let query = input["query"]
                .as_str()
                .or(input["searchQuery"].as_str())
                .unwrap_or_default()
                .to_lowercase();
            let name = input["name"].as_str();
            let rows = skills::versions(store, project)?
                .into_iter()
                .filter(|row| {
                    name.is_none_or(|name| row["name"] == name)
                        && includes(row, &input["filter"])
                        && (query.is_empty()
                            || row["name"]
                                .as_str()
                                .is_some_and(|name| name.to_lowercase().contains(&query)))
                })
                .collect::<Vec<_>>();
            let count = rows.len();
            let limit = input["limit"].as_u64().unwrap_or(50).clamp(1, 100) as usize;
            let page = input["page"].as_u64().unwrap_or(1).max(1) as usize;
            let offset = page.saturating_sub(1).saturating_mul(limit);
            let data = rows
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>();
            json!({ "data": data, "meta": {"page":page,"limit":limit,"totalItems":count,
                "totalPages":count.div_ceil(limit),"hasNextPage":count>offset.saturating_add(limit)} })
        }
        "skills.filterOptions" => {
            let rows = skills::versions(store, project)?;
            json!({ "labels": labeled(values(&rows, "labels")), "tags": labeled(values(&rows, "tags")) })
        }
        "skills.skillVersions" => {
            let name = required(input, "name")?;
            let cursor = input["cursor"].as_i64().unwrap_or(i64::MAX);
            let limit = input["limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
            let mut items: Vec<_> = store
                .resources(project, "skills")?
                .into_iter()
                .filter(|row| {
                    row["name"] == name && row["version"].as_i64().is_some_and(|v| v < cursor)
                })
                .map(|row| skills::manifest(row, false))
                .collect();
            items.sort_by_key(|item| std::cmp::Reverse(item["version"].as_i64()));
            let next = (items.len() > limit).then(|| items[limit - 1]["version"].clone());
            items.truncate(limit);
            json!({ "items": items, "nextCursor": next })
        }
        "skills.setLabels" => skills::manifest(
            store.set_version_labels(
                project,
                "skills",
                required(input, "name")?,
                input["version"]
                    .as_i64()
                    .context("Skill version is required")?,
                &string_list(&input["labels"]),
            )?,
            false,
        ),
        "skills.setTags" => skills::manifest(
            skills::update_tags(
                store,
                project,
                required(input, "name")?,
                string_list(&input["tags"]),
            )?,
            false,
        ),
        "skills.deleteVersion" => {
            let row = store
                .versioned(
                    project,
                    "skills",
                    required(input, "name")?,
                    Some(
                        input["version"]
                            .as_i64()
                            .context("Skill version is required")?,
                    ),
                    None,
                )?
                .ok_or_else(|| fail("NOT_FOUND", "Skill version not found"))?;
            if !delete_version(store, project, "skills", required(&row, "id")?)? {
                return Err(fail("NOT_FOUND", "Skill version not found"));
            }
            json!({ "deleted": true })
        }
        "skills.deleteSkill" => {
            let name = required(input, "name")?;
            let changed = store.access(|conn| {
                Ok(conn.execute(
                    "DELETE FROM resources WHERE project=?1 AND kind='skills' AND name=?2",
                    params![project, name],
                )?)
            })?;
            if changed == 0 {
                return Err(fail("NOT_FOUND", "Skill not found"));
            }
            json!({ "deleted": true })
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

fn prompt_list(rows: Vec<Value>, input: &Value) -> Result<Vec<Value>> {
    let query = input["searchQuery"].as_str().unwrap_or("").to_lowercase();
    let folder = input["pathPrefix"].as_str().unwrap_or("").trim_matches('/');
    let prefix = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    let mut records = BTreeMap::<String, Value>::new();
    for mut row in rows {
        let name = required(&row, "name")?.to_owned();
        let Some(relative) = name.strip_prefix(&prefix) else {
            continue;
        };
        if !includes(&row, &input["filter"])
            || !(query.is_empty()
                || name.to_lowercase().contains(&query)
                || row["tags"].to_string().to_lowercase().contains(&query)
                || row["prompt"].to_string().to_lowercase().contains(&query))
        {
            continue;
        }
        if let Some((section, _)) = relative.split_once('/') {
            let path = format!("{prefix}{section}");
            records.entry(path.clone()).or_insert_with(|| {
                json!({
                    "id": path, "name": path, "row_type": "folder", "labels": [], "tags": [],
                    "createdAt": row["createdAt"], "updatedAt": row["updatedAt"],
                })
            });
        } else {
            if records
                .get(&name)
                .is_some_and(|prior| prior["version"].as_i64() >= row["version"].as_i64())
            {
                continue;
            }
            row["row_type"] = json!("prompt");
            records.insert(name, row);
        }
    }
    Ok(records.into_values().collect())
}

fn validate_prompt_name(name: &str) -> Result<()> {
    if name.len() > 512
        || name.contains(['<', '>', '\0'])
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        bail!("Invalid prompt name");
    }
    Ok(())
}

fn values(rows: &[Value], field: &str) -> Vec<String> {
    let mut distinct = BTreeSet::new();
    for row in rows {
        if let Some(value) = row[field].as_str() {
            distinct.insert(value.to_owned());
        } else {
            distinct.extend(string_list(&row[field]));
        }
    }
    distinct.into_iter().take(1_000).collect()
}

fn labeled(values: Vec<String>) -> Vec<Value> {
    values
        .into_iter()
        .map(|value| json!({ "value": value }))
        .collect()
}

fn delete_version(store: &Store, project: &str, kind: &str, id: &str) -> Result<bool> {
    let Some(row) = store.resource(project, kind, id)? else {
        return Ok(false);
    };
    let name = required(&row, "name")?;
    let latest = string_list(&row["labels"])
        .iter()
        .any(|label| label == "latest");
    store.transaction(|tx| {
        let removed = tx.execute("DELETE FROM resources WHERE project=?1 AND kind=?2 AND id=?3", params![project, kind, id])? > 0;
        if removed && latest {
            tx.execute(
                "UPDATE resources SET data=json_insert(data,'$.labels[#]','latest') WHERE project=?1 AND kind=?2
                 AND id=(SELECT id FROM resources WHERE project=?1 AND kind=?2 AND name=?3
                         ORDER BY CAST(json_extract(data,'$.version') AS INTEGER) DESC LIMIT 1)",
                params![project, kind, name],
            )?;
        }
        Ok(removed)
    })
}

const PROMPT_REFERENCE: &str = "@@@langfusePrompt:name=";

struct PromptGraph<'a> {
    root_name: &'a str,
    active: BTreeSet<String>,
    dependencies: BTreeMap<String, Vec<Value>>,
    visited: usize,
    size: usize,
}

fn prompt_ref(prompt: &Value) -> Value {
    json!({"id":prompt["id"],"name":prompt["name"],"version":prompt["version"]})
}

fn resolve_prompt(
    store: &Store,
    project: &str,
    prompt: &Value,
    graph: &mut PromptGraph<'_>,
    depth: usize,
) -> Result<Value> {
    let id = required(prompt, "id")?;
    graph.visited += 1;
    if depth >= 5 || graph.visited > 500 {
        return Err(fail(
            "CONFLICT",
            "Prompt dependencies exceed the resolution limit",
        ));
    }
    if (depth > 0 && prompt["name"] == graph.root_name) || !graph.active.insert(id.to_owned()) {
        return Err(fail("CONFLICT", "Circular prompt dependency"));
    }
    let mut resolved = prompt["prompt"].clone();
    resolve_fields(store, project, &mut resolved, id, graph, depth)?;
    graph.active.remove(id);
    Ok(resolved)
}

fn resolve_fields(
    store: &Store,
    project: &str,
    value: &mut Value,
    parent: &str,
    graph: &mut PromptGraph<'_>,
    depth: usize,
) -> Result<()> {
    match value {
        Value::String(text) => {
            let mut result = String::new();
            let mut remaining = text.as_str();
            while let Some((before, tail)) = remaining.split_once(PROMPT_REFERENCE) {
                result.push_str(before);
                let Some((reference, after)) = tail.split_once("@@@") else {
                    result.push_str(PROMPT_REFERENCE);
                    remaining = tail;
                    break;
                };
                remaining = after;
                let Some((name, target)) = reference.rsplit_once('|') else {
                    result.push_str(PROMPT_REFERENCE);
                    result.push_str(reference);
                    result.push_str("@@@");
                    continue;
                };
                let version = target
                    .strip_prefix("version=")
                    .and_then(|version| version.parse::<i64>().ok());
                let label = target.strip_prefix("label=");
                if version.is_none() && label.is_none() {
                    bail!("Invalid prompt dependency");
                }
                let child = store
                    .versioned(project, "prompts", name, version, label)?
                    .ok_or_else(|| {
                        fail(
                            "CONFLICT",
                            format!("Prompt dependency {name} was not found"),
                        )
                    })?;
                let edge = prompt_ref(&child);
                let edges = graph.dependencies.entry(parent.to_owned()).or_default();
                if !edges.contains(&edge) {
                    edges.push(edge);
                }
                let resolved = resolve_prompt(store, project, &child, graph, depth + 1)?;
                result.push_str(
                    resolved
                        .as_str()
                        .ok_or_else(|| fail("CONFLICT", "A prompt dependency must contain text"))?,
                );
            }
            result.push_str(remaining);
            graph.size = graph.size.saturating_add(result.len());
            if graph.size > 10_000_000 {
                return Err(fail("CONFLICT", "Resolved prompt exceeds the size limit"));
            }
            *text = result;
        }
        Value::Array(values) => {
            for value in values {
                resolve_fields(store, project, value, parent, graph, depth)?;
            }
        }
        Value::Object(fields) => {
            for value in fields.values_mut() {
                resolve_fields(store, project, value, parent, graph, depth)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn delete_prompts(store: &Store, project: &str, input: &Value) -> Result<Value> {
    let name = input["promptName"].as_str().filter(|name| !name.is_empty());
    let path = input["pathPrefix"]
        .as_str()
        .filter(|path| !path.is_empty())
        .map(|path| format!("{}/", path.trim_end_matches('/')));
    if name.is_none() && path.is_none() {
        bail!("Choose a prompt or folder");
    }
    let rows = store.resources(project, "prompts")?;
    let names = rows
        .iter()
        .filter_map(|row| {
            let value = row["name"].as_str()?;
            (name == Some(value) || path.as_ref().is_some_and(|path| value.starts_with(path)))
                .then_some(value)
        })
        .collect::<BTreeSet<_>>();
    if rows
        .iter()
        .filter(|row| {
            row["name"]
                .as_str()
                .is_some_and(|name| !names.contains(name))
        })
        .any(|row| {
            let text = row["prompt"].to_string();
            names
                .iter()
                .any(|name| text.contains(&format!("{PROMPT_REFERENCE}{name}|")))
        })
    {
        return Err(fail(
            "CONFLICT",
            "A prompt outside this selection depends on a selected prompt",
        ));
    }
    store.transaction(|tx| {
        for name in &names {
            tx.execute(
                "DELETE FROM resources WHERE project=?1 AND kind='prompts' AND name=?2",
                params![project, name],
            )?;
        }
        Ok(())
    })?;
    Ok(json!({"deletedNames":names}))
}
