PRAGMA foreign_keys=ON;
BEGIN;

CREATE TABLE IF NOT EXISTS projects(
    id TEXT PRIMARY KEY, name TEXT NOT NULL, created_ns INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS keys(
    id TEXT PRIMARY KEY, project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind TEXT NOT NULL, public TEXT UNIQUE NOT NULL, digest BLOB UNIQUE NOT NULL,
    display TEXT NOT NULL, created_ns INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS traces(
    project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE, id TEXT NOT NULL,
    session TEXT, name TEXT NOT NULL, user_id TEXT, source TEXT NOT NULL, start_ns INTEGER NOT NULL,
    received_ns INTEGER NOT NULL, data TEXT NOT NULL,
    PRIMARY KEY(project,id)
);
CREATE INDEX IF NOT EXISTS trace_time ON traces(project,start_ns DESC);
CREATE INDEX IF NOT EXISTS trace_session ON traces(project,session,start_ns DESC);
CREATE TABLE IF NOT EXISTS observations(
    project TEXT NOT NULL, trace TEXT NOT NULL, id TEXT NOT NULL,
    parent TEXT, kind TEXT NOT NULL, start_ns INTEGER NOT NULL, data TEXT NOT NULL,
    PRIMARY KEY(project,trace,id),
    FOREIGN KEY (project,trace) REFERENCES traces(project,id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS observation_time ON observations(project,start_ns);
CREATE INDEX IF NOT EXISTS observation_id ON observations(project,id);
CREATE TABLE IF NOT EXISTS scores(
    project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE, id TEXT NOT NULL,
    trace TEXT, name TEXT NOT NULL, created_ns INTEGER NOT NULL, data TEXT NOT NULL,
    PRIMARY KEY(project,id)
);
CREATE INDEX IF NOT EXISTS score_trace ON scores(project,trace);
CREATE TRIGGER IF NOT EXISTS delete_trace_scores AFTER DELETE ON traces
BEGIN
    DELETE FROM scores WHERE project=old.project AND trace=old.id;
END;
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY, value
);

COMMIT;
