//! Recently used and starred workspace folders. Paths are stored as given: the caller
//! normalizes them.

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use oort_agent::StoreError;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::{SqliteSessionStore, backend, classify, found};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentWorkspace {
    pub path: String,
    pub starred: bool,
    pub use_count: i64,
    /// RFC 3339.
    pub last_used_at: String,
}

#[async_trait]
pub trait WorkspaceStore: Send + Sync {
    /// Inserts the folder, or counts one more use of it. The star is kept.
    async fn record_workspace(&self, path: &str) -> Result<RecentWorkspace, StoreError>;
    /// Starred folders first, then the most recently used.
    async fn recent_workspaces(&self, limit: u32) -> Result<Vec<RecentWorkspace>, StoreError>;
    async fn star_workspace(&self, path: &str, starred: bool) -> Result<(), StoreError>;
    async fn delete_workspace(&self, path: &str) -> Result<(), StoreError>;
}

fn from_row(r: &SqliteRow) -> RecentWorkspace {
    RecentWorkspace {
        path: r.get("path"),
        starred: r.get("starred"),
        use_count: r.get("use_count"),
        last_used_at: r.get("last_used_at"),
    }
}

#[async_trait]
impl WorkspaceStore for SqliteSessionStore {
    async fn record_workspace(&self, path: &str) -> Result<RecentWorkspace, StoreError> {
        // Fixed width and always UTC, so the text sorts chronologically.
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        let row = sqlx::query(
            "INSERT INTO recent_workspaces (path, last_used_at) VALUES (?, ?) \
             ON CONFLICT(path) DO UPDATE SET use_count = use_count + 1, \
             last_used_at = excluded.last_used_at RETURNING *",
        )
        .bind(path)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(classify)?;
        Ok(from_row(&row))
    }

    async fn recent_workspaces(&self, limit: u32) -> Result<Vec<RecentWorkspace>, StoreError> {
        let rows = sqlx::query(
            "SELECT * FROM recent_workspaces \
             ORDER BY starred DESC, last_used_at DESC, path LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        Ok(rows.iter().map(from_row).collect())
    }

    async fn star_workspace(&self, path: &str, starred: bool) -> Result<(), StoreError> {
        let done = sqlx::query("UPDATE recent_workspaces SET starred = ? WHERE path = ?")
            .bind(starred)
            .bind(path)
            .execute(&self.pool)
            .await
            .map_err(classify)?;
        found(done.rows_affected(), path)
    }

    async fn delete_workspace(&self, path: &str) -> Result<(), StoreError> {
        let done = sqlx::query("DELETE FROM recent_workspaces WHERE path = ?")
            .bind(path)
            .execute(&self.pool)
            .await
            .map_err(classify)?;
        found(done.rows_affected(), path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upsert_orders_starred_first_and_deletes() {
        let s = SqliteSessionStore::in_memory().await.unwrap();
        for p in ["/a", "/b", "/c"] {
            s.record_workspace(p).await.unwrap();
            tokio::task::yield_now().await;
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        s.star_workspace("/a", true).await.unwrap();
        let again = s.record_workspace("/a").await.unwrap();
        assert!(again.starred, "recording keeps the star");
        assert_eq!(again.use_count, 2);

        s.star_workspace("/b", true).await.unwrap();
        let order = |v: Vec<RecentWorkspace>| v.into_iter().map(|w| w.path).collect::<Vec<_>>();
        // Starred by recency (a was used again), then the rest.
        assert_eq!(
            order(s.recent_workspaces(20).await.unwrap()),
            ["/a", "/b", "/c"]
        );
        assert_eq!(order(s.recent_workspaces(1).await.unwrap()), ["/a"]);

        s.delete_workspace("/a").await.unwrap();
        assert_eq!(order(s.recent_workspaces(20).await.unwrap()), ["/b", "/c"]);
        assert!(matches!(
            s.delete_workspace("/a").await,
            Err(StoreError::NotFound(_))
        ));
        assert!(s.star_workspace("/missing", true).await.is_err());
    }
}
