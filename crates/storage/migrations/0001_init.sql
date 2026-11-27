-- Session persistence for oort-agent.
--
-- Mirrors AgentState's append-only design:
--   * messages / compactions are append-only logs (INSERT only; enforced by triggers),
--   * sessions holds the frozen prefix (immutable) plus a small mutable status row.
-- Timestamps are unix milliseconds.

CREATE TABLE sessions (
    id                   TEXT PRIMARY KEY NOT NULL,
    title                TEXT NOT NULL DEFAULT '',
    provider             TEXT NOT NULL,
    model                TEXT NOT NULL,
    -- Frozen prefix: byte-identical across resumes so the provider prefix cache stays valid.
    system_prompt        TEXT NOT NULL,
    frozen_tools         TEXT NOT NULL,           -- canonical JSON of name-sorted Vec<ToolDefinition>
    status               TEXT NOT NULL CHECK (status IN ('idle', 'running', 'needs_approval', 'halted')),
    halt_reason          TEXT,
    pending_calls        TEXT,                    -- JSON PendingCalls; NULL unless tools are pending
    pending_steers       TEXT NOT NULL DEFAULT '[]',
    compaction_requested INTEGER NOT NULL DEFAULT 0,
    turn                 INTEGER NOT NULL DEFAULT 0,
    usage_json           TEXT NOT NULL,           -- JSON UsageLedger
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL
);

CREATE INDEX sessions_by_updated ON sessions (updated_at DESC);

CREATE TRIGGER sessions_prefix_immutable
BEFORE UPDATE OF system_prompt, frozen_tools ON sessions
WHEN NEW.system_prompt IS NOT OLD.system_prompt OR NEW.frozen_tools IS NOT OLD.frozen_tools
BEGIN
    SELECT RAISE(ABORT, 'oort: system_prompt and frozen_tools are immutable');
END;

CREATE TRIGGER sessions_no_delete
BEFORE DELETE ON sessions
BEGIN
    SELECT RAISE(ABORT, 'oort: sessions are append-only');
END;

CREATE TABLE messages (
    session_id   TEXT NOT NULL REFERENCES sessions (id),
    seq          INTEGER NOT NULL,                -- == index into AgentState::messages()
    role         TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'tool')),
    content_json TEXT NOT NULL,                   -- JSON Vec<ContentPart>
    usage_json   TEXT,                            -- JSON Usage of the LLM call that produced it
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (session_id, seq)
) WITHOUT ROWID;

CREATE TRIGGER messages_contiguous
BEFORE INSERT ON messages
WHEN NEW.seq <> (SELECT COUNT(*) FROM messages WHERE session_id = NEW.session_id)
BEGIN
    SELECT RAISE(ABORT, 'oort: message seq must be contiguous');
END;

CREATE TRIGGER messages_no_update
BEFORE UPDATE ON messages
BEGIN
    SELECT RAISE(ABORT, 'oort: messages are append-only');
END;

CREATE TRIGGER messages_no_delete
BEFORE DELETE ON messages
BEGIN
    SELECT RAISE(ABORT, 'oort: messages are append-only');
END;

CREATE TABLE compactions (
    session_id   TEXT NOT NULL REFERENCES sessions (id),
    epoch        INTEGER NOT NULL,                -- 1-based
    up_to_seq    INTEGER NOT NULL,                -- messages[..up_to_seq] are summarized
    summary_json TEXT NOT NULL,                   -- JSON Message
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (session_id, epoch)
) WITHOUT ROWID;

CREATE TRIGGER compactions_monotonic
BEFORE INSERT ON compactions
WHEN NEW.epoch <> (SELECT COUNT(*) + 1 FROM compactions WHERE session_id = NEW.session_id)
  OR NEW.up_to_seq <= COALESCE((SELECT MAX(up_to_seq) FROM compactions WHERE session_id = NEW.session_id), 0)
  OR NEW.up_to_seq > (SELECT COUNT(*) FROM messages WHERE session_id = NEW.session_id)
BEGIN
    SELECT RAISE(ABORT, 'oort: compaction epochs must be contiguous and advance within the history');
END;

CREATE TRIGGER compactions_no_update
BEFORE UPDATE ON compactions
BEGIN
    SELECT RAISE(ABORT, 'oort: compactions are append-only');
END;

CREATE TRIGGER compactions_no_delete
BEFORE DELETE ON compactions
BEGIN
    SELECT RAISE(ABORT, 'oort: compactions are append-only');
END;
