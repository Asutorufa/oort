//! Uploaded images: content-addressed files next to the database plus one row per upload.
//!
//! A file is named after the SHA-256 of its bytes and never rewritten, so the same attachment
//! always encodes to the same bytes in later requests (the provider's prefix cache survives).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, SecondsFormat, Utc};
use oort_agent::StoreError;
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection};

use crate::{SqliteSessionStore, backend, classify, now_ms};

/// A file to store.
#[derive(Debug, Clone)]
pub struct NewAttachment {
    pub session_id: Option<String>,
    pub bytes: Vec<u8>,
    pub filename: Option<String>,
}

/// One stored upload. Several may share a file (same content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: String,
    pub session_id: Option<String>,
    pub sha256: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub size_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// File name inside the attachment directory.
    pub storage_path: String,
    pub created_at: String,
}

#[async_trait]
pub trait AttachmentStore: Send + Sync {
    /// Stores the bytes (once per content) and a new row. Rejects anything but PNG, JPEG,
    /// WebP and GIF, judged by the content itself.
    async fn save_attachment(&self, new: NewAttachment) -> Result<Attachment, StoreError>;

    async fn get_attachment(&self, id: &str) -> Result<Option<Attachment>, StoreError>;

    async fn read_attachment(&self, attachment: &Attachment) -> Result<Vec<u8>, StoreError>;

    /// Binds unbound uploads to a session, all or none, and returns them in the given order.
    /// Fails if one is unknown or already belongs to another session.
    async fn claim_attachments(
        &self,
        session_id: &str,
        ids: &[String],
    ) -> Result<Vec<Attachment>, StoreError>;

    /// Removes uploads older than `max_age_ms` that were never sent. Returns how many.
    async fn sweep_unsent_attachments(&self, max_age_ms: i64) -> Result<usize, StoreError>;
}

/// `(mime type, file extension)` from the magic bytes.
pub fn sniff_image(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image/gif", "gif"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

fn timestamp(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn from_row(r: &SqliteRow) -> Attachment {
    let dim = |name: &str| {
        r.get::<Option<i64>, _>(name)
            .and_then(|v| u32::try_from(v).ok())
    };
    Attachment {
        id: r.get("id"),
        session_id: r.get("session_id"),
        sha256: r.get("sha256"),
        mime_type: r.get("mime_type"),
        filename: r.get("filename"),
        size_bytes: r.get::<i64, _>("size_bytes").max(0) as u64,
        width: dim("width"),
        height: dim("height"),
        storage_path: r.get("storage_path"),
        created_at: r.get("created_at"),
    }
}

/// Writes `bytes` to `path` unless it is already there; readers never see a partial file.
async fn write_once(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if tokio::fs::metadata(path)
        .await
        .is_ok_and(|m| m.len() == bytes.len() as u64)
    {
        return Ok(());
    }
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::now_v7()));
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}

/// Deletes the rows of a session, returning the files they used.
pub(crate) async fn delete_session_rows(
    conn: &mut SqliteConnection,
    session_id: &str,
) -> Result<Vec<String>, StoreError> {
    sqlx::query_scalar("DELETE FROM attachments WHERE session_id = ? RETURNING storage_path")
        .bind(session_id)
        .fetch_all(conn)
        .await
        .map_err(classify)
}

impl SqliteSessionStore {
    /// Where attachment files are kept.
    pub fn attachments_dir(&self) -> &Path {
        &self.attachments_dir
    }

    /// Keeps attachment files in `dir` instead of next to the database.
    pub fn with_attachments_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.attachments_dir = dir.into();
        self
    }

    /// Removes the files no row refers to any more. The caller holds `files_lock`.
    pub(crate) async fn remove_unreferenced(
        &self,
        paths: impl IntoIterator<Item = String>,
    ) -> Result<(), StoreError> {
        for path in paths.into_iter().collect::<BTreeSet<_>>() {
            let used: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM attachments WHERE storage_path = ? LIMIT 1")
                    .bind(&path)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(backend)?;
            if used.is_none()
                && let Err(e) = tokio::fs::remove_file(self.attachments_dir.join(&path)).await
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(file = %path, error = %e, "attachment file not removed");
            }
        }
        Ok(())
    }
}

#[async_trait]
impl AttachmentStore for SqliteSessionStore {
    async fn save_attachment(&self, new: NewAttachment) -> Result<Attachment, StoreError> {
        let (mime, ext) = sniff_image(&new.bytes).ok_or_else(|| {
            StoreError::Conflict("only PNG, JPEG, WebP and GIF images are supported".into())
        })?;
        let sha256: String = Sha256::digest(&new.bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let size = imagesize::blob_size(&new.bytes).ok();
        let attachment = Attachment {
            id: format!("att_{}", uuid::Uuid::now_v7().simple()),
            session_id: new.session_id,
            storage_path: format!("{sha256}.{ext}"),
            sha256,
            mime_type: mime.to_string(),
            filename: new.filename,
            size_bytes: new.bytes.len() as u64,
            width: size.and_then(|s| u32::try_from(s.width).ok()),
            height: size.and_then(|s| u32::try_from(s.height).ok()),
            created_at: timestamp(now_ms()),
        };
        // Held so a concurrent delete cannot remove the file before its new row exists.
        let _files = self.files_lock.lock().await;
        tokio::fs::create_dir_all(&self.attachments_dir)
            .await
            .map_err(backend)?;
        write_once(
            &self.attachments_dir.join(&attachment.storage_path),
            &new.bytes,
        )
        .await
        .map_err(backend)?;
        sqlx::query(
            "INSERT INTO attachments (id, session_id, sha256, mime_type, filename, size_bytes, \
             width, height, storage_path, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&attachment.id)
        .bind(&attachment.session_id)
        .bind(&attachment.sha256)
        .bind(&attachment.mime_type)
        .bind(&attachment.filename)
        .bind(attachment.size_bytes as i64)
        .bind(attachment.width.map(i64::from))
        .bind(attachment.height.map(i64::from))
        .bind(&attachment.storage_path)
        .bind(&attachment.created_at)
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        Ok(attachment)
    }

    async fn get_attachment(&self, id: &str) -> Result<Option<Attachment>, StoreError> {
        let row = sqlx::query("SELECT * FROM attachments WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        Ok(row.as_ref().map(from_row))
    }

    async fn read_attachment(&self, attachment: &Attachment) -> Result<Vec<u8>, StoreError> {
        tokio::fs::read(self.attachments_dir.join(&attachment.storage_path))
            .await
            .map_err(|e| StoreError::Backend(format!("attachment {}: {e}", attachment.id)))
    }

    async fn claim_attachments(
        &self,
        session_id: &str,
        ids: &[String],
    ) -> Result<Vec<Attachment>, StoreError> {
        let mut tx = self.pool.begin().await.map_err(backend)?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let row = sqlx::query(
                "UPDATE attachments SET session_id = ? \
                 WHERE id = ? AND (session_id IS NULL OR session_id = ?) RETURNING *",
            )
            .bind(session_id)
            .bind(id)
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify)?
            .ok_or_else(|| StoreError::Conflict(format!("unknown attachment {id}")))?;
            out.push(from_row(&row));
        }
        tx.commit().await.map_err(backend)?;
        Ok(out)
    }

    async fn sweep_unsent_attachments(&self, max_age_ms: i64) -> Result<usize, StoreError> {
        let _files = self.files_lock.lock().await;
        let paths: Vec<String> = sqlx::query_scalar(
            "DELETE FROM attachments WHERE session_id IS NULL AND created_at < ? \
             RETURNING storage_path",
        )
        .bind(timestamp(now_ms().saturating_sub(max_age_ms)))
        .fetch_all(&self.pool)
        .await
        .map_err(classify)?;
        let removed = paths.len();
        self.remove_unreferenced(paths).await?;
        Ok(removed)
    }
}
