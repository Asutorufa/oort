-- Subagent session trees, builtin tool environments, explicit-skill state, skill invocation
-- metadata and cross-session memories (FTS5).

ALTER TABLE sessions ADD COLUMN parent_session_id TEXT REFERENCES sessions (id) ON DELETE CASCADE;
ALTER TABLE sessions ADD COLUMN parent_tool_call_id TEXT;
ALTER TABLE sessions ADD COLUMN depth INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN workspace_root TEXT;
ALTER TABLE sessions ADD COLUMN enabled_builtin_groups_json TEXT NOT NULL DEFAULT '[]';
-- Mutable step snapshot: skills whose body is loaded in the current epoch.
ALTER TABLE sessions ADD COLUMN loaded_skills_json TEXT NOT NULL DEFAULT '[]';
CREATE INDEX sessions_by_parent ON sessions (parent_session_id);

-- The tree position and tool environment are part of what a session is: never rewritten.
CREATE TRIGGER sessions_lineage_immutable
BEFORE UPDATE OF parent_session_id, parent_tool_call_id, depth, workspace_root, enabled_builtin_groups_json ON sessions
WHEN NEW.parent_session_id IS NOT OLD.parent_session_id
  OR NEW.parent_tool_call_id IS NOT OLD.parent_tool_call_id
  OR NEW.depth IS NOT OLD.depth
  OR NEW.workspace_root IS NOT OLD.workspace_root
  OR NEW.enabled_builtin_groups_json IS NOT OLD.enabled_builtin_groups_json
BEGIN
    SELECT RAISE(ABORT, 'oort: session lineage and tool environment are immutable');
END;

ALTER TABLE skills ADD COLUMN user_invocable INTEGER NOT NULL DEFAULT 1;
ALTER TABLE skills ADD COLUMN argument_hint TEXT;
ALTER TABLE skills ADD COLUMN execution_mode TEXT NOT NULL DEFAULT 'inline'
    CHECK (execution_mode IN ('inline', 'subagent'));
ALTER TABLE skills ADD COLUMN subagent_profile TEXT;

ALTER TABLE presets ADD COLUMN builtin_groups_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE presets ADD COLUMN workspace_root TEXT;

CREATE TABLE memories (
    id         TEXT PRIMARY KEY NOT NULL,
    title      TEXT NOT NULL,
    content    TEXT NOT NULL,
    tags_json  TEXT NOT NULL DEFAULT '[]',
    scope      TEXT NOT NULL DEFAULT 'global',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX memories_by_scope ON memories (scope, updated_at DESC);

-- Trigram tokens match substrings, which works for Japanese and Chinese without a segmenter.
CREATE VIRTUAL TABLE memories_fts USING fts5 (
    title, content, tags_json,
    content = 'memories', content_rowid = 'rowid', tokenize = 'trigram'
);

CREATE TRIGGER memories_fts_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts (rowid, title, content, tags_json)
    VALUES (NEW.rowid, NEW.title, NEW.content, NEW.tags_json);
END;
CREATE TRIGGER memories_fts_delete AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts (memories_fts, rowid, title, content, tags_json)
    VALUES ('delete', OLD.rowid, OLD.title, OLD.content, OLD.tags_json);
END;
CREATE TRIGGER memories_fts_update AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts (memories_fts, rowid, title, content, tags_json)
    VALUES ('delete', OLD.rowid, OLD.title, OLD.content, OLD.tags_json);
    INSERT INTO memories_fts (rowid, title, content, tags_json)
    VALUES (NEW.rowid, NEW.title, NEW.content, NEW.tags_json);
END;
