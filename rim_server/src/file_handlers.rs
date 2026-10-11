use std::sync::Arc;

use actix_multipart::Multipart;
use actix_web::http::header;
use actix_web::{HttpRequest, HttpResponse, web};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::StreamExt;
use rim_core::server_feature::model::ServerFeature;
use rim_core::storage::StorageBackend;
use rim_services::auth::service::RequestAuthenticator;
use rim_services::feature::service::FeatureGate;
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::moderation::command::AuthorizeReportAttachmentQuery;
use rim_services::moderation::service::ModerationService;
use rim_services::storage::service::StorageService;

use crate::graphql_support::{extract_auth, validate_csrf_header};

/// Map a ServiceError to an HTTP response for REST endpoints.
///
/// Shared by the non-GraphQL (FL2) file transports in `rim_server` so REST error shapes stay
/// consistent.
pub(crate) fn map_service_error_to_response(err: rim_services::ServiceError) -> HttpResponse {
    match err {
        rim_services::ServiceError::NotFound => {
            HttpResponse::NotFound().json(serde_json::json!({"error": "not found"}))
        }
        rim_services::ServiceError::PermissionDenied => {
            HttpResponse::Forbidden().json(serde_json::json!({"error": "access denied"}))
        }
        rim_services::ServiceError::Forbidden(msg) => {
            HttpResponse::Forbidden().json(serde_json::json!({"error": msg}))
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
        // The uploader is timed out in this workspace.
        rim_services::ServiceError::TimedOut { until } => {
            HttpResponse::Forbidden().json(serde_json::json!({
                "error": "timed out",
                "until": until.to_rfc3339(),
            }))
        }
        rim_services::ServiceError::FeatureDisabled(feature) => {
            HttpResponse::Forbidden().json(serde_json::json!({
                "error": format!("the {feature} feature is not available on this server"),
                "code": "FEATURE_DISABLED",
                "feature": feature.as_str(),
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

/// The type of an encrypted conversation's blob. The service accepts it only for an attachment
/// created with a `blobSha256`, and checks the bytes against it.
const ENCRYPTED_BLOB_MIME: &str = "application/octet-stream";

/// The file name every encrypted blob is downloaded under; the real one is in the message.
const ENCRYPTED_BLOB_FILENAME: &str = "encrypted.bin";

/// MIME types allowed for avatar uploads.
const ALLOWED_AVATAR_MIMES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

use rim_core::util::sanitize_filename;

fn is_mime_allowed(content_type: &str, allowed: &[&str]) -> bool {
    let ct = content_type.to_lowercase();
    allowed.iter().any(|prefix| ct.starts_with(prefix))
}

/// Refuse a route of a feature that is not enabled on this instance: 403 with the
/// `FEATURE_DISABLED` code.
pub(crate) fn require_feature(
    features: &dyn FeatureGate,
    feature: ServerFeature,
) -> Result<(), HttpResponse> {
    features
        .require(feature)
        .map_err(map_service_error_to_response)
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

    let auth = extract_auth(req, authenticator.as_ref())
        .await
        .map_err(|_| {
            HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "auth check failed"}))
        })?;

    auth.user_id.ok_or_else(|| {
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
    features: web::Data<Arc<dyn FeatureGate>>,
) -> HttpResponse {
    if let Err(resp) = require_feature(features.get_ref().as_ref(), ServerFeature::Attachments) {
        return resp;
    }
    if let Err(resp) = require_csrf(&req) {
        return resp;
    }

    let user_id = match require_auth(&req).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    upload_as(user_id, payload, attachment_service.get_ref().as_ref()).await
}

/// Complete an attachment upload for `user_id`: the session path above and forwarded requests
/// from another relay share it.
pub(crate) async fn upload_as(
    user_id: rim_core::user::model::UserId,
    payload: Multipart,
    attachment_service: &dyn MessageAttachmentService,
) -> HttpResponse {
    let upload = match read_multipart_fields(payload, MAX_UPLOAD_BYTES).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    if !is_mime_allowed(&upload.content_type, ALLOWED_MIME_PREFIXES)
        && !is_mime_allowed(&upload.content_type, &[ENCRYPTED_BLOB_MIME])
    {
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
    match attachment_service.complete_upload(cmd).await {
        Ok(attachment) => HttpResponse::Ok().json(serde_json::json!({
            "size_bytes": attachment.size_bytes,
        })),
        Err(e) => {
            tracing::error!(error = %e, "failed to complete upload via service");
            map_service_error_to_response(e)
        }
    }
}

/// GET /api/files/{path:.*}
pub async fn handle_file_download(
    req: HttpRequest,
    path: web::Path<String>,
    storage_backend: web::Data<Arc<dyn StorageBackend>>,
) -> HttpResponse {
    if let Err(resp) = require_auth(&req).await {
        return resp;
    }
    file_download_as(&path.into_inner(), storage_backend.get_ref().as_ref()).await
}

/// Serve a stored file to an authenticated user: avatars only. The session path above and
/// forwarded requests from another relay share it.
pub(crate) async fn file_download_as(
    file_path: &str,
    storage_backend: &dyn StorageBackend,
) -> HttpResponse {
    // Avatars are readable by every authenticated user; any other prefix is denied.
    if !file_path.starts_with("avatars/") {
        return HttpResponse::Forbidden().json(serde_json::json!({"error": "access denied"}));
    }

    let data = match read_stored_file(storage_backend, file_path).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    let content_type = mime_guess::from_path(file_path)
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
    features: web::Data<Arc<dyn FeatureGate>>,
) -> HttpResponse {
    if let Err(resp) = require_feature(features.get_ref().as_ref(), ServerFeature::Attachments) {
        return resp;
    }
    let user_id = match require_auth(&req).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    attachment_download_as(
        user_id,
        &path.into_inner(),
        attachment_service.get_ref().as_ref(),
        storage_backend.get_ref().as_ref(),
    )
    .await
}

/// Serve an attachment to `user_id` if they may read it: the session path above and forwarded
/// requests from another relay share it.
pub(crate) async fn attachment_download_as(
    user_id: rim_core::user::model::UserId,
    attachment_id: &str,
    attachment_service: &dyn MessageAttachmentService,
    storage_backend: &dyn StorageBackend,
) -> HttpResponse {
    let attachment_id = match attachment_id
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

    let data = match read_stored_file(storage_backend, &storage_key).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    // An encrypted blob is served as opaque bytes; the client checks its hash and decrypts it.
    let (content_type, filename) = match attachment_dto.blob_sha256 {
        Some(_) => (ENCRYPTED_BLOB_MIME, ENCRYPTED_BLOB_FILENAME),
        None => (
            attachment_dto.content_type.as_str(),
            attachment_dto.filename.as_str(),
        ),
    };
    let safe_name = filename.replace(['\0', '"', '\\', '\r', '\n'], "");

    HttpResponse::Ok()
        .content_type(content_type)
        .insert_header((
            "Content-Disposition",
            format!("attachment; filename=\"{}\"", safe_name),
        ))
        .body(data)
}

/// A stored file's bytes, or the 404/500 response.
async fn read_stored_file(
    storage_backend: &dyn StorageBackend,
    storage_key: &str,
) -> Result<Vec<u8>, HttpResponse> {
    match storage_backend.get(storage_key).await {
        Ok(d) => Ok(d),
        Err(rim_core::storage::StorageError::NotFound(_)) => {
            Err(HttpResponse::NotFound().json(serde_json::json!({"error": "file not found"})))
        }
        Err(e) => {
            tracing::error!(error = %e, "file download failed");
            Err(HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "download failed"})))
        }
    }
}

/// GET /api/reports/{report_id}/attachments/{blob_sha256}
///
/// Serves the encrypted blob of an attachment of a reported encrypted message to an
/// Owner/Moderator of the report's workspace, who decrypts it with the secret from the
/// report's verified content. `blob_sha256` is unpadded base64url. Every refusal is a bare 404.
pub async fn handle_report_attachment_download(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    moderation_service: web::Data<Arc<dyn ModerationService>>,
    storage_backend: web::Data<Arc<dyn StorageBackend>>,
) -> HttpResponse {
    let user_id = match require_auth(&req).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let (report_id, blob_sha256) = path.into_inner();
    let Ok(report_id) = report_id.parse::<uuid::Uuid>() else {
        return HttpResponse::NotFound().finish();
    };
    let Ok(blob_sha256) = URL_SAFE_NO_PAD.decode(blob_sha256) else {
        return HttpResponse::NotFound().finish();
    };
    let attachment = match moderation_service
        .authorize_report_attachment(AuthorizeReportAttachmentQuery {
            actor_user_id: user_id,
            report_id: report_id.into(),
            blob_sha256,
        })
        .await
    {
        Ok(attachment) => attachment,
        Err(rim_services::ServiceError::NotFound) => return HttpResponse::NotFound().finish(),
        Err(e) => return map_service_error_to_response(e),
    };
    let data =
        match read_stored_file(storage_backend.get_ref().as_ref(), &attachment.storage_key).await {
            Ok(d) => d,
            Err(resp) => return resp,
        };
    HttpResponse::Ok()
        .content_type(ENCRYPTED_BLOB_MIME)
        .insert_header((header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .insert_header((
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{ENCRYPTED_BLOB_FILENAME}\""),
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
    avatar_upload_as(user_id, payload, storage_service.get_ref().as_ref()).await
}

/// Store `user_id`'s avatar: the session path above and forwarded requests from another relay
/// share it.
pub(crate) async fn avatar_upload_as(
    user_id: rim_core::user::model::UserId,
    payload: Multipart,
    storage_service: &dyn StorageService,
) -> HttpResponse {
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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::App;
    use actix_web::cookie::Cookie;
    use actix_web::test as actix_test;
    use rim_core::auth::model::SessionId;
    use rim_core::moderation::model::MessageReportId;
    use rim_core::storage::{StorageError, StoredFile};
    use rim_core::user::model::UserId;
    use rim_services::auth::dto::RequestAuthResultDto;
    use rim_services::auth::service::MockRequestAuthenticator;
    use rim_services::moderation::dto::ReportAttachmentDto;
    use rim_services::moderation::service::MockModerationService;

    use crate::graphql_support::COOKIE_SESSION;

    const SESSION: &str = "rim_sess_test";
    const BLOB: &[u8] = b"opaque blob";
    /// Unpadded base64url of 32 bytes of 0x07.
    const BLOB_SHA256: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc";

    /// Storage holding one file, `uploads/blob`.
    struct OneFile;

    #[async_trait::async_trait]
    impl StorageBackend for OneFile {
        async fn put(&self, _: &str, _: &[u8], _: &str) -> Result<StoredFile, StorageError> {
            Err(StorageError::NotFound("read only".to_string()))
        }

        async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
            if key == "uploads/blob" {
                Ok(BLOB.to_vec())
            } else {
                Err(StorageError::NotFound(key.to_string()))
            }
        }

        async fn delete(&self, _: &str) -> Result<(), StorageError> {
            Ok(())
        }
    }

    /// An authenticator that knows one session, of `user_id`.
    fn authenticator(user_id: UserId) -> MockRequestAuthenticator {
        let mut auth = MockRequestAuthenticator::new();
        auth.expect_authenticate_session_token()
            .returning(move |token| {
                let known = token == Some(SESSION);
                Box::pin(async move {
                    Ok(if known {
                        RequestAuthResultDto {
                            user_id: Some(user_id),
                            session_id: Some(SessionId::new()),
                            device_key_id: None,
                        }
                    } else {
                        RequestAuthResultDto::default()
                    })
                })
            });
        auth
    }

    macro_rules! test_app {
        ($auth:expr, $service:expr) => {
            actix_test::init_service(
                App::new()
                    .service(
                        web::resource("/api/reports/{report_id}/attachments/{blob_sha256}")
                            .route(web::get().to(handle_report_attachment_download)),
                    )
                    .app_data(web::Data::new(
                        Arc::new($auth) as Arc<dyn RequestAuthenticator>
                    ))
                    .app_data(web::Data::new(
                        Arc::new($service) as Arc<dyn ModerationService>
                    ))
                    .app_data(web::Data::new(Arc::new(OneFile) as Arc<dyn StorageBackend>)),
            )
            .await
        };
    }

    fn get(path: &str, signed_in: bool) -> actix_test::TestRequest {
        let req = actix_test::TestRequest::get().uri(path);
        if signed_in {
            req.cookie(Cookie::new(COOKIE_SESSION, SESSION))
        } else {
            req
        }
    }

    #[actix_web::test]
    async fn a_reported_blob_is_served_opaque() {
        let user_id = UserId::new();
        let report_id = MessageReportId::new();
        let mut service = MockModerationService::new();
        service
            .expect_authorize_report_attachment()
            .withf(move |q| {
                q.actor_user_id == user_id && q.report_id == report_id && q.blob_sha256 == [7; 32]
            })
            .returning(|_| {
                Box::pin(async {
                    Ok(ReportAttachmentDto {
                        storage_key: "uploads/blob".to_string(),
                    })
                })
            });
        let app = test_app!(authenticator(user_id), service);

        let path = format!("/api/reports/{report_id}/attachments/{BLOB_SHA256}");
        let resp = actix_test::call_service(&app, get(&path, true).to_request()).await;
        assert_eq!(resp.status(), 200);
        let headers = resp.headers();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).expect("header"),
            "application/octet-stream"
        );
        assert_eq!(
            headers.get(header::X_CONTENT_TYPE_OPTIONS).expect("header"),
            "nosniff"
        );
        assert_eq!(
            headers.get(header::CONTENT_DISPOSITION).expect("header"),
            "attachment; filename=\"encrypted.bin\""
        );
        assert_eq!(actix_test::read_body(resp).await, BLOB);
    }

    #[actix_web::test]
    async fn every_refusal_is_a_bare_404_and_no_session_a_401() {
        let report_id = MessageReportId::new();
        let mut service = MockModerationService::new();
        service
            .expect_authorize_report_attachment()
            .times(1)
            .returning(|_| Box::pin(async { Err(rim_services::ServiceError::NotFound) }));
        let app = test_app!(authenticator(UserId::new()), service);

        let path = format!("/api/reports/{report_id}/attachments/{BLOB_SHA256}");
        let resp = actix_test::call_service(&app, get(&path, false).to_request()).await;
        assert_eq!(resp.status(), 401);
        let resp = actix_test::call_service(&app, get(&path, true).to_request()).await;
        assert_eq!(resp.status(), 404);
        assert!(actix_test::read_body(resp).await.is_empty());
        for path in [
            format!("/api/reports/not-a-uuid/attachments/{BLOB_SHA256}"),
            format!("/api/reports/{report_id}/attachments/not%2Bbase64"),
        ] {
            let resp = actix_test::call_service(&app, get(&path, true).to_request()).await;
            assert_eq!(resp.status(), 404, "{path}");
        }
    }

    macro_rules! attachment_app {
        ($gate:expr) => {
            actix_test::init_service(
                App::new()
                    .service(
                        web::resource("/api/files/upload")
                            .route(web::post().to(handle_file_upload)),
                    )
                    .service(
                        web::resource("/api/attachments/{attachment_id}/download")
                            .route(web::get().to(handle_attachment_download)),
                    )
                    .app_data(web::Data::new(
                        Arc::new(MockRequestAuthenticator::new()) as Arc<dyn RequestAuthenticator>
                    ))
                    .app_data(web::Data::new(Arc::new(
                        rim_services::message_attachment::service::MockMessageAttachmentService::new(),
                    )
                        as Arc<dyn MessageAttachmentService>))
                    .app_data(web::Data::new(Arc::new(OneFile) as Arc<dyn StorageBackend>))
                    .app_data(web::Data::new(Arc::new($gate) as Arc<dyn FeatureGate>)),
            )
            .await
        };
    }

    fn upload() -> actix_test::TestRequest {
        actix_test::TestRequest::post()
            .uri("/api/files/upload")
            .insert_header((
                header::CONTENT_TYPE,
                "multipart/form-data; boundary=rimboundary",
            ))
            .set_payload(
                "--rimboundary\r\nContent-Disposition: form-data; name=\"file\"; \
                 filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n--rimboundary--\r\n",
            )
    }

    /// With attachments disabled, upload and download answer 403 `FEATURE_DISABLED` before the
    /// session is looked at; the unprimed mocks panic on any call.
    #[actix_web::test]
    async fn a_disabled_feature_refuses_attachment_upload_and_download() {
        let gate = rim_infra_services::feature_gate::InstanceFeatureGate::new(
            &[ServerFeature::Attachments],
            &[],
        );
        let app = attachment_app!(gate);
        for req in [
            upload().to_request(),
            actix_test::TestRequest::get()
                .uri(&format!(
                    "/api/attachments/{}/download",
                    uuid::Uuid::now_v7()
                ))
                .to_request(),
        ] {
            let resp = actix_test::call_service(&app, req).await;
            assert_eq!(resp.status(), 403);
            let body: serde_json::Value = actix_test::read_body_json(resp).await;
            assert_eq!(body["code"], "FEATURE_DISABLED");
            assert_eq!(body["feature"], "attachments");
        }
    }

    /// With attachments enabled the upload goes on to the CSRF check.
    #[actix_web::test]
    async fn an_enabled_feature_lets_the_upload_reach_the_csrf_check() {
        let app =
            attachment_app!(rim_infra_services::feature_gate::InstanceFeatureGate::all_enabled());
        let resp = actix_test::call_service(&app, upload().to_request()).await;
        assert_eq!(resp.status(), 403);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert!(body.get("code").is_none(), "{body}");
    }
}
