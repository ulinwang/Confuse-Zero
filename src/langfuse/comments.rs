use super::fail;
use crate::{desktop::required, model::TraceFilter, store::Store};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn request(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    let rows = store.resources(project, "comments")?;
    match name {
        "comments.create" => {
            let kind = required(input, "objectType")?;
            let id = required(input, "objectId")?;
            if !object_exists(store, project, kind, id)? {
                return Err(fail("NOT_FOUND", "Comment target not found"));
            }
            let content = required(input, "content")?;
            if content.len() > 50_000 {
                bail!("Comment is too long");
            }
            let bits = json!({"objectId":id,"objectType":kind,"content":content,
                "dataField":input["dataField"],"path":input["path"].as_array().cloned().unwrap_or_default(),
                "rangeStart":input["rangeStart"].as_array().cloned().unwrap_or_default(),
                "rangeEnd":input["rangeEnd"].as_array().cloned().unwrap_or_default(),
                "authorUserId":"confuse-local","authorUserName":"Local","authorUserImage":null});
            store.put_resource(project, "comments", bits)
        }
        "comments.delete" => {
            let id = required(input, "commentId")?;
            let comment = rows
                .iter()
                .find(|comment| {
                    comment["id"] == id
                        && comment["objectId"] == input["objectId"]
                        && comment["objectType"] == input["objectType"]
                })
                .ok_or_else(|| fail("NOT_FOUND", "Comment not found"))?;
            if !comment["authorUserId"].is_null() && comment["authorUserId"] != "confuse-local" {
                return Err(fail("FORBIDDEN", "Only the author can delete this comment"));
            }
            store.delete_resource(project, "comments", id)?;
            Ok(Value::Null)
        }
        "comments.getByObjectId"
        | "comments.getCountByObjectId"
        | "comments.getCountByObjectType" => {
            let kind = required(input, "objectType")?;
            let id = input["objectId"].as_str();
            let found: Vec<_> = rows
                .into_iter()
                .filter(|comment| {
                    comment["objectType"] == kind && id.is_none_or(|id| comment["objectId"] == id)
                })
                .collect();
            if name == "comments.getByObjectId" {
                let _ = id.context("Comment target is required")?;
                return Ok(json!(found));
            }
            if name == "comments.getCountByObjectId" {
                return Ok(json!([[
                    id.context("Comment target is required")?,
                    found.len()
                ]]));
            }
            Ok(counts(&found))
        }
        "comments.getTraceCommentsBySessionId" | "comments.getTraceCommentCountsBySessionId" => {
            let session = required(input, "sessionId")?;
            let matching = store
                .traces(
                    project,
                    &TraceFilter {
                        session: Some(session),
                        ..Default::default()
                    },
                    i64::MAX as usize,
                    0,
                )?
                .0;
            let ids = matching
                .iter()
                .map(|trace| trace.id.as_str())
                .collect::<BTreeSet<_>>();
            let found: Vec<_> = rows
                .into_iter()
                .filter(|comment| {
                    comment["objectType"] == "TRACE"
                        && comment["objectId"]
                            .as_str()
                            .is_some_and(|id| ids.contains(id))
                })
                .collect();
            if name == "comments.getTraceCommentCountsBySessionId" {
                return Ok(counts(&found));
            }
            let mut groups = BTreeMap::<String, Vec<Value>>::new();
            for item in found.into_iter().rev() {
                groups
                    .entry(required(&item, "objectId")?.to_owned())
                    .or_default()
                    .push(item);
            }
            Ok(json!(groups))
        }
        _ => Err(fail(
            "NOT_IMPLEMENTED",
            format!("Workspace procedure is unavailable: {name}"),
        )),
    }
}

fn counts(rows: &[Value]) -> Value {
    let mut counts = BTreeMap::new();
    for row in rows {
        if let Some(id) = row["objectId"].as_str() {
            *counts.entry(id).or_insert(0) += 1;
        }
    }
    json!(counts.into_iter().collect::<Vec<_>>())
}

fn object_exists(store: &Store, project: &str, kind: &str, id: &str) -> Result<bool> {
    Ok(match kind {
        "TRACE" => store.trace(project, id)?.is_some(),
        "OBSERVATION" => store.observation(project, id)?.is_some(),
        "SESSION" => {
            store
                .traces(
                    project,
                    &TraceFilter {
                        session: Some(id),
                        ..Default::default()
                    },
                    0,
                    0,
                )?
                .1
                > 0
        }
        "PROMPT" => store.resource(project, "prompts", id)?.is_some(),
        "DATASET_RUN" => store.resource(project, "experiments", id)?.is_some(),
        "DATASET_ITEM" => store.resource(project, "dataset-items", id)?.is_some(),
        "SCORE" => store.score(project, id)?.is_some(),
        _ => bail!("Unknown comment target"),
    })
}
