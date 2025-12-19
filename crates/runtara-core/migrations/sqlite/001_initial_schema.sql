-- SQLite-compatible schema for runtara-core.

CREATE TABLE instances (
    instance_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    definition_version INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'pending',
    checkpoint_id TEXT,
    attempt INTEGER NOT NULL DEFAULT 1,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    started_at TEXT,
    finished_at TEXT,
    output BLOB,
    error TEXT,
    CONSTRAINT valid_attempt CHECK (attempt >= 1 AND attempt <= max_attempts)
);

CREATE INDEX idx_instances_tenant ON instances(tenant_id);
CREATE INDEX idx_instances_status ON instances(status);
CREATE INDEX idx_instances_created ON instances(created_at);

CREATE TABLE checkpoints (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT NOT NULL,
    state BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(instance_id, checkpoint_id)
);

CREATE INDEX idx_checkpoints_instance ON checkpoints(instance_id);
CREATE INDEX idx_checkpoints_instance_latest ON checkpoints(instance_id, created_at DESC);

CREATE TABLE wake_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT NOT NULL,
    wake_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(instance_id)
);

CREATE INDEX idx_wake_queue_wake_at ON wake_queue(wake_at);

CREATE TABLE instance_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    checkpoint_id TEXT,
    payload BLOB,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_instance_events_instance ON instance_events(instance_id);
CREATE INDEX idx_instance_events_created ON instance_events(created_at);

CREATE TABLE pending_signals (
    instance_id TEXT PRIMARY KEY REFERENCES instances(instance_id) ON DELETE CASCADE,
    signal_type TEXT NOT NULL,
    payload BLOB,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    acknowledged_at TEXT
);

CREATE TABLE pending_custom_signals (
    instance_id TEXT NOT NULL REFERENCES instances(instance_id) ON DELETE CASCADE,
    checkpoint_id TEXT NOT NULL,
    payload BLOB,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (instance_id, checkpoint_id)
);
