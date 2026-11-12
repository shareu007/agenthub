CREATE TABLE sources (
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    root TEXT NOT NULL,
    path TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    parser_version INTEGER NOT NULL,
    present INTEGER NOT NULL DEFAULT 1,
    synced_at INTEGER NOT NULL
);
CREATE INDEX sources_root ON sources(agent,root);
CREATE TABLE sessions (
    key TEXT PRIMARY KEY,
    native_id TEXT NOT NULL,
    agent TEXT NOT NULL,
    source_id TEXT NOT NULL REFERENCES sources(id),
    title TEXT NOT NULL,
    project_path TEXT,
    created_at INTEGER,
    updated_at INTEGER,
    source_path TEXT NOT NULL,
    parent_id TEXT,
    warnings TEXT NOT NULL,
    available INTEGER NOT NULL DEFAULT 1,
    message_count INTEGER NOT NULL DEFAULT 0,
    size_bytes INTEGER NOT NULL DEFAULT 0,
    display_title TEXT
);
CREATE INDEX sessions_source ON sessions(source_id);
CREATE INDEX sessions_updated ON sessions(updated_at DESC,key);
CREATE INDEX sessions_agent_updated ON sessions(agent,updated_at DESC,key);
CREATE INDEX sessions_project_updated ON sessions(project_path,updated_at DESC,key);
CREATE INDEX sessions_native_id ON sessions(native_id);
CREATE TABLE messages (
    session_key TEXT NOT NULL REFERENCES sessions(key),
    ordinal INTEGER NOT NULL,
    native_id TEXT,
    parent_id TEXT,
    role TEXT NOT NULL,
    kind TEXT NOT NULL,
    timestamp INTEGER,
    body TEXT NOT NULL,
    raw TEXT NOT NULL,
    PRIMARY KEY (session_key,ordinal)
);
CREATE TABLE documents (
    id INTEGER PRIMARY KEY,
    session_key TEXT NOT NULL REFERENCES sessions(key),
    ordinal INTEGER NOT NULL,
    body TEXT NOT NULL,
    UNIQUE(session_key,ordinal)
);
CREATE VIRTUAL TABLE documents_fts USING fts5(body,content=documents,content_rowid=id,tokenize='trigram');
CREATE TRIGGER documents_insert AFTER INSERT ON documents BEGIN
    INSERT INTO documents_fts(rowid,body) VALUES(new.id,new.body);
END;
CREATE TRIGGER documents_delete AFTER DELETE ON documents BEGIN
    INSERT INTO documents_fts(documents_fts,rowid,body) VALUES('delete',old.id,old.body);
END;
CREATE TRIGGER documents_update AFTER UPDATE ON documents BEGIN
    INSERT INTO documents_fts(documents_fts,rowid,body) VALUES('delete',old.id,old.body);
    INSERT INTO documents_fts(rowid,body) VALUES(new.id,new.body);
END;

