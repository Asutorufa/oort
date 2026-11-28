-- Uploaded images. The bytes live in `<data_dir>/attachments/<sha256>.<ext>`; messages only
-- carry the row's id and metadata, never the content. `session_id` stays NULL until a message
-- that uses the upload is sent.
CREATE TABLE attachments (
    id           TEXT PRIMARY KEY,
    session_id   TEXT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    sha256       TEXT NOT NULL,
    mime_type    TEXT NOT NULL CHECK (mime_type IN ('image/png', 'image/jpeg', 'image/webp', 'image/gif')),
    filename     TEXT NULL,
    size_bytes   INTEGER NOT NULL,
    width        INTEGER NULL,
    height       INTEGER NULL,
    storage_path TEXT NOT NULL,
    created_at   TEXT NOT NULL
);

CREATE INDEX idx_attachments_session_id ON attachments(session_id);
CREATE INDEX idx_attachments_sha256 ON attachments(sha256);
