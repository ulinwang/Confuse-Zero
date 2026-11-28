use crate::{
    desktop::required,
    model::{iso, now_ns, parse_time},
    store::Store,
};
use anyhow::{Result, anyhow, bail};
use rusqlite::params;
use serde_json::{Value, json};
use std::{fmt, sync::LazyLock};

mod dashboard;
mod datasets;
mod gateway;
pub(crate) mod playground;
mod topic_maps;
mod tracing;
mod versioned;

const LOCAL_ORGANIZATION: &str = "local";
static PROCEDURES: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../desktop/next/procedures.json"))
        .expect("valid Langfuse procedure manifest")
});

#[derive(Debug)]
struct ProcedureError {
    code: &'static str,
    message: String,
}

impl fmt::Display for ProcedureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProcedureError {}

fn fail(code: &'static str, message: impl Into<String>) -> anyhow::Error {
    ProcedureError {
        code,
        message: message.into(),
    }
    .into()
}

pub(crate) fn session(store: &Store) -> Result<Value> {
    let projects = store.access(|conn| {
        let mut statement = conn.prepare(
            "SELECT id,name,created_ns,EXISTS(SELECT 1 FROM traces WHERE project=projects.id)
             FROM projects ORDER BY created_ns,id",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "name": row.get::<_, String>(1)?,
                    "createdAt": iso(row.get(2)?),
                    "hasTraces": row.get::<_, bool>(3)?,
                    "metadata": {}, "deletedAt": null, "retentionDays": null,
                    "forceV3Experience": false, "traceEvalsEnabled": true, "scoreEditsDisabled": false,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    })?;
    let last_project = store.access(|conn| {
        use rusqlite::OptionalExtension;
        Ok(conn
            .query_row(
                "SELECT projects.id FROM projects JOIN settings ON settings.value=projects.id
             WHERE settings.key='last-visited-project'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?)
    })?;
    Ok(json!({ "projects": projects, "lastProjectId": last_project }))
}

pub(crate) async fn request(store: &Store, payload: &Value) -> Result<Value> {
    let name = required(payload, "procedure")?;
    let input = &payload["input"];
    let result = execute(store, name, input).await;
    Ok(match result {
        Ok(data) => {
            let mut dates = Vec::new();
            mark_dates(&data, "", &mut dates, name.starts_with("topics."));
            json!({ "data": data, "dates": dates })
        }
        Err(error) => {
            let code = error
                .downcast_ref::<ProcedureError>()
                .map(|details| details.code)
                .unwrap_or_else(|| {
                    if error.is::<rusqlite::Error>() {
                        "INTERNAL_SERVER_ERROR"
                    } else {
                        "BAD_REQUEST"
                    }
                });
            json!({ "error": { "code": code, "message": error.to_string() } })
        }
    })
}

async fn execute(store: &Store, name: &str, input: &Value) -> Result<Value> {
    if !input.is_object() {
        bail!("Expected an object");
    }
    let project = match PROCEDURES[name][1].as_str() {
        Some("project") => {
            let id = required(input, "projectId")?;
            let exists = store.access(|conn| {
                Ok(conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
                    [id],
                    |row| row.get::<_, bool>(0),
                )?)
            })?;
            if !exists {
                return Err(fail("NOT_FOUND", "Project not found"));
            }
            id
        }
        Some("organization") => {
            if input["orgId"] != LOCAL_ORGANIZATION && input["organizationId"] != LOCAL_ORGANIZATION
            {
                return Err(fail("FORBIDDEN", "Invalid local workspace"));
            }
            ""
        }
        Some("application") => "",
        _ => {
            return Err(fail(
                "NOT_IMPLEMENTED",
                format!("Workspace procedure is unavailable: {name}"),
            ));
        }
    };
    let value = match name {
        "public.traceReadConfig" => json!({ "v4WriteMode": "dual" }),
        "public.tracingSearchConfig" => json!({ "legacyTracingIoSearchEnabled": true }),
        "public.markdownRenderConfig" => json!({ "characterLimit": 200_000 }),
        "public.checkUpdate" => Value::Null,
        "backgroundMigrations.status" => json!({ "status": "FINISHED" }),
        "scores.getScoreColumns" => dashboard::score_columns(store, project, input)?,
        "organizations.lastTraceByProject" => dashboard::last_trace_by_project(store)?,
        _ if name.starts_with("llmApiKey.") => {
            playground::connection(store, project, name, input).await?
        }
        "projects.create" => json!(store.create_project(project_name(input, "name")?)?),
        "projects.visit" => {
            store.access(|conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO settings(key,value) VALUES('last-visited-project',?1)",
                    [project],
                )?;
                Ok(())
            })?;
            Value::Null
        }
        "projects.update" => {
            let name = project_name(input, "newName")?;
            store.transaction(|tx| {
                let duplicate = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM projects WHERE id<>?1 AND name=?2)",
                    params![project, name],
                    |r| r.get::<_, bool>(0),
                )?;
                if duplicate {
                    return Err(fail("CONFLICT", "A project with this name already exists"));
                }
                tx.execute(
                    "UPDATE projects SET name=?2 WHERE id=?1",
                    params![project, name],
                )?;
                Ok(json!(true))
            })?
        }
        "projects.environmentFilterOptions" => {
            let from = input.get("fromTimestamp").and_then(parse_time);
            store.access(|conn| {
                let mut query = conn.prepare(
                    "SELECT 'default' AS environment UNION
                     SELECT json_extract(data,'$.environment') FROM traces
                         WHERE project=?1 AND (?2 IS NULL OR start_ns>=?2)
                     UNION SELECT json_extract(data,'$.environment') FROM scores
                         WHERE project=?1 AND (?2 IS NULL OR created_ns>=?2)
                     ORDER BY environment",
                )?;
                let rows = query
                    .query_map(params![project, from], |row| {
                        row.get::<_, Option<String>>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(json!(
                    rows.into_iter()
                        .flatten()
                        .filter(|s| !s.is_empty())
                        .map(|environment| json!({ "environment": environment }))
                        .collect::<Vec<_>>()
                ))
            })?
        }
        "projectApiKeys.byProjectId" => json!(
            store
                .keys(project)?
                .into_iter()
                .filter(|key| key["kind"] == "sdk")
                .collect::<Vec<_>>()
        ),
        "projectApiKeys.create" => {
            if !input["role"].is_null() && input["role"] != "LEGACY_PROJECT_API_KEY" {
                bail!("This desktop workspace supports project SDK keys");
            }
            let expiry = if input["expiresAt"].is_null() {
                None
            } else {
                Some(
                    parse_time(&input["expiresAt"])
                        .filter(|time| *time > now_ns())
                        .ok_or_else(|| anyhow!("Expiration date must be in the future"))?,
                )
            };
            json!(store.create_key_with_attributes(
                project,
                "sdk",
                input["name"].as_str(),
                expiry
            )?)
        }
        "projectApiKeys.updateName" => {
            if !store.set_key_note(project, required(input, "keyId")?, required(input, "name")?)? {
                return Err(fail("NOT_FOUND", "API key not found"));
            }
            Value::Null
        }
        "projectApiKeys.delete" => {
            let id = required(input, "id")?;
            if !store
                .keys(project)?
                .iter()
                .any(|key| key["kind"] == "sdk" && key["id"] == id)
                || !store.revoke_key(project, id)?
            {
                return Err(fail("NOT_FOUND", "API key not found"));
            }
            Value::Null
        }
        _ if name.starts_with("dashboard.") => dashboard::request(store, project, name, input)?,
        _ if name.starts_with("datasets.") || name.starts_with("experiments.") => {
            datasets::request(store, project, name, input)?
        }
        _ if name.starts_with("aiGateway.") => gateway::request(store, name, input)?,
        _ if name.starts_with("topics.") => topic_maps::request(store, project, name, input)?,
        _ if name.starts_with("prompts.") || name.starts_with("skills.") => {
            versioned::request(store, project, name, input)?
        }
        _ => tracing::request(store, project, name, input)?,
    };
    Ok(value)
}

fn project_name<'a>(input: &'a Value, field: &str) -> Result<&'a str> {
    let name = required(input, field)?.trim();
    if name.len() > 128 || name.contains(['<', '>', '\0']) {
        bail!("Invalid project name");
    }
    Ok(name)
}

fn mark_dates(value: &Value, parent: &str, dates: &mut Vec<String>, topics: bool) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                let path = if parent.is_empty() {
                    key.to_owned()
                } else {
                    format!("{parent}.{key}")
                };
                match key.as_str() {
                    "metadata" | "input" | "output" | "config" | "prompt" | "schema"
                    | "toolParameters" | "modelParameters" | "definition" => {}
                    "from" | "to"
                        if topics
                            && (parent.ends_with("window") || parent.ends_with("timeRange")) =>
                    {
                        if parse_time(value).is_some() {
                            dates.push(path);
                        }
                    }
                    "timestamp"
                    | "createdAt"
                    | "updatedAt"
                    | "startTime"
                    | "endTime"
                    | "completionStartTime"
                    | "lastUsedAt"
                    | "expiresAt"
                    | "lastUpdatedAt"
                    | "finishedAt"
                    | "startedAt"
                    | "uploadedAt" => {
                        if !topics && value.is_string() && parse_time(value).is_some() {
                            dates.push(path);
                        }
                    }
                    _ => mark_dates(value, &path, dates, topics),
                }
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                let path = if parent.is_empty() {
                    index.to_string()
                } else {
                    format!("{parent}.{index}")
                };
                mark_dates(value, &path, dates, topics);
            }
        }
        _ => {}
    }
}
