//! HTTP routes.

use std::convert::Infallible;
use std::path::Path as FsPath;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use futures_util::Stream;
use tokio::sync::broadcast::error::RecvError;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::api::{
    ApiError, CreateSessionRequest, Envelope, InputRequest, SessionDetail, SessionSummary,
};
use crate::manager::SessionManager;

type AppState = Arc<SessionManager>;

pub fn router(manager: Arc<SessionManager>, web_dist: Option<&FsPath>) -> Router {
    let api = Router::new()
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/{id}", get(get_session))
        .route("/sessions/{id}/events", get(session_events))
        .route("/sessions/{id}/input", post(session_input))
        .route("/sessions/{id}/cancel", post(cancel_session));
    let mut app = Router::new()
        .nest("/api", api)
        .with_state(manager)
        .layer(TraceLayer::new_for_http());
    if let Some(dist) = web_dist {
        let index = dist.join("index.html");
        app = app.fallback_service(ServeDir::new(dist).fallback(ServeFile::new(index)));
    }
    app
}

async fn list_sessions(State(m): State<AppState>) -> Result<Json<Vec<SessionSummary>>, ApiError> {
    Ok(Json(m.list().await?))
}

async fn create_session(
    State(m): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionDetail>, ApiError> {
    let id = m.create(req).await?;
    Ok(Json(m.detail(&id).await?))
}

async fn get_session(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionDetail>, ApiError> {
    Ok(Json(m.detail(&id).await?))
}

async fn session_input(
    State(m): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<InputRequest>,
) -> Result<StatusCode, ApiError> {
    m.input(&id, input).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn cancel_session(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    Ok(if m.cancel(&id).await? {
        StatusCode::ACCEPTED
    } else {
        StatusCode::NO_CONTENT
    })
}

/// `data:` is an [`Envelope`] JSON; `id:` is its per-session sequence number.
async fn session_events(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let rx = m.subscribe(&id).await?;
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        let env = match rx.recv().await {
            Ok(env) => env,
            Err(RecvError::Lagged(skipped)) => Envelope::Lagged { skipped },
            Err(RecvError::Closed) => return None,
        };
        let mut ev = Event::default().json_data(&env).unwrap_or_default();
        if let Some(seq) = env.seq() {
            ev = ev.id(seq.to_string());
        }
        Some((Ok(ev), rx))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
