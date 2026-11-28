use super::fail;
use crate::store::Store;
use anyhow::{Context, Result, bail};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};

const CONFIG: &str = "local-gateway-config";

pub(super) fn request(store: &Store, name: &str, input: &Value) -> Result<Value> {
    let config = get(store)?;
    match name {
        "aiGateway.getSetupStatus" => {
            let project = config["defaultIngestionProject"].is_object();
            let mode = config["ingestionMode"].as_str();
            Ok(
                json!({ "isConfigured": mode == Some("NONE") || project, "isRecording": mode != Some("NONE") && project }),
            )
        }
        "aiGateway.getConfig" => Ok(config),
        "aiGateway.getOverviewConfig" => Ok(json!({ "config": config, "hasSpendLimits": false })),
        "aiGateway.updateConfig" => {
            let project = input["defaultIngestionProjectId"].as_str();
            let mode = input["ingestionMode"]
                .as_str()
                .context("Ingestion mode is required")?;
            // Existing project-issued gateway keys always record into their
            // own project. Other modes require a change to that contract.
            if mode != "FULL" {
                bail!("Choose full recording into a local project");
            }
            let id = project.context("Select a recording project")?;
            let chosen = store
                .projects()?
                .into_iter()
                .find(|project| project.id == id)
                .ok_or_else(|| fail("NOT_FOUND", "Project not found"))?;
            let value = json!({ "organizationId":"local", "defaultIngestionProjectId": chosen.id,
                "defaultIngestionProject": chosen, "ingestionMode": mode });
            store.access(|conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)",
                    (CONFIG, value.to_string()),
                )?;
                Ok(())
            })?;
            Ok(value)
        }
        _ => Err(fail(
            "NOT_IMPLEMENTED",
            format!("Workspace procedure is unavailable: {name}"),
        )),
    }
}

fn get(store: &Store) -> Result<Value> {
    let existing: Option<String> = store.access(|conn| {
        Ok(conn
            .query_row("SELECT value FROM settings WHERE key=?1", [CONFIG], |row| {
                row.get(0)
            })
            .optional()?)
    })?;
    if let Some(data) = existing {
        return Ok(serde_json::from_str(&data)?);
    }
    Ok(Value::Null)
}
