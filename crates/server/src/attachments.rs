//! Image attachments: upload and download endpoints, binding them to messages, and turning the
//! references kept in history into Base64 right before a request leaves.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::extract::multipart::{Multipart, MultipartError};
use axum::extract::{DefaultBodyLimit, Json, Path, State};
use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use oort_agent::ModelClient;
use oort_llm::{ChatRequest, ChatStream, ContentPart, ImageSource, Message};
use oort_storage::attachments::{Attachment, AttachmentStore, NewAttachment, sniff_image};
use serde::Serialize;

use crate::api::ApiError;
use crate::manager::SessionManager;

pub const MAX_PER_MESSAGE: usize = 6;
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = MAX_PER_MESSAGE * MAX_FILE_BYTES + 1024 * 1024;
const MAX_FILENAME_CHARS: usize = 200;

/// Appended to a provider's rejection of a request that carried images (the web client matches
/// it to show a translated explanation).
pub const IMAGE_HINT: &str = "The current model may not support image input.";

#[derive(Debug, Serialize)]
pub struct AttachmentView {
    pub id: String,
    pub url: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub size_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl From<Attachment> for AttachmentView {
    fn from(a: Attachment) -> Self {
        Self {
            url: format!("/api/attachments/{}", a.id),
            id: a.id,
            mime_type: a.mime_type,
            filename: a.filename,
            size_bytes: a.size_bytes,
            width: a.width,
            height: a.height,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Uploaded {
    pub attachments: Vec<AttachmentView>,
}

pub(crate) fn routes() -> Router<Arc<SessionManager>> {
    Router::new()
        .route(
            "/attachments",
            post(upload).layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES)),
        )
        .route("/attachments/{id}", get(download))
}

fn bad_upload(e: MultipartError) -> ApiError {
    ApiError::BadRequest(format!("invalid upload: {}", e.body_text()))
}

/// The base name without control characters.
fn clean_filename(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = base
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_FILENAME_CHARS)
        .collect();
    Some(clean.trim().to_string()).filter(|n| !n.is_empty())
}

/// `multipart/form-data` with one or more `file` / `files` parts. Everything is checked before
/// anything is stored.
async fn upload(
    State(manager): State<Arc<SessionManager>>,
    headers: HeaderMap,
    mut form: Multipart,
) -> Result<Json<Uploaded>, ApiError> {
    // A multipart POST needs no CORS preflight, so refuse other sites explicitly.
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site")
    {
        return Err(ApiError::BadRequest("cross-site upload refused".into()));
    }
    let mut files = Vec::new();
    while let Some(field) = form.next_field().await.map_err(bad_upload)? {
        if !matches!(field.name(), Some("file" | "files" | "files[]")) {
            continue;
        }
        if files.len() == MAX_PER_MESSAGE {
            return Err(ApiError::BadRequest(format!(
                "at most {MAX_PER_MESSAGE} images per upload"
            )));
        }
        let filename = field.file_name().and_then(clean_filename);
        let bytes = field.bytes().await.map_err(bad_upload)?.to_vec();
        let label = filename.clone().unwrap_or_else(|| "file".into());
        if bytes.len() > MAX_FILE_BYTES {
            return Err(ApiError::BadRequest(format!(
                "{label}: larger than {} MB",
                MAX_FILE_BYTES / 1024 / 1024
            )));
        }
        if sniff_image(&bytes).is_none() {
            return Err(ApiError::BadRequest(format!(
                "{label}: only PNG, JPEG, WebP and GIF images are supported"
            )));
        }
        files.push(NewAttachment {
            session_id: None,
            bytes,
            filename,
        });
    }
    if files.is_empty() {
        return Err(ApiError::BadRequest("no `file` part in the upload".into()));
    }
    let store = &manager.registry().attachments;
    let mut attachments = Vec::with_capacity(files.len());
    for file in files {
        attachments.push(store.save_attachment(file).await?.into());
    }
    Ok(Json(Uploaded { attachments }))
}

async fn download(
    State(manager): State<Arc<SessionManager>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let store = &manager.registry().attachments;
    let attachment = store
        .get_attachment(&id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("attachment {id} not found")))?;
    let bytes = store.read_attachment(&attachment).await?;
    Ok((
        [
            (header::CONTENT_TYPE, attachment.mime_type),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".to_string(),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        bytes,
    ))
}

/// Count and duplicate checks of the ids sent with a message.
pub fn check_ids(ids: &[String]) -> Result<(), ApiError> {
    if ids.len() > MAX_PER_MESSAGE {
        return Err(ApiError::BadRequest(format!(
            "at most {MAX_PER_MESSAGE} images per message"
        )));
    }
    if ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id)) {
        return Err(ApiError::BadRequest("an attachment is listed twice".into()));
    }
    Ok(())
}

/// A user message: the images first, then the text (omitted when blank next to images).
pub fn user_message(attachments: &[Attachment], text: String) -> Message {
    let mut parts: Vec<ContentPart> = attachments
        .iter()
        .map(|a| ContentPart::Image {
            source: ImageSource::Attachment {
                attachment_id: a.id.clone(),
                media_type: a.mime_type.clone(),
                filename: a.filename.clone(),
                width: a.width,
                height: a.height,
            },
        })
        .collect();
    if parts.is_empty() || !text.trim().is_empty() {
        parts.push(ContentPart::text(text));
    }
    Message::user_parts(parts)
}

/// Replaces attachment references with the file's Base64 on a copy of the request, so history
/// and the database only ever hold the reference. The encoding is deterministic: earlier turns
/// reach the provider byte for byte as before.
pub struct AttachmentModel {
    inner: Arc<dyn ModelClient>,
    store: Arc<dyn AttachmentStore>,
}

impl AttachmentModel {
    pub fn wrap(
        inner: Arc<dyn ModelClient>,
        store: Arc<dyn AttachmentStore>,
    ) -> Arc<dyn ModelClient> {
        Arc::new(Self { inner, store })
    }

    /// Returns whether the request carries images.
    async fn inline(&self, request: &mut ChatRequest) -> Result<bool, oort_llm::Error> {
        let mut encoded: BTreeMap<String, String> = BTreeMap::new();
        let mut images = false;
        for part in request.messages.iter_mut().flat_map(|m| &mut m.content) {
            let ContentPart::Image { source } = part else {
                continue;
            };
            images = true;
            let ImageSource::Attachment {
                attachment_id,
                media_type,
                ..
            } = source
            else {
                continue;
            };
            let data = match encoded.get(attachment_id) {
                Some(data) => data.clone(),
                None => {
                    let data = STANDARD.encode(self.load(attachment_id).await?);
                    encoded.insert(attachment_id.clone(), data.clone());
                    data
                }
            };
            *source = ImageSource::Base64 {
                media_type: std::mem::take(media_type),
                data,
            };
        }
        Ok(images)
    }

    async fn load(&self, id: &str) -> Result<Vec<u8>, oort_llm::Error> {
        let missing = |why: String| oort_llm::Error::InvalidConfig(why);
        let attachment = self
            .store
            .get_attachment(id)
            .await
            .map_err(|e| missing(e.to_string()))?
            .ok_or_else(|| missing(format!("attachment {id} no longer exists")))?;
        self.store
            .read_attachment(&attachment)
            .await
            .map_err(|e| missing(e.to_string()))
    }
}

#[async_trait]
impl ModelClient for AttachmentModel {
    async fn stream(&self, mut request: ChatRequest) -> Result<ChatStream, oort_llm::Error> {
        let images = self.inline(&mut request).await?;
        self.inner.stream(request).await.map_err(|e| match e {
            oort_llm::Error::Api {
                status: status @ Some(400 | 415 | 422),
                message,
                body,
            } if images => oort_llm::Error::Api {
                status,
                message: format!("{message} — {IMAGE_HINT}"),
                body,
            },
            other => other,
        })
    }
}
