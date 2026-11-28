use std::sync::Arc;

use oort_server::{Registry, ServerConfig, SessionManager, router};
use oort_storage::SqliteSessionStore;
use oort_storage::attachments::AttachmentStore;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,oort=debug,tower_http=info".into()),
        )
        .init();

    let cfg = ServerConfig::from_env()?;
    let store = Arc::new(SqliteSessionStore::connect(&cfg.db_url).await?);
    // Uploads that were never sent are dropped after a day.
    match store.sweep_unsent_attachments(24 * 60 * 60 * 1000).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(n, "removed unsent attachments"),
        Err(e) => tracing::warn!(error = %e, "attachment cleanup failed"),
    }
    let builtin = Arc::new(oort_server::tools::builtin_executor(&cfg.workspace));
    let registry = Arc::new(
        Registry::new(store.clone(), builtin)
            .with_skill_dirs(cfg.skill_dirs.clone())
            .with_public_url(&cfg.public_url)
            .with_web_fetch(oort_tools::builtin::utilities::WebFetchOptions {
                // Opt-in for trusted local setups; private targets are refused by default (SSRF).
                allow_private: std::env::var("OORT_WEB_FETCH_ALLOW_PRIVATE")
                    .is_ok_and(|v| v == "1"),
                ..Default::default()
            }),
    );
    tracing::info!(dirs = ?cfg.skill_dirs, redirect_uri = %registry.oauth.redirect_uri(), "skills and OAuth configured");
    if let Some(path) = &cfg.mcp_config {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let imported = registry
            .import_mcp_config(&raw)
            .await
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !imported.is_empty() {
            tracing::info!(?imported, "imported MCP servers from OORT_MCP_CONFIG");
        }
    }
    let manager = SessionManager::new(store, registry);
    let app = router(manager.clone(), cfg.web_dist.as_deref());

    let listener = tokio::net::TcpListener::bind(cfg.addr).await?;
    tracing::info!(addr = %cfg.addr, db = %cfg.db_url, "oort-server listening");
    let on_signal = manager.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down (press Ctrl+C again to force)");
            // A second Ctrl+C exits immediately instead of being swallowed by the handler.
            tokio::spawn(async {
                let _ = tokio::signal::ctrl_c().await;
                tracing::warn!("forced exit");
                std::process::exit(130);
            });
            // Cancels the loops and ends SSE streams so open connections can drain.
            on_signal.shutdown().await;
        })
        .await?;
    Ok(())
}
