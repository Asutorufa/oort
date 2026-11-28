-- Workspace folders the user has picked or started chats in. `path` is absolute, with `~`
-- expanded and no trailing slash.
CREATE TABLE recent_workspaces (
    path         TEXT PRIMARY KEY,
    starred      INTEGER NOT NULL DEFAULT 0,
    use_count    INTEGER NOT NULL DEFAULT 1,
    last_used_at TEXT NOT NULL
);

CREATE INDEX idx_recent_workspaces_order ON recent_workspaces(starred DESC, last_used_at DESC);
