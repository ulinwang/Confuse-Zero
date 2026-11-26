CREATE TABLE IF NOT EXISTS topic_summaries(
    project TEXT NOT NULL, trace TEXT NOT NULL, facet TEXT NOT NULL, signature TEXT NOT NULL,
    updated_ns INTEGER NOT NULL, summary TEXT NOT NULL, embedding TEXT NOT NULL,
    PRIMARY KEY(project,trace,facet,signature),
    FOREIGN KEY(project,trace) REFERENCES traces(project,id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS topic_assignments(
    project TEXT NOT NULL, map TEXT NOT NULL, facet TEXT NOT NULL, trace TEXT NOT NULL,
    topic TEXT, summary TEXT, x REAL NOT NULL, y REAL NOT NULL,
    PRIMARY KEY(project,map,facet,trace),
    FOREIGN KEY(project,trace) REFERENCES traces(project,id) ON DELETE CASCADE
);
