-- MCP OAuth tokens, database skills, and skills attached to presets.

-- Kept apart from mcp_servers: refreshing a token must not change the server record, whose
-- updated_at versions pooled connections.
CREATE TABLE mcp_oauth_tokens (
    server_id             TEXT PRIMARY KEY NOT NULL REFERENCES mcp_servers (id) ON DELETE CASCADE,
    issuer                TEXT NOT NULL,
    resource              TEXT NOT NULL,
    token_endpoint        TEXT NOT NULL,
    registration_endpoint TEXT,
    client_id             TEXT NOT NULL,
    client_secret         TEXT,
    client_source         TEXT NOT NULL CHECK (client_source IN ('dcr', 'manual')),
    access_token          TEXT,
    refresh_token         TEXT,
    expires_at            INTEGER,                  -- unix ms
    scopes                TEXT NOT NULL DEFAULT '', -- stored in full, never truncated
    status                TEXT NOT NULL CHECK (status IN ('connected', 'auth_required')),
    updated_at            INTEGER NOT NULL
);

CREATE TABLE skills (
    id                 TEXT PRIMARY KEY NOT NULL,
    name               TEXT NOT NULL UNIQUE,
    description        TEXT NOT NULL,
    body               TEXT NOT NULL,
    allowed_tools_json TEXT NOT NULL DEFAULT '[]',
    tags_json          TEXT NOT NULL DEFAULT '[]',
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);

ALTER TABLE presets ADD COLUMN skill_ids_json TEXT NOT NULL DEFAULT '[]';
