use crate::model::{Observation, Record, Trace, TraceFilter, iso, now_ns, stable_id};
use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub const DEFAULT_PROJECT: &str = "local";

#[derive(Clone)]
pub struct Store(Arc<Mutex<Connection>>);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKey {
    pub id: String,
    pub public_key: String,
    pub secret_key: String,
    pub display_secret_key: String,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Self::initialize(conn)
    }

    pub fn memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(conn: Connection) -> Result<Self> {
        conn.execute_batch(include_str!("store.sql"))?;
        conn.execute_batch(include_str!("agents/store.sql"))?;
        conn.execute_batch(include_str!("resources.sql"))?;
        conn.execute_batch(include_str!("topics.sql"))?;
        conn.execute(
            "INSERT OR IGNORE INTO projects(id, name, created_ns) VALUES(?1, 'Local', ?2)",
            params![DEFAULT_PROJECT, now_ns()],
        )?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    pub(crate) fn access<T>(&self, action: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut conn = self
            .0
            .lock()
            .map_err(|_| anyhow!("database lock poisoned"))?;
        action(&mut conn)
    }

    pub(crate) fn transaction<T>(
        &self,
        action: impl FnOnce(&Transaction) -> Result<T>,
    ) -> Result<T> {
        self.access(|conn| {
            let transaction = conn.transaction()?;
            let result = action(&transaction)?;
            transaction.commit()?;
            Ok(result)
        })
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.access(|conn| {
            let mut stmt = conn.prepare("SELECT id, name FROM projects ORDER BY created_ns")?;
            Ok(stmt
                .query_map([], |row| {
                    Ok(Project {
                        id: row.get(0)?,
                        name: row.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn create_project(&self, name: &str) -> Result<Project> {
        let name = name.trim();
        if name.is_empty() {
            bail!("Enter a project name");
        }
        let project = Project {
            id: Uuid::new_v4().to_string(),
            name: name.to_owned(),
        };
        self.access(|conn| {
            conn.execute(
                "INSERT INTO projects VALUES(?1, ?2, ?3)",
                params![project.id, project.name, now_ns()],
            )?;
            Ok(())
        })?;
        Ok(project)
    }

    pub fn create_key(&self, project: &str, kind: &str) -> Result<ApiKey> {
        self.create_key_with_attributes(project, kind, None, None)
    }

    pub(crate) fn create_key_with_attributes(
        &self,
        project: &str,
        kind: &str,
        note: Option<&str>,
        expires_ns: Option<i64>,
    ) -> Result<ApiKey> {
        let id = Uuid::new_v4().to_string();
        let public_key = format!("pk-lf-{id}");
        let secret_key = format!(
            "sk-lf-{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let display_secret_key = format!("sk-lf-…{}", &secret_key[secret_key.len() - 4..]);
        let digest = Sha256::digest(secret_key.as_bytes());
        self.transaction(|conn| {
            conn.execute(
                "INSERT INTO keys(id, project, kind, public, digest, display, created_ns)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    project,
                    kind,
                    public_key,
                    &digest[..],
                    display_secret_key,
                    now_ns()
                ],
            )?;
            if note.is_some() || expires_ns.is_some() {
                conn.execute(
                    "INSERT INTO key_attributes(id,note,expires_ns) VALUES(?1,?2,?3)",
                    params![id, note, expires_ns],
                )?;
            }
            Ok(())
        })?;
        Ok(ApiKey {
            id,
            public_key,
            secret_key,
            display_secret_key,
        })
    }

    pub fn keys(&self, project: &str) -> Result<Vec<Value>> {
        self.access(|conn| {
            let mut stmt = conn.prepare(
                "SELECT keys.id, public, display, kind, created_ns, note, expires_ns
                FROM keys LEFT JOIN key_attributes ON keys.id=key_attributes.id
                WHERE project=?1 ORDER BY created_ns",
            )?;
            Ok(stmt.query_map([project], |row| Ok(json!({
                "id": row.get::<_,String>(0)?, "publicKey": row.get::<_,String>(1)?,
                "displaySecretKey": row.get::<_,String>(2)?, "kind": row.get::<_,String>(3)?,
                "createdAt": iso(row.get(4)?),
                "note": row.get::<_,Option<String>>(5)?,
                "expiresAt": row.get::<_,Option<i64>>(6)?.map(iso),
            })))?.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn revoke_key(&self, project: &str, id: &str) -> Result<bool> {
        self.access(|conn| {
            Ok(conn.execute(
                "DELETE FROM keys WHERE project=?1 AND id=?2",
                params![project, id],
            )? > 0)
        })
    }

    pub(crate) fn set_key_note(&self, project: &str, id: &str, note: &str) -> Result<bool> {
        self.access(|conn| {
            Ok(conn.execute(
                "INSERT INTO key_attributes(id,note)
                 SELECT id,?3 FROM keys WHERE project=?1 AND id=?2 AND kind='sdk'
                 ON CONFLICT(id) DO UPDATE SET note=excluded.note",
                params![project, id, note],
            )? > 0)
        })
    }

    pub fn project_for_key(&self, public: &str, secret: &str) -> Result<Option<String>> {
        let digest = Sha256::digest(secret.as_bytes());
        self.access(|conn| {
            let key: Option<(String, Vec<u8>)> = conn
                .query_row(
                    "SELECT project, digest FROM keys LEFT JOIN key_attributes ON keys.id=key_attributes.id
                     WHERE public=?1 AND kind='sdk' AND (expires_ns IS NULL OR expires_ns>?2)",
                    params![public, now_ns()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            Ok(key.and_then(|(project, expected)| {
                bool::from(expected.as_slice().ct_eq(&digest[..])).then_some(project)
            }))
        })
    }

    pub fn project_for_gateway_key(&self, secret: &str) -> Result<Option<String>> {
        let digest = Sha256::digest(secret.as_bytes());
        self.access(|conn| {
            Ok(conn
                .query_row(
                    "SELECT project FROM keys LEFT JOIN key_attributes ON keys.id=key_attributes.id
                     WHERE digest=?1 AND kind='gateway' AND (expires_ns IS NULL OR expires_ns>?2)",
                    params![&digest[..], now_ns()],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    pub fn ingest(&self, project: &str, records: &[Record]) -> Result<()> {
        self.transaction(|tx| Self::write_records(tx, project, records))
    }

    pub(crate) fn write_records(tx: &Transaction, project: &str, records: &[Record]) -> Result<()> {
        for record in records {
            let mut trace = record.trace.clone();
            let old: Option<String> = tx
                .query_row(
                    "SELECT data FROM traces WHERE project=?1 AND id=?2",
                    params![project, trace.id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(old) = old {
                let existing: Trace = decode(&old)?;
                if existing.start_ns > 0
                    && (trace.start_ns == 0 || existing.start_ns < trace.start_ns)
                {
                    trace.start_ns = existing.start_ns;
                }
                trace.end_ns = trace.end_ns.max(existing.end_ns);
                trace.received_ns = trace.received_ns.max(existing.received_ns);
                if !record.root && !existing.name.is_empty() {
                    trace.name = existing.name;
                }
                trace.session_id = trace.session_id.or(existing.session_id);
                trace.user_id = trace.user_id.or(existing.user_id);
                trace.version = trace.version.or(existing.version);
                trace.release = trace.release.or(existing.release);
                if trace.environment == "default" {
                    trace.environment = existing.environment;
                }
                if trace.input.is_null() || (!record.root && !existing.input.is_null()) {
                    trace.input = existing.input;
                }
                if trace.output.is_null() || (!record.root && !existing.output.is_null()) {
                    trace.output = existing.output;
                }
                if trace.metadata.is_null() || (!record.root && !existing.metadata.is_null()) {
                    trace.metadata = existing.metadata;
                }
                if trace.tags.is_empty() {
                    trace.tags = existing.tags;
                }
            }
            tx.execute(
                "INSERT INTO traces(project, id, session, name, user_id, source, start_ns, received_ns, data)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(project,id) DO UPDATE SET session=excluded.session,name=excluded.name,
                 user_id=excluded.user_id,source=excluded.source,start_ns=excluded.start_ns,
                 received_ns=excluded.received_ns,data=excluded.data",
                params![project,trace.id,trace.session_id,trace.name,trace.user_id,trace.source,
                    trace.start_ns,trace.received_ns,serde_json::to_string(&trace)?],
            )?;
            if let Some(obs) = &record.observation {
                tx.execute(
                    "INSERT INTO observations(project, trace, id, parent, kind, start_ns, data)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)
                     ON CONFLICT(project,trace,id) DO UPDATE SET parent=excluded.parent,kind=excluded.kind,
                     start_ns=excluded.start_ns,data=excluded.data",
                    params![project,trace.id,obs.id,obs.parent_id,obs.kind,obs.start_ns,serde_json::to_string(obs)?],
                )?;
            }
        }
        Ok(())
    }

    pub fn trace(&self, project: &str, id: &str) -> Result<Option<Trace>> {
        self.access(|conn| {
            let data: Option<String> = conn
                .query_row(
                    "SELECT data FROM traces WHERE project=?1 AND id=?2",
                    params![project, id],
                    |row| row.get(0),
                )
                .optional()?;
            data.map(|data| decode(&data)).transpose()
        })
    }

    pub fn traces(
        &self,
        project: &str,
        filter: &TraceFilter,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<Trace>, usize)> {
        self.access(|conn| {
            const WHERE: &str = "FROM traces WHERE project=?1
                AND (?2 IS NULL OR instr(lower(data),lower(?2))>0)
                AND (?3 IS NULL OR session=?3) AND (?4 IS NULL OR name=?4)
                AND (?5 IS NULL OR user_id=?5) AND (?6 IS NULL OR source=?6)
                AND (?7 IS NULL OR start_ns>=?7) AND (?8 IS NULL OR start_ns<?8)
                AND (?9 IS NULL OR EXISTS(SELECT 1 FROM topic_assignments AS ta WHERE ta.project=traces.project AND ta.trace=traces.id AND ta.topic=?9))";
            let params = params![
                project,
                filter.query,
                filter.session,
                filter.name,
                filter.user,
                filter.source,
                filter.from,
                filter.to,
                filter.topic
            ];
            let total: i64 =
                conn.query_row(&format!("SELECT COUNT(*) {WHERE}"), params, |row| {
                    row.get::<_, i64>(0)
                })?;
            let mut stmt = conn.prepare(&format!(
                "SELECT data {WHERE} ORDER BY start_ns DESC,id LIMIT ?10 OFFSET ?11"
            ))?;
            let traces = stmt
                .query_map(
                    params![
                        project,
                        filter.query,
                        filter.session,
                        filter.name,
                        filter.user,
                        filter.source,
                        filter.from,
                        filter.to,
                        filter.topic,
                        i64::try_from(limit)?,
                        i64::try_from(offset)?
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .map(|r| decode(&r?))
                .collect::<Result<Vec<_>>>()?;
            Ok((traces, usize::try_from(total)?))
        })
    }

    pub fn observations(
        &self,
        project: &str,
        trace: Option<&str>,
        kind: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<Observation>, usize)> {
        self.access(|conn| {
            const WHERE: &str = "FROM observations WHERE project=?1 AND (?2 IS NULL OR trace=?2) AND (?3 IS NULL OR kind=?3)";
            let count=conn.query_row(&format!("SELECT COUNT(*) {WHERE}"),params![project,trace,kind],|r|r.get::<_,i64>(0))?;
            let mut stmt = conn.prepare(&format!("SELECT data {WHERE} ORDER BY start_ns,id LIMIT ?4 OFFSET ?5"))?;
            let observations = stmt.query_map(params![project,trace,kind,i64::try_from(limit)?,i64::try_from(offset)?], |row|row.get::<_,String>(0))?
                .map(|r| decode(&r?)).collect::<Result<Vec<_>>>()?;
            Ok((observations,usize::try_from(count)?))
        })
    }

    pub fn observation(&self, project: &str, id: &str) -> Result<Option<Observation>> {
        self.access(|conn| {
            let data: Option<String> = conn
                .query_row(
                    "SELECT data FROM observations WHERE project=?1 AND id=?2",
                    params![project, id],
                    |r| r.get(0),
                )
                .optional()?;
            data.map(|data| decode(&data)).transpose()
        })
    }

    pub fn delete_trace(&self, project: &str, id: &str) -> Result<bool> {
        self.access(|conn| {
            Ok(conn.execute(
                "DELETE FROM traces WHERE project=?1 AND id=?2",
                params![project, id],
            )? > 0)
        })
    }

    pub fn sessions(
        &self,
        project: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<Value>, usize)> {
        self.access(|conn| {
            let total = conn.query_row("SELECT COUNT(DISTINCT session) FROM traces WHERE project=?1", [project], |row|row.get::<_,i64>(0))?;
            let mut stmt = conn.prepare("SELECT session, MIN(start_ns), MAX(start_ns), COUNT(*), MIN(source) FROM traces
                WHERE project=?1 AND session IS NOT NULL GROUP BY session ORDER BY MAX(start_ns) DESC,session LIMIT ?2 OFFSET ?3")?;
            let rows = stmt.query_map(params![project,i64::try_from(limit)?,i64::try_from(offset)?], |row| Ok(json!({
                "id":row.get::<_,String>(0)?, "createdAt":iso(row.get(1)?), "updatedAt":iso(row.get(2)?),
                "traceCount":row.get::<_,i64>(3)?, "source":row.get::<_,String>(4)?, "projectId":project,
                "environment":"default", "bookmarked":false, "public":false
            })))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((rows,usize::try_from(total)?))
        })
    }

    pub fn put_score(&self, project: &str, mut data: Value) -> Result<Value> {
        let name = data
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .context("score requires name")?;
        let value = data
            .get("value")
            .filter(|value| value.is_number() || value.is_string() || value.is_boolean())
            .context("score requires a score value")?
            .clone();
        let trace = data
            .get("traceId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let id = data
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let kind = data
            .get("dataType")
            .and_then(Value::as_str)
            .unwrap_or(if value.is_number() {
                "NUMERIC"
            } else if value.is_boolean() {
                "BOOLEAN"
            } else {
                "CATEGORICAL"
            })
            .to_owned();
        let name = name.to_owned();
        let now = now_ns();
        data["id"] = json!(id);
        data["dataType"] = json!(kind);
        data["source"] = json!(data.get("source").and_then(Value::as_str).unwrap_or("API"));
        data["timestamp"] = json!(iso(now));
        data["createdAt"] = json!(iso(now));
        data["updatedAt"] = json!(iso(now));
        if (kind == "CATEGORICAL" || kind == "TEXT") && data["stringValue"].is_null() {
            data["stringValue"] = value;
        }
        self.access(|conn| {
            conn.execute("INSERT INTO scores(project,id,trace,name,created_ns,data) VALUES(?1,?2,?3,?4,?5,?6)
                ON CONFLICT(project,id) DO UPDATE SET trace=excluded.trace,name=excluded.name,data=excluded.data",
                params![project,id,trace,name,now,serde_json::to_string(&data)?])?;
            Ok(data)
        })
    }

    pub fn scores(
        &self,
        project: &str,
        trace: Option<&str>,
        name: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<Value>, usize)> {
        self.access(|conn| {
            const WHERE: &str = "FROM scores WHERE project=?1 AND (?2 IS NULL OR trace=?2) AND (?3 IS NULL OR name=?3)";
            let count = conn.query_row(&format!("SELECT COUNT(*) {WHERE}"), params![project,trace,name],|r|r.get::<_,i64>(0))?;
            let mut stmt=conn.prepare(&format!("SELECT data {WHERE} ORDER BY created_ns DESC,id LIMIT ?4 OFFSET ?5"))?;
            let data=stmt.query_map(params![project,trace,name,i64::try_from(limit)?,i64::try_from(offset)?],|r|r.get::<_,String>(0))?.map(|r|decode(&r?)).collect::<Result<Vec<_>>>()?;
            Ok((data,usize::try_from(count)?))
        })
    }

    pub fn score(&self, project: &str, id: &str) -> Result<Option<Value>> {
        self.access(|c| {
            let data: Option<String> = c
                .query_row(
                    "SELECT data FROM scores WHERE project=?1 AND id=?2",
                    params![project, id],
                    |r| r.get(0),
                )
                .optional()?;
            data.map(|data| decode(&data)).transpose()
        })
    }

    pub fn delete_score(&self, project: &str, id: &str) -> Result<bool> {
        self.access(|c| {
            Ok(c.execute(
                "DELETE FROM scores WHERE project=?1 AND id=?2",
                params![project, id],
            )? > 0)
        })
    }

    pub fn existing_observation_id(
        &self,
        project: &str,
        trace: &str,
        observation: &str,
    ) -> Result<bool> {
        self.access(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM observations WHERE project=?1 AND trace=?2 AND id=?3)",
                params![project, trace, observation],
                |r| r.get(0),
            )?)
        })
    }

    pub fn endpoint_port(&self) -> Result<u16> {
        self.access(|c| {
            let port: Option<u16> = c
                .query_row("SELECT value FROM settings WHERE key='port'", [], |r| {
                    r.get(0)
                })
                .optional()?;
            Ok(port.unwrap_or(32127))
        })
    }

    pub fn set_endpoint_port(&self, port: u16) -> Result<()> {
        if port == 0 {
            bail!("The port must be between 1 and 65535")
        }
        self.access(|c| {
            c.execute("INSERT INTO settings(key,value) VALUES('port',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [port])?;
            Ok(())
        })
    }
}

pub(crate) fn decode<T: DeserializeOwned>(data: &str) -> Result<T> {
    Ok(serde_json::from_str(data)?)
}

pub fn correlation_id(source: &str, session: &str, event: &str) -> String {
    stable_id(&[source, session, event], 16)
}
