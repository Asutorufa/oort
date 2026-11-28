-- Usage statistics and budget.
--
-- The ledgers are append-only records of what was spent. They deliberately do *not* cascade
-- with sessions: deleting a chat sets `session_id` to NULL and keeps the cost history.
-- `origin_session_id` (no foreign key) keeps rows of one deleted chat grouped together.

CREATE TABLE usage_ledger (
    id                     TEXT PRIMARY KEY NOT NULL,
    session_id             TEXT NULL REFERENCES sessions (id) ON DELETE SET NULL,
    origin_session_id      TEXT NULL,
    session_title_snapshot TEXT NULL,
    provider_id            TEXT NULL,
    vendor                 TEXT NOT NULL,
    model                  TEXT NOT NULL,
    -- turn: a conversation (or subagent) step; compaction / title: auxiliary requests.
    purpose                TEXT NOT NULL DEFAULT 'turn' CHECK (purpose IN ('turn', 'compaction', 'title')),
    is_subagent            INTEGER NOT NULL DEFAULT 0,
    input_tokens           INTEGER NOT NULL DEFAULT 0,
    cached_input_tokens    INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens     INTEGER NOT NULL DEFAULT 0,
    output_tokens          INTEGER NOT NULL DEFAULT 0,
    thinking_tokens        INTEGER NOT NULL DEFAULT 0,
    ttft_ms                INTEGER NULL,
    duration_ms            INTEGER NULL,
    estimated_cost_usd     REAL NOT NULL DEFAULT 0.0,
    cache_saved_usd        REAL NOT NULL DEFAULT 0.0,
    created_at             TEXT NOT NULL              -- RFC 3339 UTC, millisecond precision
);

CREATE INDEX idx_usage_ledger_created_at ON usage_ledger (created_at);
CREATE INDEX idx_usage_ledger_session_id ON usage_ledger (session_id);
CREATE INDEX idx_usage_ledger_model ON usage_ledger (model);

CREATE TABLE tool_usage_ledger (
    id          TEXT PRIMARY KEY NOT NULL,
    session_id  TEXT NULL REFERENCES sessions (id) ON DELETE SET NULL,
    tool_name   TEXT NOT NULL,
    tool_kind   TEXT NOT NULL CHECK (tool_kind IN ('builtin', 'server', 'mcp')),
    duration_ms INTEGER NULL,
    is_error    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);

CREATE INDEX idx_tool_usage_created_at ON tool_usage_ledger (created_at);
CREATE INDEX idx_tool_usage_session_id ON tool_usage_ledger (session_id);

-- One row (id = 1).
CREATE TABLE budget_settings (
    id                    INTEGER PRIMARY KEY CHECK (id = 1),
    monthly_budget_usd    REAL NULL,
    warning_threshold_pct INTEGER NOT NULL DEFAULT 80,
    scoped_budgets_json   TEXT NULL                   -- optional per model / provider budgets
);

INSERT INTO budget_settings (id) VALUES (1);

CREATE TABLE model_pricing_overrides (
    model_pattern      TEXT PRIMARY KEY NOT NULL,     -- exact model id, or a prefix ending in '*'
    input_per_m        REAL,
    cached_input_per_m REAL,
    cache_write_per_m  REAL,
    output_per_m       REAL,
    updated_at         TEXT
);

-- Marks one-time data migrations (the backfill needs the price table, so it runs in Rust).
CREATE TABLE analytics_meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
