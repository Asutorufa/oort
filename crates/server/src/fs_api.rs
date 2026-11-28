//! `/api/fs/list` and `/api/workspaces/recent`: the server-side folder browser behind the
//! workspace path picker (it works the same over a remote connection).
//!
//! Only directory names are exposed, never file contents.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Json, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use oort_storage::workspaces::{RecentWorkspace, WorkspaceStore};
use serde::{Deserialize, Serialize};

use crate::api::ApiError;
use crate::manager::SessionManager;

type AppState = Arc<SessionManager>;
type ApiResult<T> = Result<Json<T>, ApiError>;

const MAX_ENTRIES: usize = 200;
const MAX_RECENT: u32 = 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/fs/list", get(list_dir))
        .route(
            "/workspaces/recent",
            get(list_recent).post(add_recent).delete(delete_recent),
        )
        .route("/workspaces/recent/star", patch(star_recent))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// An absolute path without a trailing slash: `~` and `~/…` become the home directory, and
/// relative paths are taken from the server's working directory. Symlinks are kept.
pub fn expand_path(raw: &str) -> PathBuf {
    let raw = raw.trim();
    let tilde = (raw == "~")
        .then_some("")
        .or_else(|| raw.strip_prefix("~/"));
    let path = match (tilde, home_dir()) {
        (Some(rest), Some(home)) => home.join(rest.trim_start_matches('/')),
        _ => cwd().join(raw),
    };
    // `..` after a symlink cannot be resolved by text alone.
    if path.components().any(|c| c == Component::ParentDir)
        && let Ok(real) = path.canonicalize()
    {
        return real;
    }
    // Rebuilding from components drops `.`, doubled and trailing slashes.
    path.components().collect()
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn is_git_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

#[derive(Debug, Default, Deserialize)]
struct ListParams {
    path: Option<String>,
    #[serde(default)]
    include_hidden: bool,
    prefix: Option<String>,
}

#[derive(Debug, Serialize)]
struct DirEntryView {
    name: String,
    path: String,
    is_git_repo: bool,
    is_hidden: bool,
}

#[derive(Debug, Serialize)]
struct DirListing {
    current_path: String,
    parent_path: Option<String>,
    home_dir: Option<String>,
    cwd: String,
    exists: bool,
    is_dir: bool,
    /// False when the folder exists but may not be read.
    readable: bool,
    truncated: bool,
    entries: Vec<DirEntryView>,
}

fn list(params: ListParams) -> DirListing {
    let current = match params.path.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => expand_path(p),
        _ => home_dir().unwrap_or_else(cwd),
    };
    let meta = std::fs::metadata(&current).ok();
    let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
    let prefix = params.prefix.unwrap_or_default().to_lowercase();

    let reader = is_dir.then(|| std::fs::read_dir(&current).ok()).flatten();
    let readable = reader.is_some();
    // (lowercased name, name): only names are collected before the limit applies.
    let mut names: Vec<(String, String)> = reader
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let lower = name.to_lowercase();
            if (!params.include_hidden && name.starts_with('.')) || !lower.starts_with(&prefix) {
                return None;
            }
            let kind = entry.file_type().ok()?;
            // A symlink counts when it points at a directory.
            let dir = kind.is_dir() || (kind.is_symlink() && entry.path().is_dir());
            dir.then_some((lower, name))
        })
        .collect();
    names.sort();
    let truncated = names.len() > MAX_ENTRIES;
    names.truncate(MAX_ENTRIES);

    DirListing {
        parent_path: current.parent().map(text),
        home_dir: home_dir().as_deref().map(text),
        cwd: text(&cwd()),
        exists: meta.is_some(),
        is_dir,
        readable,
        truncated,
        entries: names
            .into_iter()
            .map(|(_, name)| {
                let path = current.join(&name);
                DirEntryView {
                    is_git_repo: is_git_repo(&path),
                    is_hidden: name.starts_with('.'),
                    path: text(&path),
                    name,
                }
            })
            .collect(),
        current_path: text(&current),
    }
}

/// Filesystem work stays off the async workers (a network mount may be slow).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))
}

async fn list_dir(Query(params): Query<ListParams>) -> ApiResult<DirListing> {
    Ok(Json(blocking(move || list(params)).await?))
}

// ----- recent workspaces ---------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct RecentView {
    path: String,
    starred: bool,
    use_count: i64,
    last_used_at: String,
    exists: bool,
    is_git_repo: bool,
}

fn view(w: RecentWorkspace) -> RecentView {
    let path = Path::new(&w.path);
    RecentView {
        exists: path.is_dir(),
        is_git_repo: is_git_repo(path),
        path: w.path,
        starred: w.starred,
        use_count: w.use_count,
        last_used_at: w.last_used_at,
    }
}

fn store(m: &AppState) -> &Arc<dyn WorkspaceStore> {
    &m.registry().workspaces
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
}

#[derive(Deserialize)]
struct StarBody {
    path: String,
    starred: bool,
}

/// The stored form of a path sent by a client.
fn key(raw: &str) -> Result<String, ApiError> {
    if raw.trim().is_empty() {
        return Err(ApiError::BadRequest("`path` is required".into()));
    }
    Ok(text(&expand_path(raw)))
}

/// Remembers a workspace that was just used. Never fails the caller: history is a convenience.
pub async fn remember(workspaces: &Arc<dyn WorkspaceStore>, raw: &str) {
    let path = expand_path(raw);
    if raw.trim().is_empty() || !path.is_dir() {
        return;
    }
    if let Err(e) = workspaces.record_workspace(&text(&path)).await {
        tracing::warn!(error = %e, "could not record the recent workspace");
    }
}

async fn list_recent(State(m): State<AppState>) -> ApiResult<Vec<RecentView>> {
    let rows = store(&m).recent_workspaces(MAX_RECENT).await?;
    Ok(Json(
        blocking(move || rows.into_iter().map(view).collect()).await?,
    ))
}

async fn add_recent(
    State(m): State<AppState>,
    Json(body): Json<PathBody>,
) -> ApiResult<RecentView> {
    let path = key(&body.path)?;
    if !Path::new(&path).is_dir() {
        return Err(ApiError::BadRequest(format!("{path} is not a directory")));
    }
    let row = store(&m).record_workspace(&path).await?;
    Ok(Json(view(row)))
}

async fn star_recent(
    State(m): State<AppState>,
    Json(body): Json<StarBody>,
) -> Result<StatusCode, ApiError> {
    store(&m)
        .star_workspace(&key(&body.path)?, body.starred)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_recent(
    State(m): State<AppState>,
    Json(body): Json<PathBody>,
) -> Result<StatusCode, ApiError> {
    store(&m).delete_workspace(&key(&body.path)?).await?;
    Ok(StatusCode::NO_CONTENT)
}
