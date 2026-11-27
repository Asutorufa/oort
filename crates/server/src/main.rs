use std::sync::Arc;

use oort_server::{AgentFactory, ServerConfig, SessionManager, router};
use oort_storage::SqliteSessionStore;
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
    let factory = Arc::new(AgentFactory::from_config(&cfg).await?);
    tracing::info!(tools = ?factory.frozen().definitions().iter().map(|t| &t.name).collect::<Vec<_>>(), "tool catalog frozen");
    let manager = SessionManager::new(store, factory);
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
