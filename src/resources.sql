CREATE TABLE IF NOT EXISTS resources(
    project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL,
    created_ns INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(project,kind,id)
);
CREATE INDEX IF NOT EXISTS resource_lookup ON resources(project,kind,name,created_ns DESC);
CREATE UNIQUE INDEX IF NOT EXISTS dataset_names ON resources(project,name) WHERE kind='datasets';
CREATE TABLE IF NOT EXISTS media(
    project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    id TEXT NOT NULL, trace TEXT, mime TEXT NOT NULL, size INTEGER NOT NULL,
    digest TEXT NOT NULL, grant TEXT NOT NULL, expires_ns INTEGER NOT NULL, uploaded_at TEXT, bytes BLOB,
    PRIMARY KEY(project,id)
);
CREATE TRIGGER IF NOT EXISTS delete_trace_media AFTER DELETE ON traces
BEGIN
    DELETE FROM media WHERE project=old.project AND trace=old.id;
END;
