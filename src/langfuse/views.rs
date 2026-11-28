use super::fail;
use crate::{desktop::required, store::Store};
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::LazyLock;

const KIND: &str = "table-view-presets";
const SYSTEM_PREFIX: &str = "__langfuse_";
static SYSTEM_VIEWS: LazyLock<Vec<Value>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("table-view-presets.json"))
        .expect("pinned system table views")
});

pub(super) fn request(store: &Store, project: &str, name: &str, input: &Value) -> Result<Value> {
    match name {
        "TableViewPresets.getByTableName" => {
            let table = table_name(input, "tableName")?;
            let mut views = store.resources(project, KIND)?;
            views.retain(|view| view["tableName"] == table);
            views.extend(
                SYSTEM_VIEWS
                    .iter()
                    .filter(|view| view["tableName"] == table)
                    .cloned(),
            );
            Ok(json!(views))
        }
        "TableViewPresets.getById" => {
            Ok(get(store, project, required(input, "viewId")?)?.unwrap_or(Value::Null))
        }
        "TableViewPresets.create" | "TableViewPresets.update" | "TableViewPresets.updateName" => {
            let table = table_name(input, "tableName")?;
            let view_name = required(input, "name")?.trim();
            if view_name.len() > 200 {
                bail!("View name is too long");
            }
            let id = if name == "TableViewPresets.create" {
                None
            } else {
                Some(required(input, "id")?)
            };
            if let Some(id) = id {
                let previous =
                    get(store, project, id)?.ok_or_else(|| fail("NOT_FOUND", "View not found"))?;
                if previous["isSystem"] == true {
                    return Err(fail("FORBIDDEN", "System views are read-only"));
                }
                if previous["tableName"] != table {
                    bail!("View belongs to another table");
                }
            }
            if store.resources(project, KIND)?.iter().any(|view| {
                view["tableName"] == table && view["name"] == view_name && view["id"].as_str() != id
            }) {
                return Err(fail("CONFLICT", "A view with this name already exists"));
            }
            let mut value = json!({"id":id,"name":view_name,"tableName":table,
                "createdBy":"confuse-local","createdByUser":{"name":"Local"}});
            if name != "TableViewPresets.updateName" {
                for key in [
                    "filters",
                    "columnOrder",
                    "columnVisibility",
                    "searchQuery",
                    "orderBy",
                ] {
                    value[key] = input[key].clone();
                }
                if !value["filters"].is_array()
                    || !value["columnOrder"].is_array()
                    || !value["columnVisibility"].is_object()
                {
                    bail!("Invalid saved view");
                }
            }
            Ok(json!({"success":true,"view":store.put_resource(project,KIND,value)?}))
        }
        "TableViewPresets.delete" => {
            let id = required(input, "tableViewPresetsId")?;
            if id.starts_with(SYSTEM_PREFIX) {
                return Ok(Value::Null);
            }
            if let Some(view) = get(store, project, id)? {
                let table = required(&view, "tableName")?;
                store.transaction(|tx| {
                    tx.execute(
                        "DELETE FROM resources WHERE project=?1 AND kind=?2 AND id=?3",
                        params![project, KIND, id],
                    )?;
                    for scope in ["user", "project"] {
                        tx.execute(
                            "DELETE FROM settings WHERE key=?1 AND value=?2",
                            params![default_key(project, table, scope), id],
                        )?;
                    }
                    Ok(())
                })?;
            }
            Ok(Value::Null)
        }
        "TableViewPresets.getDefault" | "TableViewPresets.getDefaultAssignments" => {
            let table = table_name(input, "viewName")?;
            let user = default(store, project, table, "user")?;
            let project = default(store, project, table, "project")?;
            if name.ends_with("Assignments") {
                Ok(json!({"userDefaultViewId":user,"projectDefaultViewId":project}))
            } else if let Some(user) = user {
                Ok(json!({"viewId":user,"scope":"user"}))
            } else {
                Ok(project
                    .map(|id| json!({"viewId":id,"scope":"project"}))
                    .unwrap_or(Value::Null))
            }
        }
        "TableViewPresets.setAsDefault" | "TableViewPresets.clearDefault" => {
            let scope = required(input, "scope")?;
            if !matches!(scope, "user" | "project") {
                bail!("Invalid default view scope");
            }
            if name == "TableViewPresets.setAsDefault" {
                let id = required(input, "viewId")?;
                let view =
                    get(store, project, id)?.ok_or_else(|| fail("NOT_FOUND", "View not found"))?;
                let table = table_name(&view, "tableName")?;
                if input["viewName"]
                    .as_str()
                    .is_some_and(|expected| expected != table)
                {
                    bail!("View belongs to another table");
                }
                store.access(|conn| {
                    conn.execute(
                        "INSERT OR REPLACE INTO settings(key,value) VALUES(?1,?2)",
                        params![default_key(project, table, scope), id],
                    )?;
                    Ok(())
                })?;
            } else {
                let table = table_name(input, "viewName")?;
                store.access(|conn| {
                    conn.execute(
                        "DELETE FROM settings WHERE key=?1",
                        [default_key(project, table, scope)],
                    )?;
                    Ok(())
                })?;
            }
            Ok(json!({"success":true}))
        }
        "TableViewPresets.generatePermalink" => {
            let id = required(input, "viewId")?;
            let table = table_name(input, "tableName")?;
            let view =
                get(store, project, id)?.ok_or_else(|| fail("NOT_FOUND", "View not found"))?;
            if view["tableName"] != table {
                bail!("View belongs to another table");
            }
            let route = match table {
                "observations-events" => "observations",
                "evaluators-v2" => "evals",
                "evaluation-rules-v2" => "evals/v2/rules",
                "monitors" => "alerts",
                "session-detail" | "experiment-items" => {
                    bail!("Open the parent item to use this saved view")
                }
                other => other,
            };
            let mut link =
                reqwest::Url::parse(&format!("confuse-ui://workspace/project/{project}/{route}"))?;
            link.query_pairs_mut().append_pair("viewId", id);
            Ok(json!(link.as_str()))
        }
        _ => Err(fail(
            "NOT_IMPLEMENTED",
            format!("Workspace procedure is unavailable: {name}"),
        )),
    }
}

fn table_name<'a>(input: &'a Value, field: &str) -> Result<&'a str> {
    let value = required(input, field)?;
    if !matches!(
        value,
        "traces"
            | "observations"
            | "observations-events"
            | "scores"
            | "sessions"
            | "session-detail"
            | "datasets"
            | "experiments"
            | "experiment-items"
            | "evaluators-v2"
            | "evaluation-rules-v2"
            | "users"
            | "prompts"
            | "monitors"
    ) {
        bail!("Unknown view table");
    }
    Ok(value)
}

fn get(store: &Store, project: &str, id: &str) -> Result<Option<Value>> {
    if id.starts_with(SYSTEM_PREFIX) {
        return Ok(SYSTEM_VIEWS.iter().find(|view| view["id"] == id).cloned());
    }
    Ok(store
        .resource(project, KIND, id)?
        .filter(|view| view["id"] == id))
}

fn default_key(project: &str, table: &str, scope: &str) -> String {
    format!("default-view:{project}:{table}:{scope}")
}

fn default(store: &Store, project: &str, table: &str, scope: &str) -> Result<Option<String>> {
    let id: Option<String> = store.access(|conn| {
        Ok(conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                [default_key(project, table, scope)],
                |row| row.get(0),
            )
            .optional()?)
    })?;
    if let Some(id) = id
        && get(store, project, &id)?.is_some_and(|view| view["tableName"] == table)
    {
        return Ok(Some(id));
    }
    Ok(None)
}
