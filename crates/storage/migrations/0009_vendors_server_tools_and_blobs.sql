-- Vendor catalog, provider-hosted server tools, custom extra_body, and tool output spill cache.

-- 1. Providers: decouple vendor from protocol (kind), add default server tools & extra_body.
ALTER TABLE providers ADD COLUMN vendor TEXT NOT NULL DEFAULT 'custom';
ALTER TABLE providers ADD COLUMN default_server_tools_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE providers ADD COLUMN extra_body_json TEXT;

-- Backfill vendor for existing providers based on their protocol kind.
UPDATE providers
SET vendor = CASE kind
    WHEN 'openai' THEN 'openai'
    WHEN 'anthropic' THEN 'anthropic'
    WHEN 'gemini' THEN 'gemini'
    ELSE 'custom'
END
WHERE vendor = 'custom';

-- 2. Sessions & Presets: optional server tools override.
ALTER TABLE sessions ADD COLUMN enabled_server_tools_json TEXT;
ALTER TABLE presets ADD COLUMN enabled_server_tools_json TEXT;

-- 3. Tool output overflow cache (spill-to-storage for web_fetch / run_command).
CREATE TABLE tool_output_blobs (
    id          TEXT PRIMARY KEY NOT NULL,
    session_id  TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    tool_name   TEXT NOT NULL,
    source_hint TEXT,
    content     TEXT NOT NULL,
    total_chars INTEGER NOT NULL,
    total_lines INTEGER NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE INDEX tool_output_blobs_by_session ON tool_output_blobs (session_id);
