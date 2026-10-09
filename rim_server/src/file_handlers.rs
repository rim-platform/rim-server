use std::sync::Arc;

use actix_multipart::Multipart;
use actix_web::{HttpRequest, HttpResponse, web};
use futures_util::StreamExt;
use rim_core::storage::StorageBackend;
use rim_services::auth::service::RequestAuthenticator;
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::storage::service::StorageService;

use crate::graphql_support::{extract_actor_and_session_id, validate_csrf_header};

/// Map a ServiceError to an HTTP response for REST endpoints.
///
/// Shared by every non-GraphQL (FL2) transport in `rim_server` — file handlers and the
/// alert-ingest webhook handlers (`alert_handlers.rs`) — so REST error shapes stay consistent.
pub(crate) fn map_service_error_to_response(err: rim_services::ServiceError) -> HttpResponse {
    match err {
        rim_services::ServiceError::NotFound => {
            HttpResponse::NotFound().json(serde_json::json!({"error": "not found"}))
        }
        rim_services::ServiceError::PermissionDenied => {
            HttpResponse::Forbidden().json(serde_json::json!({"error": "access denied"}))
        }
        rim_services::ServiceError::Unauthenticated => HttpResponse::Unauthorized()
            .json(serde_json::json!({"error": "authentication required"})),
        rim_services::ServiceError::Validation(msg) => {
            HttpResponse::BadRequest().json(serde_json::json!({"error": msg}))
        }
        rim_services::ServiceError::Conflict(msg) => {
            HttpResponse::Conflict().json(serde_json::json!({"error": msg}))
        }
        rim_services::ServiceError::ConversationArchived => {
            HttpResponse::Conflict().json(serde_json::json!({"error": "conversation is archived"}))
        }
        // Track 1.2: the uploader is timed out in this workspace.
        rim_services::ServiceError::TimedOut { until } => {
            HttpResponse::Forbidden().json(serde_json::json!({
                "error": "timed out",
                "until": until.to_rfc3339(),
            }))
        }
        rim_services::ServiceError::RateLimited {
            retry_after_seconds,
        } => HttpResponse::TooManyRequests()
            .insert_header(("Retry-After", retry_after_seconds.to_string()))
            .json(serde_json::json!({
                "error": "rate limit exceeded",
                "retryAfterSeconds": retry_after_seconds,
            })),
        // Track 0.4: a disabled workspace module looks like an unknown route on REST
        // transports (bare 404, no body), matching the alert-webhook feature gate.
        rim_services::ServiceError::FeatureDisabled { .. } => HttpResponse::NotFound().finish(),
        _ => {
            tracing::error!(error = %err, "service error in file handler");
            HttpResponse::InternalServerError().json(serde_json::json!({"error": "internal error"}))
        }
    }
}

/// Maximum upload size: 25 MB, the same cap the attachment service puts on declared sizes.
const MAX_UPLOAD_BYTES: u64 = rim_services::message_attachment::MAX_ATTACHMENT_BYTES;

/// Maximum avatar size: 2 MB.
const MAX_AVATAR_BYTES: u64 = 2 * 1024 * 1024;

/// MIME types allowed for general file uploads.
const ALLOWED_MIME_PREFIXES: &[&str] = &[
    "image/",
    "text/",
    "application/pdf",
    "application/json",
    "application/zip",
    "application/gzip",
];

/// MIME types allowed for avatar uploads.
const ALLOWED_AVATAR_MIMES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

use rim_core::util::sanitize_filename;

fn is_mime_allowed(content_type: &str, allowed: &[&str]) -> bool {
    let ct = content_type.to_lowercase();
    allowed.iter().any(|prefix| ct.starts_with(prefix))
}

/// Enforce CSRF validation for REST endpoints. Returns 403 on failure.
pub(crate) fn require_csrf(req: &HttpRequest) -> Result<(), HttpResponse> {
    validate_csrf_header(req)
        .map_err(|msg| HttpResponse::Forbidden().json(serde_json::json!({"error": msg})))
}

/// Require an authenticated user from the session cookie. Returns user_id or 401.
pub(crate) async fn require_auth(
    req: &HttpRequest,
) -> Result<rim_core::user::model::UserId, HttpResponse> {
    let authenticator = req
        .app_data::<web::Data<Arc<dyn RequestAuthenticator>>>()
        .map(|d| Arc::clone(d.get_ref()));

    let authenticator = authenticator.ok_or_else(|| {
        HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": "auth not configured"}))
    })?;

    let (actor, _session_id) = extract_actor_and_session_id(req, authenticator.as_ref())
        .await
        .map_err(|_| {
            HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "auth check failed"}))
        })?;

    actor.ok_or_else(|| {
        HttpResponse::Unauthorized().json(serde_json::json!({"error": "authentication required"}))
    })
}

/// Parsed multipart upload result.
struct MultipartUpload {
    filename: String,
    content_type: String,
    data: Vec<u8>,
    upload_token: Option<String>,
    attachment_id: Option<String>,
}

/// Read fields from a multipart upload, enforcing size limits.
/// Extracts `upload_token`, `attachment_id` text fields and exactly one file field.
async fn read_multipart_fields(
    mut payload: Multipart,
    max_bytes: u64,
) -> Result<MultipartUpload, HttpResponse> {
    let mut upload_token: Option<String> = None;
    let mut attachment_id: Option<String> = None;
    let mut file_result: Option<(String, String, Vec<u8>)> = None;

    while let Some(field) = payload.next().await {
        let Ok(mut field) = field else {
            return Err(HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "malformed multipart field"})));
        };

        let field_name = field
            .content_disposition()
            .and_then(|cd| cd.get_name().map(|s| s.to_string()));

        let is_file = field
            .content_disposition()
            .and_then(|cd| cd.get_filename().map(|s| s.to_string()))
            .is_some();

        if !is_file {
            // Text field — read value.
            let name = field_name.as_deref();
            if name == Some("upload_token") || name == Some("attachment_id") {
                let mut buf = Vec::new();
                while let Some(chunk) = field.next().await {
                    let chunk = chunk.map_err(|e| {
                        HttpResponse::BadRequest()
                            .json(serde_json::json!({"error": format!("read error: {e}")}))
                    })?;
                    buf.extend_from_slice(&chunk);
                    if buf.len() > 256 {
                        return Err(HttpResponse::BadRequest()
                            .json(serde_json::json!({"error": "field value too long"})));
                    }
                }
                let value = String::from_utf8_lossy(&buf).trim().to_string();
                if name == Some("upload_token") {
                    upload_token = Some(value);
                } else {
                    attachment_id = Some(value);
                }
            }
            continue;
        }

        // File field.
        if file_result.is_some() {
            return Err(HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "only one file allowed per upload"})));
        }

        let content_type = field
            .content_type()
            .map(|ct| ct.to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let filename = field
            .content_disposition()
            .and_then(|cd| cd.get_filename().map(|s| s.to_string()))
            .unwrap_or_else(|| "upload".to_string());

        let filename = sanitize_filename(&filename).ok_or_else(|| {
            HttpResponse::BadRequest().json(serde_json::json!({"error": "invalid filename"}))
        })?;

        let mut data = Vec::new();
        while let Some(chunk) = field.next().await {
            let chunk = chunk.map_err(|e| {
                HttpResponse::BadRequest()
                    .json(serde_json::json!({"error": format!("read error: {e}")}))
            })?;
            data.extend_from_slice(&chunk);
            if data.len() as u64 > max_bytes {
                return Err(HttpResponse::PayloadTooLarge().json(serde_json::json!({
                    "error": format!("file exceeds {} byte limit", max_bytes)
                })));
            }
        }

        file_result = Some((filename, content_type, data));
    }

    let (filename, content_type, data) = file_result.ok_or_else(|| {
        HttpResponse::BadRequest().json(serde_json::json!({"error": "no file in upload"}))
    })?;

    Ok(MultipartUpload {
        filename,
        content_type,
        data,
        upload_token,
        attachment_id,
    })
}

/// POST /api/files/upload
pub async fn handle_file_upload(
    req: HttpRequest,
    payload: Multipart,
    attachment_service: web::Data<Arc<dyn MessageAttachmentService>>,
) -> HttpResponse {
    if let Err(resp) = require_csrf(&req) {
        return resp;
    }

    let user_id = match require_auth(&req).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let upload = match read_multipart_fields(payload, MAX_UPLOAD_BYTES).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if !is_mime_allowed(&upload.content_type, ALLOWED_MIME_PREFIXES) {
        return HttpResponse::UnsupportedMediaType()
            .json(serde_json::json!({"error": "file type not allowed"}));
    }

    let upload_token = match upload.upload_token {
        Some(t) if !t.is_empty() => t,
        _ => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "upload_token is required"}));
        }
    };

    let attachment_id_str = match upload.attachment_id {
        Some(id) if !id.is_empty() => id,
        _ => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "attachment_id is required"}));
        }
    };

    let attachment_id = match attachment_id_str
        .parse::<uuid::Uuid>()
        .map(rim_core::message_attachment::model::MessageAttachmentId::from)
    {
        Ok(id) => id,
        Err(_) => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "invalid attachment_id"}));
        }
    };

    // The service checks the token, ownership and declared size before it writes anything.
    let cmd = rim_services::message_attachment::command::CompleteUploadCmd {
        actor_user_id: user_id,
        attachment_id,
        upload_token,
        uploaded_filename: upload.filename,
        content_type: upload.content_type,
        data: upload.data,
    };
    let attachment = match attachment_service.complete_upload(cmd).await {
        Ok(attachment) => attachment,
        Err(e) => {
            tracing::error!(error = %e, "failed to complete upload via service");
            return map_service_error_to_response(e);
        }
    };
    let Some(storage_key) = attachment.storage_key else {
        tracing::error!(attachment_id = %attachment.id, "completed upload has no storage key");
        return HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": "upload failed"}));
    };

    HttpResponse::Ok().json(serde_json::json!({
        "path": storage_key,
        "size_bytes": attachment.size_bytes,
        "url": format!("/api/files/{}", storage_key),
    }))
}

/// GET /api/files/{path:.*}
pub async fn handle_file_download(
    req: HttpRequest,
    path: web::Path<String>,
    storage_backend: web::Data<Arc<dyn StorageBackend>>,
) -> HttpResponse {
    let file_path = path.into_inner();

    // Legacy uploads endpoint — clients should use /api/attachments/{id}/download instead.
    if file_path.starts_with("uploads/") {
        return HttpResponse::Gone()
            .json(serde_json::json!({"error": "use /api/attachments/{id}/download"}));
    }

    // Avatars are publicly accessible (only require authentication).
    if file_path.starts_with("avatars/") {
        // Avatars are public — just require authentication.
        if let Err(resp) = require_auth(&req).await {
            return resp;
        }
    } else {
        // Unknown path prefix — require auth and deny.
        if let Err(resp) = require_auth(&req).await {
            return resp;
        }
        return HttpResponse::Forbidden().json(serde_json::json!({"error": "access denied"}));
    }

    let data = match storage_backend.get(&file_path).await {
        Ok(d) => d,
        Err(rim_core::storage::StorageError::NotFound(_)) => {
            return HttpResponse::NotFound().json(serde_json::json!({"error": "file not found"}));
        }
        Err(e) => {
            tracing::error!(error = %e, "file download failed");
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "download failed"}));
        }
    };

    let content_type = mime_guess::from_path(&file_path)
        .first_or_octet_stream()
        .to_string();

    let filename = file_path.rsplit('/').next().unwrap_or("download");
    let safe_name = filename.replace(['\0', '"', '\\', '\r', '\n'], "");

    HttpResponse::Ok()
        .content_type(content_type)
        .insert_header((
            "Content-Disposition",
            format!("attachment; filename=\"{}\"", safe_name),
        ))
        .body(data)
}

/// GET /api/attachments/{attachment_id}/download
///
/// Serves an attachment file after verifying that the requesting user is a member
/// of the conversation the attachment belongs to. Authorization is handled by the
/// service layer.
pub async fn handle_attachment_download(
    req: HttpRequest,
    path: web::Path<String>,
    attachment_service: web::Data<Arc<dyn MessageAttachmentService>>,
    storage_backend: web::Data<Arc<dyn StorageBackend>>,
) -> HttpResponse {
    let user_id = match require_auth(&req).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let attachment_id = match path
        .into_inner()
        .parse::<uuid::Uuid>()
        .map(rim_core::message_attachment::model::MessageAttachmentId::from)
    {
        Ok(id) => id,
        Err(_) => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "invalid attachment id"}));
        }
    };

    // Authorize access and get attachment details via the service layer.
    let attachment_dto = match attachment_service
        .authorize_download(
            rim_services::message_attachment::command::AuthorizeDownloadQuery {
                actor_user_id: user_id,
                attachment_id,
            },
        )
        .await
    {
        Ok(dto) => dto,
        Err(e) => return map_service_error_to_response(e),
    };

    // Check storage key is available.
    let storage_key = match &attachment_dto.storage_key {
        Some(key) => key.clone(),
        None => {
            return HttpResponse::NotFound()
                .json(serde_json::json!({"error": "attachment not yet uploaded"}));
        }
    };

    // Fetch the file from storage.
    let data = match storage_backend.get(&storage_key).await {
        Ok(d) => d,
        Err(rim_core::storage::StorageError::NotFound(_)) => {
            return HttpResponse::NotFound().json(serde_json::json!({"error": "file not found"}));
        }
        Err(e) => {
            tracing::error!(error = %e, "file download failed");
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "download failed"}));
        }
    };

    let content_type = &attachment_dto.content_type;
    let safe_name = attachment_dto
        .filename
        .replace(['\0', '"', '\\', '\r', '\n'], "");

    HttpResponse::Ok()
        .content_type(content_type.as_str())
        .insert_header((
            "Content-Disposition",
            format!("attachment; filename=\"{}\"", safe_name),
        ))
        .body(data)
}

/// POST /api/files/avatar
pub async fn handle_avatar_upload(
    req: HttpRequest,
    payload: Multipart,
    storage_service: web::Data<Arc<dyn StorageService>>,
) -> HttpResponse {
    if let Err(resp) = require_csrf(&req) {
        return resp;
    }

    let user_id = match require_auth(&req).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let upload = match read_multipart_fields(payload, MAX_AVATAR_BYTES).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if !is_mime_allowed(&upload.content_type, ALLOWED_AVATAR_MIMES) {
        return HttpResponse::UnsupportedMediaType()
            .json(serde_json::json!({"error": "avatar must be an image (png, jpeg, gif, webp)"}));
    }

    // Extension from the vetted content type, never the client filename, so each user has at
    // most one avatar file per allowed type instead of one per invented extension.
    let content_type = upload.content_type.to_lowercase();
    let ext = ALLOWED_AVATAR_MIMES
        .iter()
        .find(|mime| content_type.starts_with(**mime))
        .and_then(|mime| mime.strip_prefix("image/"))
        .unwrap_or("png");
    let storage_key = format!("avatars/{}/avatar.{}", user_id, ext);

    match storage_service
        .upload_file(&storage_key, &upload.data, &upload.content_type)
        .await
    {
        Ok(stored) => HttpResponse::Ok().json(serde_json::json!({
            "path": stored.key,
            "size_bytes": stored.size_bytes,
            "url": format!("/api/files/{}", stored.key),
        })),
        Err(e) => {
            tracing::error!(error = %e, "avatar upload failed");
            HttpResponse::InternalServerError().json(serde_json::json!({"error": "upload failed"}))
        }
    }
}
