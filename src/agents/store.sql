CREATE TABLE IF NOT EXISTS agent_sources(
    project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE, agent TEXT NOT NULL,
    root TEXT NOT NULL, selection TEXT NOT NULL, initial_ns INTEGER NOT NULL, pending INTEGER NOT NULL,
    active INTEGER NOT NULL, PRIMARY KEY(project,agent,root)
);
CREATE TABLE IF NOT EXISTS agent_cursors(
    project TEXT NOT NULL, agent TEXT NOT NULL, root TEXT NOT NULL, stream TEXT NOT NULL, data TEXT NOT NULL,
    PRIMARY KEY(project,agent,root,stream),
    FOREIGN KEY(project,agent,root) REFERENCES agent_sources(project,agent,root) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS notifications(
    id INTEGER PRIMARY KEY, project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title TEXT NOT NULL, message TEXT NOT NULL, level TEXT NOT NULL, source TEXT,
    created_ns INTEGER NOT NULL, read INTEGER NOT NULL DEFAULT 0, fingerprint TEXT,
    UNIQUE (project,fingerprint)
);
