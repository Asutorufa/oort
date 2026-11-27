-- Workbench configuration (providers, MCP servers, presets), mutable session settings,
-- per-LLM-call measurements and guarded session deletion.
--
-- Configuration rows are ordinary mutable records. Sessions only *reference* them by id in
-- config_json; their frozen prefix (system_prompt, frozen_tools) is never derived again, so
-- editing or deleting configuration cannot change an existing session's requests.

CREATE TABLE providers (
    id                 TEXT PRIMARY KEY NOT NULL,
    name               TEXT NOT NULL UNIQUE,
    kind               TEXT NOT NULL CHECK (kind IN ('openai', 'anthropic', 'gemini', 'openai_compatible', 'mock')),
    base_url           TEXT,
    api_key            TEXT,                          -- literal or 'env:NAME'
    default_model      TEXT NOT NULL DEFAULT '',
    extra_headers_json TEXT NOT NULL DEFAULT '{}',
    is_enabled         INTEGER NOT NULL DEFAULT 1,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);

-- An offline provider so the workbench works without any API key.
INSERT INTO providers (id, name, kind, default_model, created_at, updated_at)
VALUES ('mock', 'Mock (offline)', 'mock', 'mock',
        CAST(strftime('%s', 'now') AS INTEGER) * 1000, CAST(strftime('%s', 'now') AS INTEGER) * 1000);

CREATE TABLE mcp_servers (
    id           TEXT PRIMARY KEY NOT NULL,
    name         TEXT NOT NULL UNIQUE,              -- tool-name prefix inside sessions
    transport    TEXT NOT NULL CHECK (transport IN ('streamable_http', 'stdio')),
    url          TEXT,
    command      TEXT,
    args_json    TEXT NOT NULL DEFAULT '[]',
    env_json     TEXT NOT NULL DEFAULT '{}',
    headers_json TEXT NOT NULL DEFAULT '{}',
    auth_json    TEXT NOT NULL DEFAULT '{"type":"none"}',
    is_enabled   INTEGER NOT NULL DEFAULT 1,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);

CREATE TABLE presets (
    id                  TEXT PRIMARY KEY NOT NULL,
    name                TEXT NOT NULL UNIQUE,
    description         TEXT NOT NULL DEFAULT '',
    provider_id         TEXT NOT NULL REFERENCES providers (id),
    model               TEXT NOT NULL DEFAULT '',
    system_prompt       TEXT NOT NULL DEFAULT '',
    mcp_server_ids_json TEXT NOT NULL DEFAULT '[]',
    builtin_tools       INTEGER NOT NULL DEFAULT 1,
    approval_policy     TEXT NOT NULL CHECK (approval_policy IN ('auto_all', 'auto_readonly', 'always_ask')),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);

-- Mutable session settings (not part of the request prefix).
ALTER TABLE sessions ADD COLUMN approval_policy TEXT NOT NULL DEFAULT 'auto_readonly'
    CHECK (approval_policy IN ('auto_all', 'auto_readonly', 'always_ask'));
-- Existing sessions created with approval "never" keep running without approval.
UPDATE sessions SET approval_policy = 'auto_all' WHERE json_extract(config_json, '$.approval') = 'never';

-- Set (in the same transaction) right before a session is deleted; nothing else may delete.
ALTER TABLE sessions ADD COLUMN deleting INTEGER NOT NULL DEFAULT 0;

-- One row per LLM call, keyed by the seq of the assistant message it produced. Append-only.
CREATE TABLE llm_calls (
    session_id   TEXT NOT NULL REFERENCES sessions (id),
    seq          INTEGER NOT NULL,
    turn         INTEGER NOT NULL,
    ttft_ms      INTEGER,
    latency_ms   INTEGER NOT NULL,
    usage_json   TEXT,
    request_json TEXT NOT NULL,                     -- JSON RequestSummary (no payload)
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (session_id, seq)
) WITHOUT ROWID;

CREATE TRIGGER llm_calls_no_update
BEFORE UPDATE ON llm_calls
BEGIN
    SELECT RAISE(ABORT, 'oort: llm_calls are append-only');
END;

-- ----- guarded deletion ----------------------------------------------------------------------

DROP TRIGGER sessions_no_delete;
DROP TRIGGER messages_no_delete;
DROP TRIGGER compactions_no_delete;

CREATE TRIGGER sessions_no_delete
BEFORE DELETE ON sessions
WHEN OLD.deleting IS NOT 1
BEGIN
    SELECT RAISE(ABORT, 'oort: sessions are append-only (mark deleting first)');
END;

CREATE TRIGGER messages_no_delete
BEFORE DELETE ON messages
WHEN (SELECT deleting FROM sessions WHERE id = OLD.session_id) IS NOT 1
BEGIN
    SELECT RAISE(ABORT, 'oort: messages are append-only');
END;

CREATE TRIGGER compactions_no_delete
BEFORE DELETE ON compactions
WHEN (SELECT deleting FROM sessions WHERE id = OLD.session_id) IS NOT 1
BEGIN
    SELECT RAISE(ABORT, 'oort: compactions are append-only');
END;

CREATE TRIGGER llm_calls_no_delete
BEFORE DELETE ON llm_calls
WHEN (SELECT deleting FROM sessions WHERE id = OLD.session_id) IS NOT 1
BEGIN
    SELECT RAISE(ABORT, 'oort: llm_calls are append-only');
END;
