//! SQLite implementation of [`oort_agent::SessionStore`] (sqlx).
//!
//! Every connection runs with `journal_mode = WAL` and `foreign_keys = ON`. The schema
//! (`migrations/`) enforces the append-only invariants with triggers, so even a buggy caller
//! cannot rewrite history or the frozen prefix.

use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use oort_agent::store::{assemble_state, canonical_tools_json};
use oort_agent::{
    AgentState, ApprovalPolicyKind, CompactionCheckpoint, Lineage, LlmCallStats, NewSession,
    ReasoningEffort, SessionId, SessionMeta, SessionSnapshot, SessionStatus, SessionStore,
    StepDelta, StoreError, StoredSession,
};
use oort_llm::{ContentPart, Message, Role, Usage};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqliteConnection, SqlitePool};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

mod config;

/// [`SessionStore`] and [`oort_agent::ConfigStore`] backed by one SQLite database.
#[derive(Debug, Clone)]
pub struct SqliteSessionStore {
    pool: SqlitePool,
}

impl SqliteSessionStore {
    /// Opens (creating if needed) a database such as `sqlite://oort.db` and runs migrations.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        let opts = SqliteConnectOptions::from_str(url)
            .map_err(backend)?
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(with_pragmas(opts))
            .await
            .map_err(backend)?;
        Self::from_pool(pool).await
    }

    /// A private in-memory database (`sqlite::memory:`). The pool is pinned to one connection
    /// that never expires, because each in-memory connection is a separate database.
    pub async fn in_memory() -> Result<Self, StoreError> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:").map_err(backend)?;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(with_pragmas(opts))
            .await
            .map_err(backend)?;
        Self::from_pool(pool).await
    }

    /// Wraps an existing pool (its connections must enable `foreign_keys`) and migrates it.
    pub async fn from_pool(pool: SqlitePool) -> Result<Self, StoreError> {
        MIGRATOR.run(&pool).await.map_err(backend)?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

fn with_pragmas(opts: SqliteConnectOptions) -> SqliteConnectOptions {
    opts.journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn create_session(
        &self,
        meta: NewSession,
        state: &AgentState,
    ) -> Result<SessionId, StoreError> {
        let tools = state.tool_snapshot().ok_or_else(|| {
            StoreError::Corrupt("tool catalog must be frozen before create_session".into())
        })?;
        let id = meta.id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let snap = SessionSnapshot::from_state(state);
        let now = now_ms();

        let mut tx = self.pool.begin().await.map_err(backend)?;
        sqlx::query(
            "INSERT INTO sessions (id, title, provider, model, config_json, approval_policy, system_prompt, \
             frozen_tools, status, halt_reason, pending_calls, pending_steers, compaction_requested, \
             turn, usage_json, created_at, updated_at, parent_session_id, parent_tool_call_id, depth, \
             workspace_root, enabled_builtin_groups_json, loaded_skills_json, reasoning_effort) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&meta.title)
        .bind(&meta.provider)
        .bind(&meta.model)
        .bind(to_json(&meta.config))
        .bind(meta.approval_policy.as_str())
        .bind(state.system_prompt())
        .bind(canonical_tools_json(tools))
        .bind(snap.status.as_str())
        .bind(&snap.halt_reason)
        .bind(snap.pending_calls.as_ref().map(to_json))
        .bind(to_json(&snap.pending_steers))
        .bind(snap.compaction_requested)
        .bind(snap.turn)
        .bind(to_json(&snap.usage))
        .bind(now)
        .bind(now)
        .bind(&meta.lineage.parent_session_id)
        .bind(&meta.lineage.parent_tool_call_id)
        .bind(meta.lineage.depth)
        .bind(&meta.lineage.workspace_root)
        .bind(to_json(&meta.lineage.builtin_groups))
        .bind(to_json(&snap.loaded_skills))
        .bind(meta.reasoning_effort.as_str())
        .execute(&mut *tx)
        .await
        .map_err(classify)?;
        for (seq, msg) in state.messages().iter().enumerate() {
            insert_message(&mut tx, &id, seq as u64, msg, None).await?;
        }
        for (i, cp) in state.checkpoints().iter().enumerate() {
            insert_compaction(&mut tx, &id, i as u32 + 1, cp).await?;
        }
        tx.commit().await.map_err(backend)?;
        Ok(id)
    }

    async fn list_sessions(&self) -> Result<Vec<SessionMeta>, StoreError> {
        let rows = sqlx::query(&format!(
            "{META_SELECT} WHERE s.parent_session_id IS NULL ORDER BY s.updated_at DESC, s.id DESC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter().map(meta_from_row).collect()
    }

    async fn load_state(&self, id: &str) -> Result<StoredSession, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(backend)?;
        let row = sqlx::query(&format!("{META_SELECT} WHERE s.id = ?"))
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(backend)?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        let meta = meta_from_row(&row)?;
        let snapshot = SessionSnapshot {
            status: meta.status,
            halt_reason: row.get("halt_reason"),
            pending_calls: from_json_opt(row.get("pending_calls"), "pending_calls")?,
            pending_steers: from_json(row.get("pending_steers"), "pending_steers")?,
            compaction_requested: row.get("compaction_requested"),
            turn: row.get("turn"),
            usage: from_json(row.get("usage_json"), "usage_json")?,
            loaded_skills: from_json(row.get("loaded_skills_json"), "loaded_skills_json")?,
        };
        let system_prompt: String = row.get("system_prompt");
        let frozen_tools: String = row.get("frozen_tools");

        let msg_rows = sqlx::query(
            "SELECT seq, role, content_json, usage_json FROM messages WHERE session_id = ? ORDER BY seq",
        )
        .bind(id)
        .fetch_all(&mut *conn)
        .await
        .map_err(backend)?;
        let mut messages = Vec::with_capacity(msg_rows.len());
        let mut message_usage = Vec::with_capacity(msg_rows.len());
        for (expected, r) in msg_rows.iter().enumerate() {
            let seq: i64 = r.get("seq");
            if seq != expected as i64 {
                return Err(StoreError::Corrupt(format!(
                    "message seq gap at {expected}"
                )));
            }
            let role: Role = from_json(format!("\"{}\"", r.get::<String, _>("role")), "role")?;
            let content: Vec<ContentPart> = from_json(r.get("content_json"), "content_json")?;
            messages.push(Message::new(role, content));
            message_usage.push(from_json_opt::<Usage>(r.get("usage_json"), "usage_json")?);
        }

        let cp_rows = sqlx::query(
            "SELECT epoch, up_to_seq, summary_json FROM compactions WHERE session_id = ? ORDER BY epoch",
        )
        .bind(id)
        .fetch_all(&mut *conn)
        .await
        .map_err(backend)?;
        let checkpoints = cp_rows
            .iter()
            .map(|r| {
                Ok(CompactionCheckpoint {
                    up_to_index: r.get::<i64, _>("up_to_seq") as usize,
                    summary_message: from_json(r.get("summary_json"), "summary_json")?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;

        let state = assemble_state(
            system_prompt,
            &frozen_tools,
            messages,
            checkpoints,
            snapshot,
        )?;
        Ok(StoredSession {
            meta,
            state,
            message_usage,
        })
    }

    async fn append_message(
        &self,
        id: &str,
        seq: u64,
        message: &Message,
        usage: Option<&Usage>,
    ) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(backend)?;
        insert_message(&mut conn, id, seq, message, usage).await?;
        touch(&mut conn, id).await
    }

    async fn save_compaction(
        &self,
        id: &str,
        epoch: u32,
        checkpoint: &CompactionCheckpoint,
    ) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(backend)?;
        insert_compaction(&mut conn, id, epoch, checkpoint).await?;
        touch(&mut conn, id).await
    }

    async fn update_session_status(
        &self,
        id: &str,
        snapshot: &SessionSnapshot,
    ) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(backend)?;
        update_status(&mut conn, id, snapshot).await
    }

    /// One transaction per step: the stored session is always protocol-consistent.
    async fn commit_step(&self, id: &str, delta: &StepDelta) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(backend)?;
        for (i, (msg, usage)) in delta.messages.iter().enumerate() {
            insert_message(&mut tx, id, delta.first_seq + i as u64, msg, usage.as_ref()).await?;
        }
        for (i, cp) in delta.checkpoints.iter().enumerate() {
            insert_compaction(&mut tx, id, delta.first_epoch + i as u32, cp).await?;
        }
        for stats in &delta.llm_calls {
            insert_llm_call(&mut tx, id, stats).await?;
        }
        update_status(&mut tx, id, &delta.snapshot).await?;
        tx.commit().await.map_err(backend)
    }

    async fn save_llm_call(&self, id: &str, stats: &LlmCallStats) -> Result<(), StoreError> {
        let mut conn = self.pool.acquire().await.map_err(backend)?;
        insert_llm_call(&mut conn, id, stats).await
    }

    async fn list_llm_calls(&self, id: &str) -> Result<Vec<LlmCallStats>, StoreError> {
        let rows = sqlx::query(
            "SELECT seq, turn, ttft_ms, latency_ms, thinking_ms, usage_json, request_json FROM llm_calls \
             WHERE session_id = ? ORDER BY seq",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter()
            .map(|r| {
                Ok(LlmCallStats {
                    turn: r.get::<i64, _>("turn") as u32,
                    seq: r.get::<i64, _>("seq") as usize,
                    ttft_ms: r.get::<Option<i64>, _>("ttft_ms").map(|v| v as u64),
                    latency_ms: r.get::<i64, _>("latency_ms") as u64,
                    thinking_ms: r.get::<Option<i64>, _>("thinking_ms").map(|v| v as u64),
                    usage: from_json_opt(r.get("usage_json"), "llm_calls.usage_json")?,
                    request: from_json(r.get("request_json"), "llm_calls.request_json")?,
                })
            })
            .collect()
    }

    async fn set_title(&self, id: &str, title: &str) -> Result<(), StoreError> {
        let done = sqlx::query("UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?")
            .bind(title)
            .bind(now_ms())
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(classify)?;
        found(done.rows_affected(), id)
    }

    async fn set_approval_policy(
        &self,
        id: &str,
        policy: ApprovalPolicyKind,
    ) -> Result<(), StoreError> {
        let done =
            sqlx::query("UPDATE sessions SET approval_policy = ?, updated_at = ? WHERE id = ?")
                .bind(policy.as_str())
                .bind(now_ms())
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(classify)?;
        found(done.rows_affected(), id)
    }

    async fn set_model(&self, id: &str, model: &str) -> Result<(), StoreError> {
        let done = sqlx::query("UPDATE sessions SET model = ?, updated_at = ? WHERE id = ?")
            .bind(model)
            .bind(now_ms())
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(classify)?;
        found(done.rows_affected(), id)
    }

    async fn set_reasoning_effort(
        &self,
        id: &str,
        effort: ReasoningEffort,
    ) -> Result<(), StoreError> {
        let done =
            sqlx::query("UPDATE sessions SET reasoning_effort = ?, updated_at = ? WHERE id = ?")
                .bind(effort.as_str())
                .bind(now_ms())
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(classify)?;
        found(done.rows_affected(), id)
    }

    async fn list_children(&self, id: &str) -> Result<Vec<SessionMeta>, StoreError> {
        let rows = sqlx::query(&format!(
            "{META_SELECT} WHERE s.parent_session_id = ? ORDER BY s.created_at, s.id"
        ))
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        rows.iter().map(meta_from_row).collect()
    }

    /// Deletes the session and its subagent descendants (children first) in one transaction.
    /// Every row is marked `deleting` first, which the delete triggers require.
    async fn delete_session(&self, id: &str) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(backend)?;
        let ids: Vec<String> = sqlx::query_scalar(
            "WITH RECURSIVE tree(id, depth) AS (SELECT id, 0 FROM sessions WHERE id = ? \
             UNION ALL SELECT s.id, t.depth + 1 FROM sessions s JOIN tree t ON s.parent_session_id = t.id) \
             SELECT id FROM tree ORDER BY depth DESC",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .map_err(backend)?;
        found(ids.len() as u64, id)?;
        for sid in &ids {
            sqlx::query("UPDATE sessions SET deleting = 1 WHERE id = ?")
                .bind(sid)
                .execute(&mut *tx)
                .await
                .map_err(classify)?;
        }
        for sid in &ids {
            for table in ["llm_calls", "compactions", "messages"] {
                sqlx::query(&format!("DELETE FROM {table} WHERE session_id = ?"))
                    .bind(sid)
                    .execute(&mut *tx)
                    .await
                    .map_err(classify)?;
            }
            sqlx::query("DELETE FROM sessions WHERE id = ?")
                .bind(sid)
                .execute(&mut *tx)
                .await
                .map_err(classify)?;
        }
        tx.commit().await.map_err(backend)
    }
}

pub(crate) fn found(rows: u64, id: &str) -> Result<(), StoreError> {
    if rows == 0 {
        Err(StoreError::NotFound(id.to_string()))
    } else {
        Ok(())
    }
}

async fn insert_llm_call(
    conn: &mut SqliteConnection,
    id: &str,
    stats: &LlmCallStats,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO llm_calls (session_id, seq, turn, ttft_ms, latency_ms, usage_json, request_json, created_at, thinking_ms) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(stats.seq as i64)
    .bind(i64::from(stats.turn))
    .bind(stats.ttft_ms.map(|v| v as i64))
    .bind(stats.latency_ms as i64)
    .bind(stats.usage.as_ref().map(to_json))
    .bind(to_json(&stats.request))
    .bind(now_ms())
    .bind(stats.thinking_ms.map(|v| v as i64))
    .execute(conn)
    .await
    .map_err(classify)?;
    Ok(())
}

// ----- SQL helpers ---------------------------------------------------------------------------

const META_SELECT: &str = "SELECT s.*, \
    (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) AS message_count \
    FROM sessions s";

async fn insert_message(
    conn: &mut SqliteConnection,
    id: &str,
    seq: u64,
    message: &Message,
    usage: Option<&Usage>,
) -> Result<(), StoreError> {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        Role::System => {
            return Err(StoreError::Corrupt(
                "system messages are never stored".into(),
            ));
        }
    };
    sqlx::query(
        "INSERT INTO messages (session_id, seq, role, content_json, usage_json, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(seq as i64)
    .bind(role)
    .bind(to_json(&message.content))
    .bind(usage.map(to_json))
    .bind(now_ms())
    .execute(conn)
    .await
    .map_err(classify)?;
    Ok(())
}

async fn insert_compaction(
    conn: &mut SqliteConnection,
    id: &str,
    epoch: u32,
    cp: &CompactionCheckpoint,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO compactions (session_id, epoch, up_to_seq, summary_json, created_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(epoch)
    .bind(cp.up_to_index as i64)
    .bind(to_json(&cp.summary_message))
    .bind(now_ms())
    .execute(conn)
    .await
    .map_err(classify)?;
    Ok(())
}

async fn update_status(
    conn: &mut SqliteConnection,
    id: &str,
    s: &SessionSnapshot,
) -> Result<(), StoreError> {
    let done = sqlx::query(
        "UPDATE sessions SET status = ?, halt_reason = ?, pending_calls = ?, pending_steers = ?, \
         compaction_requested = ?, turn = ?, usage_json = ?, loaded_skills_json = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(s.status.as_str())
    .bind(&s.halt_reason)
    .bind(s.pending_calls.as_ref().map(to_json))
    .bind(to_json(&s.pending_steers))
    .bind(s.compaction_requested)
    .bind(s.turn)
    .bind(to_json(&s.usage))
    .bind(to_json(&s.loaded_skills))
    .bind(now_ms())
    .bind(id)
    .execute(conn)
    .await
    .map_err(classify)?;
    if done.rows_affected() == 0 {
        return Err(StoreError::NotFound(id.to_string()));
    }
    Ok(())
}

async fn touch(conn: &mut SqliteConnection, id: &str) -> Result<(), StoreError> {
    sqlx::query("UPDATE sessions SET updated_at = ? WHERE id = ?")
        .bind(now_ms())
        .bind(id)
        .execute(conn)
        .await
        .map_err(backend)?;
    Ok(())
}

fn meta_from_row(row: &SqliteRow) -> Result<SessionMeta, StoreError> {
    Ok(SessionMeta {
        id: row.get("id"),
        title: row.get("title"),
        provider: row.get("provider"),
        model: row.get("model"),
        config: from_json(row.get("config_json"), "config_json")?,
        approval_policy: ApprovalPolicyKind::parse(row.get("approval_policy"))?,
        reasoning_effort: ReasoningEffort::parse(row.get("reasoning_effort"))?,
        lineage: Lineage {
            parent_session_id: row.get("parent_session_id"),
            parent_tool_call_id: row.get("parent_tool_call_id"),
            depth: row.get::<i64, _>("depth") as u32,
            workspace_root: row.get("workspace_root"),
            builtin_groups: from_json(
                row.get("enabled_builtin_groups_json"),
                "enabled_builtin_groups_json",
            )?,
        },
        status: SessionStatus::parse(row.get("status"))?,
        turn: row.get("turn"),
        message_count: row.get::<i64, _>("message_count") as u64,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

// ----- conversions ---------------------------------------------------------------------------

pub(crate) fn to_json<T: serde::Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).expect("agent types are always serializable")
}

pub(crate) fn from_json<T: serde::de::DeserializeOwned>(
    raw: String,
    column: &str,
) -> Result<T, StoreError> {
    serde_json::from_str(&raw).map_err(|e| StoreError::Corrupt(format!("{column}: {e}")))
}

fn from_json_opt<T: serde::de::DeserializeOwned>(
    raw: Option<String>,
    column: &str,
) -> Result<Option<T>, StoreError> {
    raw.map(|r| from_json(r, column)).transpose()
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

pub(crate) fn backend(e: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(e.to_string())
}

/// Trigger aborts (`oort: …`) and key violations are conflicts, not backend failures.
pub(crate) fn classify(e: sqlx::Error) -> StoreError {
    if let sqlx::Error::Database(db) = &e {
        let msg = db.message();
        if msg.starts_with("oort:") || db.is_unique_violation() || db.is_foreign_key_violation() {
            return StoreError::Conflict(msg.to_string());
        }
    }
    backend(e)
}

#[cfg(test)]
mod tests;
