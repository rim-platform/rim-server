use std::sync::Arc;

use actix_multipart::Multipart;
use actix_web::{HttpRequest, HttpResponse, web};
use futures_util::StreamExt;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::message::repository::MessageRepository;
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::storage::StorageBackend;
use rim_services::auth::service::RequestAuthenticator;
use rim_services::storage::service::StorageService;

use crate::graphql_support::{extract_actor_and_session_id, validate_csrf_header};

/// Maximum upload size: 25 MB.
const MAX_UPLOAD_BYTES: u64 = 25 * 1024 * 1024;

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
fn require_csrf(req: &HttpRequest) -> Result<(), HttpResponse> {
    validate_csrf_header(req)
        .map_err(|msg| HttpResponse::Forbidden().json(serde_json::json!({"error": msg})))
}

/// Require an authenticated user from the session cookie. Returns user_id or 401.
async fn require_auth(req: &HttpRequest) -> Result<rim_core::user::model::UserId, HttpResponse> {
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
}

/// Read fields from a multipart upload, enforcing size limits.
/// Extracts an optional `upload_token` text field and exactly one file field.
async fn read_multipart_fields(
    mut payload: Multipart,
    max_bytes: u64,
) -> Result<MultipartUpload, HttpResponse> {
    let mut upload_token: Option<String> = None;
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
            if field_name.as_deref() == Some("upload_token") {
                let mut buf = Vec::new();
                while let Some(chunk) = field.next().await {
                    let chunk = chunk.map_err(|e| {
                        HttpResponse::BadRequest()
                            .json(serde_json::json!({"error": format!("read error: {e}")}))
                    })?;
                    buf.extend_from_slice(&chunk);
                    if buf.len() > 256 {
                        return Err(HttpResponse::BadRequest()
                            .json(serde_json::json!({"error": "upload_token too long"})));
                    }
                }
                upload_token = Some(String::from_utf8_lossy(&buf).trim().to_string());
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
    })
}

/// POST /api/files/upload
pub async fn handle_file_upload(
    req: HttpRequest,
    payload: Multipart,
    storage_service: web::Data<Arc<dyn StorageService>>,
    attachment_repo: web::Data<
        Arc<dyn rim_core::message_attachment::repository::MessageAttachmentRepository>,
    >,
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

    // Validate upload token against a pending attachment.
    let upload_token = match upload.upload_token {
        Some(t) if !t.is_empty() => t,
        _ => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "upload_token is required"}));
        }
    };

    // Token format: "upload-{attachment_id}"
    let attachment_id_str = match upload_token.strip_prefix("upload-") {
        Some(id) => id,
        None => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "invalid upload token format"}));
        }
    };

    let attachment_id = match attachment_id_str
        .parse::<uuid::Uuid>()
        .map(rim_core::message_attachment::model::MessageAttachmentId::from)
    {
        Ok(id) => id,
        Err(_) => {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({"error": "invalid upload token"}));
        }
    };

    let attachment = match attachment_repo.get_by_id(attachment_id).await {
        Ok(Some(att)) => att,
        Ok(None) => {
            return HttpResponse::NotFound()
                .json(serde_json::json!({"error": "upload token not found"}));
        }
        Err(e) => {
            tracing::error!(error = %e, "attachment lookup failed");
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "upload validation failed"}));
        }
    };

    // Verify the attachment is in pending state with matching token.
    let expected_storage_url = format!("pending://{}", upload_token);
    if attachment.storage_url != expected_storage_url {
        return HttpResponse::BadRequest()
            .json(serde_json::json!({"error": "upload token already used or invalid"}));
    }

    // Check that attachment metadata shows pending_upload status.
    if let Some(ref metadata) = attachment.metadata
        && metadata.get("status").and_then(|v| v.as_str()) != Some("pending_upload")
    {
        return HttpResponse::BadRequest()
            .json(serde_json::json!({"error": "attachment is not pending upload"}));
    }

    // Validate MIME type matches the declared content_type.
    if !attachment.mime_type.is_empty()
        && upload.content_type.to_lowercase() != attachment.mime_type.to_lowercase()
    {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "file content type does not match declared type"
        }));
    }

    // Validate file size matches the declared size.
    if attachment.file_size > 0 && upload.data.len() as i64 != attachment.file_size {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "error": "file size does not match declared size"
        }));
    }

    let unique_id = uuid::Uuid::now_v7();
    let storage_key = format!("uploads/{}/{}_{}", user_id, unique_id, upload.filename);

    let stored = match storage_service
        .upload_file(&storage_key, &upload.data, &upload.content_type)
        .await
    {
        Ok(stored) => stored,
        Err(e) => {
            tracing::error!(error = %e, "file upload failed");
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "upload failed"}));
        }
    };

    // Bug 2 fix: Update the attachment in DB after successful upload.
    let uploaded_metadata = serde_json::json!({
        "status": "uploaded",
        "storage_filename": upload.filename,
    });
    if let Err(e) = attachment_repo
        .update_attachment(attachment_id, stored.key.clone(), Some(uploaded_metadata))
        .await
    {
        tracing::error!(error = %e, "failed to update attachment after upload");
        return HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": "failed to update attachment record"}));
    }

    HttpResponse::Ok().json(serde_json::json!({
        "path": stored.key,
        "size_bytes": stored.size_bytes,
        "url": format!("/api/files/{}", stored.key),
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
    let safe_name = filename
        .replace(['\0', '"', '\\', '\r', '\n'], "");

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
/// of the conversation the attachment belongs to. This replaces direct storage-path
/// based access for uploaded files.
pub async fn handle_attachment_download(
    req: HttpRequest,
    path: web::Path<String>,
    attachment_repo: web::Data<Arc<dyn MessageAttachmentRepository>>,
    message_repo: web::Data<Arc<dyn MessageRepository>>,
    conversation_member_repo: web::Data<Arc<dyn ConversationMemberRepository>>,
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

    // Look up the attachment.
    let attachment = match attachment_repo.get_by_id(attachment_id).await {
        Ok(Some(att)) => att,
        Ok(None) => {
            return HttpResponse::NotFound()
                .json(serde_json::json!({"error": "attachment not found"}));
        }
        Err(e) => {
            tracing::error!(error = %e, "attachment lookup failed");
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "internal error"}));
        }
    };

    // Look up the message to find the conversation.
    let message = match message_repo.get_by_id(attachment.message_id).await {
        Ok(Some(msg)) => msg,
        Ok(None) => {
            tracing::error!(
                attachment_id = %attachment_id,
                message_id = %attachment.message_id,
                "attachment references missing message"
            );
            return HttpResponse::NotFound()
                .json(serde_json::json!({"error": "attachment not found"}));
        }
        Err(e) => {
            tracing::error!(error = %e, "message lookup failed");
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "internal error"}));
        }
    };

    // Verify the requesting user can access this conversation.
    // Channels only require server membership (no conversation_member rows).
    // DMs/Groups require explicit conversation membership.
    let conversation_repo = req
        .app_data::<web::Data<Arc<dyn rim_core::conversation::repository::ConversationRepository>>>()
        .map(|d| Arc::clone(d.get_ref()));
    let conversation_repo = match conversation_repo {
        Some(r) => r,
        None => {
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "internal error"}));
        }
    };
    let conversation = match conversation_repo.get_by_id(message.conversation_id).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return HttpResponse::NotFound()
                .json(serde_json::json!({"error": "conversation not found"}));
        }
        Err(e) => {
            tracing::error!(error = %e, "conversation lookup failed");
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "internal error"}));
        }
    };

    // Channels: require server membership
    // DMs/Groups: must be explicit conversation member
    if conversation.conversation_type == rim_core::conversation::model::ConversationType::Channel {
        let server_member_repo = req
            .app_data::<web::Data<Arc<dyn rim_core::server_member::repository::ServerMemberRepository>>>()
            .map(|d| Arc::clone(d.get_ref()));
        let server_member_repo = match server_member_repo {
            Some(r) => r,
            None => {
                return HttpResponse::InternalServerError()
                    .json(serde_json::json!({"error": "internal error"}));
            }
        };
        match server_member_repo
            .is_member(conversation.server_id, user_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return HttpResponse::Forbidden()
                    .json(serde_json::json!({"error": "access denied"}));
            }
            Err(e) => {
                tracing::error!(error = %e, "server membership check failed");
                return HttpResponse::InternalServerError()
                    .json(serde_json::json!({"error": "internal error"}));
            }
        }
    } else {
        match conversation_member_repo
            .is_member(message.conversation_id, user_id)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return HttpResponse::Forbidden()
                    .json(serde_json::json!({"error": "access denied"}));
            }
            Err(e) => {
                tracing::error!(error = %e, "membership check failed");
                return HttpResponse::InternalServerError()
                    .json(serde_json::json!({"error": "internal error"}));
            }
        }
    }

    // Resolve the storage key from the attachment's storage_url.
    let storage_key = &attachment.storage_url;
    if storage_key.starts_with("pending://") || storage_key.is_empty() {
        return HttpResponse::NotFound()
            .json(serde_json::json!({"error": "attachment not yet uploaded"}));
    }

    // Fetch the file from storage.
    let data = match storage_backend.get(storage_key).await {
        Ok(d) => d,
        Err(rim_core::storage::StorageError::NotFound(_)) => {
            return HttpResponse::NotFound()
                .json(serde_json::json!({"error": "file not found"}));
        }
        Err(e) => {
            tracing::error!(error = %e, "file download failed");
            return HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "download failed"}));
        }
    };

    let content_type = if attachment.mime_type.is_empty() {
        mime_guess::from_path(&attachment.file_name)
            .first_or_octet_stream()
            .to_string()
    } else {
        attachment.mime_type
    };

    let safe_name = attachment
        .file_name
        .replace(['\0', '"', '\\', '\r', '\n'], "");

    HttpResponse::Ok()
        .content_type(content_type)
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

    let ext = upload.filename.rsplit('.').next().unwrap_or("png");
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
