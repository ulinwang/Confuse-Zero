use crate::{
    model::{iso, now_ns, stable_id},
    resources::string_list,
    store::{Store, decode},
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::params;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

pub fn normalize(store: &Store, project: &str, value: &mut Value) -> Result<()> {
    let existing = store.resources(project, "skills")?;
    let contents = blob_contents(&existing);
    let mut inputs = value["files"].as_array().cloned().unwrap_or_default();
    if let Some(body) = value["content"].as_str() {
        inputs.retain(|file| file["path"] != "SKILL.md");
        inputs.push(json!({"path":"SKILL.md","content":body}));
    }
    if inputs.is_empty() || inputs.len() > 100 {
        bail!("Provide SKILL.md and up to 99 supporting files");
    }
    let mut files = Vec::with_capacity(inputs.len());
    let mut paths = BTreeSet::new();
    let mut bytes = 0;
    for file in inputs {
        let path = file["path"]
            .as_str()
            .filter(|path| {
                !path.is_empty()
                    && path.len() <= 1024
                    && !path.contains('\\')
                    && Path::new(path)
                        .components()
                        .all(|part| matches!(part, Component::Normal(_)))
            })
            .context("Skill files require relative paths")?;
        if !paths.insert(path.to_owned()) {
            bail!("Duplicate skill file path");
        }
        let inline = file["content"].as_str();
        let reference = file["sha256Hash"].as_str();
        let text = match (inline, reference) {
            (Some(content), None) => content,
            (None, Some(hash)) => contents
                .get(hash)
                .map(String::as_str)
                .context("Skill file hash is unavailable in this project")?,
            _ => bail!("Provide exactly one content or sha256Hash per file"),
        };
        bytes += text.len();
        if bytes > 4 * 1024 * 1024 {
            bail!("Skill exceeds the 4 MiB total size limit");
        }
        let hash = STANDARD.encode(Sha256::digest(text.as_bytes()));
        files.push(json!({
            "id":stable_id(&[project,path,&hash],16),"blobId":stable_id(&[project,&hash],16),
            "path":path,"sha256Hash":hash, "contentType":"text/plain; charset=utf-8",
            "contentLength":text.len(),"content":text,
        }));
    }
    let main = files
        .iter()
        .find(|file| file["path"] == "SKILL.md")
        .context("Skill requires SKILL.md")?;
    let text = main["content"].as_str().unwrap_or_default();
    let frontmatter = if let Some(after) = text.strip_prefix("---\n") {
        let (body, _) = after
            .split_once("\n---")
            .context("Unclosed SKILL.md frontmatter")?;
        serde_yaml::from_str::<Value>(body).context("Invalid SKILL.md frontmatter")?
    } else {
        json!({})
    };
    let name = frontmatter["name"]
        .as_str()
        .or_else(|| value["name"].as_str())
        .filter(|name| !name.trim().is_empty())
        .context("SKILL.md frontmatter requires a name")?
        .to_owned();
    let description = frontmatter["description"]
        .as_str()
        .or_else(|| value["description"].as_str())
        .unwrap_or_default()
        .to_owned();
    let tags = existing
        .iter()
        .find(|row| row["name"] == name)
        .map(|row| string_list(&row["tags"]))
        .unwrap_or_default();
    value["content"] = json!(text);
    value["files"] = json!(files);
    value["version"] = Value::Null;
    value["name"] = json!(name);
    value["description"] = json!(description);
    value["frontmatter"] = frontmatter;
    value["createdBy"] = json!("local");
    value["tags"] = json!(tags);
    Ok(())
}

pub fn manifest(mut data: Value, content: bool) -> Value {
    if !content {
        if let Some(entry) = data.as_object_mut() {
            entry.remove("content");
        }
        if let Some(files) = data["files"].as_array_mut() {
            for file in files {
                if let Some(entry) = file.as_object_mut() {
                    entry.remove("content");
                }
            }
        }
    }
    data
}

pub fn versions(store: &Store, project: &str) -> Result<Vec<Value>> {
    let versions = store.resources(project, "skills")?;
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    for row in versions {
        let name = row["name"].as_str().unwrap_or_default();
        let entry = grouped.entry(name.to_owned()).or_insert_with(|| {
            json!({
                "name":name,"description":row["description"],"tags":row["tags"],
                "latestVersion":row["version"],"latestVersionCreatedAt":row["createdAt"],
                "latestVersionLastUpdatedAt":row["updatedAt"],"productionVersion":null
            })
        });
        if string_list(&row["labels"])
            .iter()
            .any(|label| label == "production")
        {
            entry["productionVersion"] = row["version"].clone();
        }
    }
    Ok(grouped.into_values().collect())
}

pub fn contents(store: &Store, project: &str, requested: &str) -> Result<Vec<Value>> {
    let hashes = requested.split(',').collect::<Vec<_>>();
    if hashes.is_empty()
        || hashes.len() > 50
        || hashes.iter().any(|hash| {
            !STANDARD
                .decode(hash)
                .is_ok_and(|bytes| bytes.len() == 32 && STANDARD.encode(bytes) == *hash)
        })
    {
        bail!("Provide one to 50 canonical base64 SHA-256 hashes");
    }
    let blobs = blob_contents(&store.resources(project, "skills")?);
    let mut results = Vec::new();
    for hash in hashes {
        if let Some(content) = blobs.get(hash) {
            results.push(json!({"sha256Hash":hash,"content":content}));
        }
    }
    Ok(results)
}

pub fn update_tags(store: &Store, project: &str, name: &str, tags: Vec<String>) -> Result<Value> {
    store.transaction(|tx|{
        let mut query=tx.prepare("SELECT id,data FROM resources WHERE project=?1 AND kind='skills' AND name=?2 ORDER BY created_ns DESC")?;
        let rows=query.query_map(params![project,name],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut latest=None;
        let updated=iso(now_ns());
        for (id,raw) in rows {
            let mut data:Value=decode(&raw)?;
            data["tags"]=json!(tags);
            data["updatedAt"]=json!(updated);
            tx.execute("UPDATE resources SET data=?3 WHERE project=?1 AND kind='skills' AND id=?2",
                params![project,id,data.to_string()])?;
            if latest.is_none(){latest=Some(data);}
        }
        latest.context("Skill not found")
    })
}

fn blob_contents(versions: &[Value]) -> BTreeMap<String, String> {
    versions
        .iter()
        .filter_map(|row| row["files"].as_array())
        .flatten()
        .filter_map(|file| {
            Some((
                file["sha256Hash"].as_str()?.to_owned(),
                file["content"].as_str()?.to_owned(),
            ))
        })
        .collect()
}
