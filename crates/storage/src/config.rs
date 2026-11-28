//! [`ConfigStore`] on the same database as the sessions.

use async_trait::async_trait;
use oort_agent::{
    ApprovalPolicyKind, ConfigStore, McpServerRecord, McpTransportKind, OAuthTokenRecord,
    OAuthTokenStore, PresetRecord, ProviderKind, ProviderRecord, SkillRecord, SkillStore,
    StoreError,
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
        skill_ids: from_json(r.get("skill_ids_json"), "skill_ids_json")?,
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
             mcp_server_ids_json, builtin_tools, approval_policy, skill_ids_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(to_json(&p.skill_ids))
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
             skill_ids_json = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&p.name)
        .bind(&p.description)
        .bind(&p.provider_id)
        .bind(&p.model)
        .bind(&p.system_prompt)
        .bind(to_json(&p.mcp_server_ids))
        .bind(p.builtin_tools)
        .bind(p.approval_policy.as_str())
        .bind(to_json(&p.skill_ids))
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

// ----- OAuth tokens --------------------------------------------------------------------------

fn oauth_from_row(r: &SqliteRow) -> Result<OAuthTokenRecord, StoreError> {
    let source: String = r.get("client_source");
    let status: String = r.get("status");
    Ok(OAuthTokenRecord {
        server_id: r.get("server_id"),
        issuer: r.get("issuer"),
        resource: r.get("resource"),
        token_endpoint: r.get("token_endpoint"),
        registration_endpoint: r.get("registration_endpoint"),
        client_id: r.get("client_id"),
        client_secret: r.get("client_secret"),
        client_source: from_json(format!("\"{source}\""), "client_source")?,
        access_token: r.get("access_token"),
        refresh_token: r.get("refresh_token"),
        expires_at: r.get("expires_at"),
        scopes: r.get("scopes"),
        status: from_json(format!("\"{status}\""), "status")?,
        updated_at: r.get("updated_at"),
    })
}

fn enum_str<T: serde::Serialize>(v: &T) -> String {
    to_json(v).trim_matches('"').to_string()
}

#[async_trait]
impl OAuthTokenStore for SqliteSessionStore {
    async fn get_oauth_token(
        &self,
        server_id: &str,
    ) -> Result<Option<OAuthTokenRecord>, StoreError> {
        sqlx::query("SELECT * FROM mcp_oauth_tokens WHERE server_id = ?")
            .bind(server_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?
            .as_ref()
            .map(oauth_from_row)
            .transpose()
    }

    async fn put_oauth_token(&self, t: &OAuthTokenRecord) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO mcp_oauth_tokens (server_id, issuer, resource, token_endpoint, \
             registration_endpoint, client_id, client_secret, client_source, access_token, \
             refresh_token, expires_at, scopes, status, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (server_id) DO UPDATE SET issuer = excluded.issuer, \
             resource = excluded.resource, token_endpoint = excluded.token_endpoint, \
             registration_endpoint = excluded.registration_endpoint, client_id = excluded.client_id, \
             client_secret = excluded.client_secret, client_source = excluded.client_source, \
             access_token = excluded.access_token, refresh_token = excluded.refresh_token, \
             expires_at = excluded.expires_at, scopes = excluded.scopes, status = excluded.status, \
             updated_at = excluded.updated_at",
        )
        .bind(&t.server_id)
        .bind(&t.issuer)
        .bind(&t.resource)
        .bind(&t.token_endpoint)
        .bind(&t.registration_endpoint)
        .bind(&t.client_id)
        .bind(&t.client_secret)
        .bind(enum_str(&t.client_source))
        .bind(&t.access_token)
        .bind(&t.refresh_token)
        .bind(t.expires_at)
        .bind(&t.scopes)
        .bind(enum_str(&t.status))
        .bind(t.updated_at)
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        Ok(())
    }

    async fn delete_oauth_token(&self, server_id: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM mcp_oauth_tokens WHERE server_id = ?")
            .bind(server_id)
            .execute(&self.pool)
            .await
            .map_err(classify)?;
        Ok(())
    }
}

// ----- skills --------------------------------------------------------------------------------

fn skill_from_row(r: &SqliteRow) -> Result<SkillRecord, StoreError> {
    Ok(SkillRecord {
        id: r.get("id"),
        name: r.get("name"),
        description: r.get("description"),
        body: r.get("body"),
        allowed_tools: from_json(r.get("allowed_tools_json"), "allowed_tools_json")?,
        tags: from_json(r.get("tags_json"), "tags_json")?,
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    })
}

#[async_trait]
impl SkillStore for SqliteSessionStore {
    async fn list_skills(&self) -> Result<Vec<SkillRecord>, StoreError> {
        self.fetch_all("SELECT * FROM skills ORDER BY name", skill_from_row)
            .await
    }

    async fn get_skill(&self, id: &str) -> Result<SkillRecord, StoreError> {
        self.fetch_one("SELECT * FROM skills WHERE id = ?", id, skill_from_row)
            .await
    }

    async fn create_skill(&self, k: &SkillRecord) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO skills (id, name, description, body, allowed_tools_json, tags_json, \
             created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&k.id)
        .bind(&k.name)
        .bind(&k.description)
        .bind(&k.body)
        .bind(to_json(&k.allowed_tools))
        .bind(to_json(&k.tags))
        .bind(k.created_at)
        .bind(k.updated_at)
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        Ok(())
    }

    async fn update_skill(&self, k: &SkillRecord) -> Result<(), StoreError> {
        let done = sqlx::query(
            "UPDATE skills SET name = ?, description = ?, body = ?, allowed_tools_json = ?, \
             tags_json = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&k.name)
        .bind(&k.description)
        .bind(&k.body)
        .bind(to_json(&k.allowed_tools))
        .bind(to_json(&k.tags))
        .bind(k.updated_at)
        .bind(&k.id)
        .execute(&self.pool)
        .await
        .map_err(classify)?;
        found(done.rows_affected(), &k.id)
    }

    async fn delete_skill(&self, id: &str) -> Result<(), StoreError> {
        self.delete_row("skills", id).await
    }
}
