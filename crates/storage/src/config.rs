//! [`ConfigStore`] on the same database as the sessions.

use async_trait::async_trait;
use oort_agent::{
    ApprovalPolicyKind, ConfigStore, McpServerRecord, McpTransportKind, PresetRecord, ProviderKind,
    ProviderRecord, StoreError,
};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::{SqliteSessionStore, backend, classify, found, from_json, to_json};

fn provider_from_row(r: &SqliteRow) -> Result<ProviderRecord, StoreError> {
    Ok(ProviderRecord {
        id: r.get("id"),
        name: r.get("name"),
        kind: ProviderKind::parse(r.get("kind"))?,
        base_url: r.get("base_url"),
        api_key: r.get("api_key"),
        default_model: r.get("default_model"),
        extra_headers: from_json(r.get("extra_headers_json"), "extra_headers_json")?,
        is_enabled: r.get("is_enabled"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    })
}

fn mcp_from_row(r: &SqliteRow) -> Result<McpServerRecord, StoreError> {
    Ok(McpServerRecord {
        id: r.get("id"),
        name: r.get("name"),
        transport: McpTransportKind::parse(r.get("transport"))?,
        url: r.get("url"),
        command: r.get("command"),
        args: from_json(r.get("args_json"), "args_json")?,
        env: from_json(r.get("env_json"), "env_json")?,
        headers: from_json(r.get("headers_json"), "headers_json")?,
        auth: from_json(r.get("auth_json"), "auth_json")?,
        is_enabled: r.get("is_enabled"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    })
}

fn preset_from_row(r: &SqliteRow) -> Result<PresetRecord, StoreError> {
    Ok(PresetRecord {
        id: r.get("id"),
        name: r.get("name"),
        description: r.get("description"),
        provider_id: r.get("provider_id"),
        model: r.get("model"),
        system_prompt: r.get("system_prompt"),
        mcp_server_ids: from_json(r.get("mcp_server_ids_json"), "mcp_server_ids_json")?,
        builtin_tools: r.get("builtin_tools"),
        approval_policy: ApprovalPolicyKind::parse(r.get("approval_policy"))?,
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    })
}

impl SqliteSessionStore {
    async fn fetch_all<T>(
        &self,
        sql: &str,
        map: fn(&SqliteRow) -> Result<T, StoreError>,
    ) -> Result<Vec<T>, StoreError> {
        let rows = sqlx::query(sql)
            .fetch_all(&self.pool)
            .await
            .map_err(backend)?;
        rows.iter().map(map).collect()
    }

    async fn fetch_one<T>(
        &self,
        sql: &str,
        id: &str,
        map: fn(&SqliteRow) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let row = sqlx::query(sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        map(&row)
    }

    async fn delete_row(&self, table: &str, id: &str) -> Result<(), StoreError> {
        let done = sqlx::query(&format!("DELETE FROM {table} WHERE id = ?"))
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(classify)?;
        found(done.rows_affected(), id)
    }

    /// Names of presets that reference `id` through `column` (an id or a JSON id array).
    async fn referencing_presets(
        &self,
        id: &str,
        json_array: bool,
    ) -> Result<Vec<String>, StoreError> {
        let sql = if json_array {
            "SELECT name FROM presets WHERE EXISTS \
             (SELECT 1 FROM json_each(presets.mcp_server_ids_json) WHERE value = ?) ORDER BY name"
        } else {
            "SELECT name FROM presets WHERE provider_id = ? ORDER BY name"
        };
        let rows = sqlx::query(sql)
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(backend)?;
        Ok(rows.iter().map(|r| r.get("name")).collect())
    }

    async fn ensure_unreferenced(&self, id: &str, json_array: bool) -> Result<(), StoreError> {
        let names = self.referencing_presets(id, json_array).await?;
        if names.is_empty() {
            Ok(())
        } else {
            Err(StoreError::Conflict(format!(
                "still used by preset(s): {}",
                names.join(", ")
            )))
        }
    }
}

#[async_trait]
impl ConfigStore for SqliteSessionStore {
    async fn list_providers(&self) -> Result<Vec<ProviderRecord>, StoreError> {
        self.fetch_all("SELECT * FROM providers ORDER BY name", provider_from_row)
            .await
    }

    async fn get_provider(&self, id: &str) -> Result<ProviderRecord, StoreError> {
        self.fetch_one(
            "SELECT * FROM providers WHERE id = ?",
            id,
            provider_from_row,
        )
        .await
    }

    async fn create_provider(&self, p: &ProviderRecord) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO providers (id, name, kind, base_url, api_key, default_model, \
             extra_headers_json, is_enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&p.id)
        .bind(&p.name)
        .bind(p.kind.as_str())
        .bind(&p.base_url)
        .bind(&p.api_key)
        .bind(&p.default_model)
        .bind(to_json(&p.extra_headers))
        .bind(p.is_enabled)
        .bind(p.created_at)
        .bind(p.updated_at)
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        Ok(())
    }

    async fn update_provider(&self, p: &ProviderRecord) -> Result<(), StoreError> {
        let done = sqlx::query(
            "UPDATE providers SET name = ?, kind = ?, base_url = ?, api_key = ?, default_model = ?, \
             extra_headers_json = ?, is_enabled = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&p.name)
        .bind(p.kind.as_str())
        .bind(&p.base_url)
        .bind(&p.api_key)
        .bind(&p.default_model)
        .bind(to_json(&p.extra_headers))
        .bind(p.is_enabled)
        .bind(p.updated_at)
        .bind(&p.id)
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        found(done.rows_affected(), &p.id)
    }

    async fn delete_provider(&self, id: &str) -> Result<(), StoreError> {
        self.ensure_unreferenced(id, false).await?;
        self.delete_row("providers", id).await
    }

    async fn list_mcp_servers(&self) -> Result<Vec<McpServerRecord>, StoreError> {
        self.fetch_all("SELECT * FROM mcp_servers ORDER BY name", mcp_from_row)
            .await
    }

    async fn get_mcp_server(&self, id: &str) -> Result<McpServerRecord, StoreError> {
        self.fetch_one("SELECT * FROM mcp_servers WHERE id = ?", id, mcp_from_row)
            .await
    }

    async fn create_mcp_server(&self, m: &McpServerRecord) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO mcp_servers (id, name, transport, url, command, args_json, env_json, \
             headers_json, auth_json, is_enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&m.id)
        .bind(&m.name)
        .bind(m.transport.as_str())
        .bind(&m.url)
        .bind(&m.command)
        .bind(to_json(&m.args))
        .bind(to_json(&m.env))
        .bind(to_json(&m.headers))
        .bind(to_json(&m.auth))
        .bind(m.is_enabled)
        .bind(m.created_at)
        .bind(m.updated_at)
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        Ok(())
    }

    async fn update_mcp_server(&self, m: &McpServerRecord) -> Result<(), StoreError> {
        let done = sqlx::query(
            "UPDATE mcp_servers SET name = ?, transport = ?, url = ?, command = ?, args_json = ?, \
             env_json = ?, headers_json = ?, auth_json = ?, is_enabled = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&m.name)
        .bind(m.transport.as_str())
        .bind(&m.url)
        .bind(&m.command)
        .bind(to_json(&m.args))
        .bind(to_json(&m.env))
        .bind(to_json(&m.headers))
        .bind(to_json(&m.auth))
        .bind(m.is_enabled)
        .bind(m.updated_at)
        .bind(&m.id)
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        found(done.rows_affected(), &m.id)
    }

    async fn delete_mcp_server(&self, id: &str) -> Result<(), StoreError> {
        self.ensure_unreferenced(id, true).await?;
        self.delete_row("mcp_servers", id).await
    }

    async fn list_presets(&self) -> Result<Vec<PresetRecord>, StoreError> {
        self.fetch_all("SELECT * FROM presets ORDER BY name", preset_from_row)
            .await
    }

    async fn get_preset(&self, id: &str) -> Result<PresetRecord, StoreError> {
        self.fetch_one("SELECT * FROM presets WHERE id = ?", id, preset_from_row)
            .await
    }

    async fn create_preset(&self, p: &PresetRecord) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO presets (id, name, description, provider_id, model, system_prompt, \
             mcp_server_ids_json, builtin_tools, approval_policy, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&p.id)
        .bind(&p.name)
        .bind(&p.description)
        .bind(&p.provider_id)
        .bind(&p.model)
        .bind(&p.system_prompt)
        .bind(to_json(&p.mcp_server_ids))
        .bind(p.builtin_tools)
        .bind(p.approval_policy.as_str())
        .bind(p.created_at)
        .bind(p.updated_at)
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        Ok(())
    }

    async fn update_preset(&self, p: &PresetRecord) -> Result<(), StoreError> {
        let done = sqlx::query(
            "UPDATE presets SET name = ?, description = ?, provider_id = ?, model = ?, \
             system_prompt = ?, mcp_server_ids_json = ?, builtin_tools = ?, approval_policy = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(&p.name)
        .bind(&p.description)
        .bind(&p.provider_id)
        .bind(&p.model)
        .bind(&p.system_prompt)
        .bind(to_json(&p.mcp_server_ids))
        .bind(p.builtin_tools)
        .bind(p.approval_policy.as_str())
        .bind(p.updated_at)
        .bind(&p.id)
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        found(done.rows_affected(), &p.id)
    }

    async fn delete_preset(&self, id: &str) -> Result<(), StoreError> {
        self.delete_row("presets", id).await
    }
}
