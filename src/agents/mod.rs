mod discover;
mod normalize;

pub use discover::{Discovery, Installation};

use crate::{
    model::{Record, iso, now_ns},
    store::{Store, decode},
};
use anyhow::{Context, Result, bail};
use normalize::Session;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

const MAX_LINE: usize = 8 * 1024 * 1024;
const ROWS_PER_TRANSACTION: usize = 256;
const MAX_DEPTH: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Agent {
    KimiCode,
    Codex,
    ClaudeCode,
    #[serde(rename = "deepseek-harness")]
    DeepSeekHarness,
    Pi,
}

impl Agent {
    pub const ALL: [Self; 5] = [
        Self::KimiCode,
        Self::Codex,
        Self::ClaudeCode,
        Self::DeepSeekHarness,
        Self::Pi,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::KimiCode => "kimi-code",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::DeepSeekHarness => "deepseek-harness",
            Self::Pi => "pi",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::KimiCode => "Kimi Code",
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
            Self::DeepSeekHarness => "DeepSeek Harness",
            Self::Pi => "Pi",
        }
    }

    fn directories(self) -> &'static [&'static str] {
        match self {
            Self::KimiCode | Self::DeepSeekHarness => &["sessions"],
            Self::Codex => &["sessions", "archived_sessions"],
            Self::ClaudeCode => &["projects"],
            Self::Pi => &[],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub agent: Agent,
    pub root: PathBuf,
    pub history: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub selection: Selection,
    pub initial_ns: i64,
    pub pending: bool,
    pub active: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Cursor {
    lines: u64,
    bytes: u64,
    device: u64,
    inode: u64,
    snapshot_len: u64,
    snapshot_modified: i64,
    compressed: bool,
    caught_up: bool,
    session: Session,
}

#[derive(Default)]
struct Counts {
    files: usize,
    records: usize,
    skipped: usize,
}

struct Candidate {
    path: PathBuf,
    key: String,
    version: u32,
    compressed: bool,
}

pub struct Collector {
    store: Store,
}

impl Collector {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn select(&self, project: &str, selections: &[Selection]) -> Result<()> {
        let now = now_ns();
        self.store.transaction(|tx| {
            for selection in selections {
                if !selection.root.is_absolute() {bail!("Choose an absolute Agent directory");}
                let selection=Selection{root:selection.root.canonicalize().unwrap_or_else(|_|selection.root.clone()),..selection.clone()};
                let root=selection.root.to_str().context("Agent directory must be UTF-8")?;
                tx.execute("INSERT INTO agent_sources(project,agent,root,selection,initial_ns,pending,active)
                    VALUES(?1,?2,?3,?4,?5,1,1) ON CONFLICT(project,agent,root)
                    DO UPDATE SET selection=excluded.selection,pending=1,active=1",
                    params![project,selection.agent.id(),root,serde_json::to_string(&selection)?,now])?;
                tx.execute("UPDATE agent_sources SET active=0 WHERE project=?1 AND agent=?2 AND root<>?3",
                    params![project,selection.agent.id(),root])?;
            }
            Ok(())
        })
    }

    pub fn sources(&self, project: &str) -> Result<Vec<Source>> {
        self.store.access(|conn|{
            let mut stmt=conn.prepare("SELECT selection,initial_ns,pending,active FROM agent_sources WHERE project=?1 ORDER BY agent,root")?;
            let data=stmt.query_map([project],|row|Ok((row.get::<_,String>(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
            data.map(|row| {
                let (selection,initial_ns,pending,active)=row?;
                Ok(Source{selection:decode(&selection)?,initial_ns,pending,active})
            }).collect()
        })
    }

    pub fn disable(&self, project: &str, agent: Agent) -> Result<()> {
        self.store.access(|conn| {
            conn.execute(
                "UPDATE agent_sources SET active=0 WHERE project=?1 AND agent=?2",
                params![project, agent.id()],
            )?;
            Ok(())
        })
    }

    pub fn update(&self, project: &str) -> Result<()> {
        for source in self.sources(project)?.into_iter().filter(|s| s.active) {
            let result = self.collect_source(project, &source);
            if source.pending {
                self.finish_initial(project, &source, result)?;
            } else if result.is_err() || result.as_ref().is_ok_and(|counts| counts.skipped > 0) {
                self.store.notify(
                    project,
                    source.selection.agent.label(),
                    "Some Agent records could not be read",
                    "warning",
                    Some(source.selection.agent.id()),
                    false,
                )?;
            }
        }
        Ok(())
    }

    fn collect_source(&self, project: &str, source: &Source) -> Result<Counts> {
        let root = &source.selection.root;
        fs::read_dir(root).context("Session directory is unavailable")?;
        let candidates = find_sessions(source.selection.agent, root)?;
        let mut counts = Counts {
            files: candidates.len(),
            ..Default::default()
        };
        for candidate in candidates {
            if self
                .collect_file(project, source, &candidate, &mut counts)
                .is_err()
            {
                counts.skipped += 1;
            }
        }
        Ok(counts)
    }

    fn finish_initial(&self, project: &str, source: &Source, result: Result<Counts>) -> Result<()> {
        let agent = source.selection.agent;
        let (message, level) = match result {
            Ok(counts) if counts.skipped == 0 => (
                format!(
                    "Connected · {} files · {} records imported",
                    counts.files, counts.records
                ),
                "success",
            ),
            Ok(counts) => (
                format!(
                    "Connected · {} records imported · {} skipped",
                    counts.records, counts.skipped
                ),
                "warning",
            ),
            Err(_) => (
                "Session directory unavailable. Check access and retry.".to_owned(),
                "error",
            ),
        };
        self.store.transaction(|tx|{
            let changed=tx.execute("UPDATE agent_sources SET pending=0 WHERE project=?1 AND agent=?2 AND root=?3 AND active=1 AND pending=1",
                params![project,agent.id(),source.selection.root.to_string_lossy()])?;
            if changed>0 {
                Store::write_notification(tx,project,agent.label(),&message,level,Some(agent.id()),true)?;
            }
            Ok(())
        })
    }

    fn collect_file(
        &self,
        project: &str,
        source: &Source,
        file: &Candidate,
        counts: &mut Counts,
    ) -> Result<()> {
        let agent = source.selection.agent;
        let root = source.selection.root.to_string_lossy();
        let canonical = file.path.canonicalize()?;
        if !canonical.starts_with(source.selection.root.canonicalize()?) {
            bail!("Session path escaped its selected directory");
        }
        let handle = open_regular(&file.path)?;
        let metadata = handle.metadata()?;
        let len = metadata.len();
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|time| i64::try_from(time.as_nanos()).ok())
            .unwrap_or_default();
        let persistent:Option<String>=self.store.access(|conn|Ok(conn.query_row(
            "SELECT data FROM agent_cursors WHERE project=?1 AND agent=?2 AND root=?3 AND stream=?4",
            params![project,agent.id(),root,file.key],|row|row.get(0)
        ).optional()?))?;
        let mut cursor: Cursor = persistent
            .as_deref()
            .map(decode)
            .transpose()?
            .unwrap_or_default();
        let same_inode = cursor.device == metadata.dev() && cursor.inode == metadata.ino();
        if same_inode
            && cursor.caught_up
            && cursor.snapshot_len == len
            && cursor.snapshot_modified == modified
        {
            return Ok(());
        }
        let capture_baseline =
            persistent.is_none() && !source.selection.history && modified <= source.initial_ns;
        let mut handle = handle;
        let seekable = same_inode && !file.compressed && !cursor.compressed && cursor.bytes <= len;
        let mut reader: Box<dyn BufRead> = if file.compressed {
            Box::new(BufReader::new(zstd::stream::read::Decoder::new(handle)?))
        } else {
            if seekable {
                handle.seek(SeekFrom::Start(cursor.bytes))?;
            }
            Box::new(BufReader::new(handle))
        };
        let mut observed_lines = if seekable { cursor.lines } else { 0 };
        let mut observed_bytes = if seekable { cursor.bytes } else { 0 };
        let prev_line = cursor.lines;
        let mut position_is_prefix = !seekable && persistent.is_some();
        if !seekable && !file.compressed && same_inode && cursor.bytes > len {
            cursor = Cursor::default();
            position_is_prefix = false;
        }
        cursor.device = metadata.dev();
        cursor.inode = metadata.ino();
        cursor.compressed = file.compressed;
        cursor.snapshot_len = len;
        cursor.snapshot_modified = modified;
        cursor.caught_up = false;
        let mut records = Vec::new();
        let mut line = Vec::new();
        let mut processed = 0;
        loop {
            line.clear();
            let amount = reader
                .by_ref()
                .take(MAX_LINE as u64)
                .read_until(b'\n', &mut line);
            let amount = match amount {
                Ok(amount) => amount,
                Err(error)
                    if file.compressed
                        && matches!(
                            error.kind(),
                            io::ErrorKind::UnexpectedEof
                                | io::ErrorKind::InvalidData
                                | io::ErrorKind::Other
                        ) =>
                {
                    counts.skipped += 1;
                    cursor.caught_up = true;
                    break;
                }
                Err(error) => return Err(error.into()),
            };
            if amount == 0 {
                cursor.caught_up = true;
                break;
            }
            if !line.ends_with(b"\n") {
                if amount == MAX_LINE {
                    let remaining = reader.skip_until(b'\n')?;
                    observed_bytes = observed_bytes.saturating_add((remaining + amount) as u64);
                    observed_lines += 1;
                    counts.skipped += 1;
                    continue;
                }
                break;
            }
            observed_lines += 1;
            observed_bytes += u64::try_from(amount)?;
            if position_is_prefix && observed_lines <= prev_line {
                continue;
            }
            match serde_json::from_slice::<Value>(&line) {
                Ok(event) => {
                    let old_event = persistent.is_none()
                        && !source.selection.history
                        && event
                            .get("timestamp")
                            .or_else(|| event.get("time"))
                            .and_then(crate::model::parse_time)
                            .is_some_and(|time| time < source.initial_ns);
                    if capture_baseline || old_event {
                        cursor.session.learn_header(&event);
                    } else if let Some(record) =
                        cursor
                            .session
                            .normalize(agent, &event, observed_lines, &file.key, now_ns())
                    {
                        records.push(record);
                    }
                }
                Err(_) => counts.skipped += 1,
            }
            processed += 1;
            if processed >= ROWS_PER_TRANSACTION && !capture_baseline {
                cursor.lines = observed_lines;
                cursor.bytes = observed_bytes;
                self.commit(project, source, file, &cursor, &records)?;
                counts.records += records.len();
                records.clear();
                processed = 0;
            }
        }
        cursor.lines = cursor.lines.max(observed_lines);
        cursor.bytes = observed_bytes;
        if position_is_prefix && observed_lines < prev_line && !file.compressed {
            // A rewrite with fewer complete rows is ambiguous. Retain the logical watermark.
            counts.skipped += 1;
        }
        counts.records += records.len();
        self.commit(project, source, file, &cursor, &records)?;
        Ok(())
    }

    fn commit(
        &self,
        project: &str,
        source: &Source,
        file: &Candidate,
        cursor: &Cursor,
        records: &[Record],
    ) -> Result<()> {
        let agent = source.selection.agent.id();
        let root = source.selection.root.to_string_lossy();
        self.store.transaction(|tx| {
            let active = tx
                .query_row(
                    "SELECT active FROM agent_sources WHERE project=?1 AND agent=?2 AND root=?3",
                    params![project, agent, root],
                    |row| row.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false);
            if !active {
                return Ok(());
            }
            Store::write_records(tx, project, records)?;
            tx.execute(
                "INSERT INTO agent_cursors(project,agent,root,stream,data) VALUES(?1,?2,?3,?4,?5)
                ON CONFLICT(project,agent,root,stream) DO UPDATE SET data=excluded.data",
                params![
                    project,
                    agent,
                    root,
                    file.key,
                    serde_json::to_string(cursor)?
                ],
            )?;
            Ok(())
        })
    }
}

fn find_sessions(agent: Agent, root: &Path) -> Result<Vec<Candidate>> {
    let configured = agent.directories();
    let mut files = Vec::new();
    if configured.is_empty() {
        visit(root, 0, &mut files)?;
    } else {
        let mut found = false;
        for name in configured {
            let directory = root.join(name);
            if directory.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
                visit(&directory, 0, &mut files)?;
                found = true;
            }
        }
        if !found {
            visit(root, 0, &mut files)?;
        }
    }
    let mut candidates: BTreeMap<String, Candidate> = BTreeMap::new();
    for path in files {
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let compressed = filename.ends_with(".zst") || filename.ends_with(".zstd");
        let name = filename.trim_end_matches(".zst").trim_end_matches(".zstd");
        if !name.ends_with(".jsonl") {
            continue;
        }
        let stem = name.trim_end_matches(".jsonl");
        let (key, version) = match agent {
            Agent::KimiCode if matches!(stem, "wire" | "context") => (
                relative
                    .with_file_name("wire")
                    .to_string_lossy()
                    .into_owned(),
                u32::from(stem == "wire"),
            ),
            Agent::Codex if stem.starts_with("rollout-") => (stem.to_owned(), 0),
            Agent::ClaudeCode
                if !relative
                    .components()
                    .any(|p| p.as_os_str() == "tool-results") =>
            {
                (
                    relative.with_file_name(stem).to_string_lossy().into_owned(),
                    0,
                )
            }
            Agent::DeepSeekHarness if stem == "session" || stem.starts_with("session.v") => (
                relative
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                stem.strip_prefix("session.v")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default(),
            ),
            Agent::Pi => (
                relative.with_file_name(stem).to_string_lossy().into_owned(),
                0,
            ),
            _ => continue,
        };
        if candidates
            .get(&key)
            .is_some_and(|old| (old.version, !old.compressed) >= (version, !compressed))
        {
            continue;
        }
        let candidate = Candidate {
            path,
            key: key.clone(),
            version,
            compressed,
        };
        candidates.insert(key, candidate);
    }
    Ok(candidates.into_values().collect())
}

fn visit(root: &Path, depth: usize, files: &mut Vec<PathBuf>) -> Result<()> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            visit(&entry.path(), depth + 1, files)?;
        } else if file_type.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn open_regular(path: &Path) -> Result<File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        bail!("Not a regular session file");
    }
    Ok(file)
}

impl Store {
    pub(crate) fn write_notification(
        conn: &rusqlite::Connection,
        project: &str,
        title: &str,
        message: &str,
        level: &str,
        source: Option<&str>,
        repeat: bool,
    ) -> Result<()> {
        let day = now_ns() / 86_400_000_000_000;
        let fingerprint =
            (!repeat).then(|| format!("{}:{title}:{message}:{day}", source.unwrap_or("local")));
        conn.execute("INSERT OR IGNORE INTO notifications(project,title,message,level,source,created_ns,fingerprint)
            VALUES(?1,?2,?3,?4,?5,?6,?7)",params![project,title,message,level,source,now_ns(),fingerprint])?;
        Ok(())
    }

    pub fn notify(
        &self,
        project: &str,
        title: &str,
        message: &str,
        level: &str,
        source: Option<&str>,
        repeat: bool,
    ) -> Result<()> {
        self.access(|conn| {
            Self::write_notification(conn, project, title, message, level, source, repeat)
        })
    }

    pub fn notifications(&self, project: &str) -> Result<Vec<Value>> {
        self.access(|conn|{
            let mut stmt=conn.prepare("SELECT id,title,message,level,source,created_ns,read FROM notifications WHERE project=?1 ORDER BY id DESC LIMIT 100")?;
            Ok(stmt.query_map([project],|row|Ok(json!({
                "id":row.get::<_,i64>(0)?,"title":row.get::<_,String>(1)?,"message":row.get::<_,String>(2)?,
                "level":row.get::<_,String>(3)?,"source":row.get::<_,Option<String>>(4)?,"createdAt":iso(row.get(5)?),
                "read":row.get::<_,bool>(6)?
            })))?.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn mark_notifications_read(&self, project: &str) -> Result<()> {
        self.access(|conn| {
            conn.execute(
                "UPDATE notifications SET read=1 WHERE project=?1",
                [project],
            )?;
            Ok(())
        })
    }
}
