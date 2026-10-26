use std::sync::Arc;

use actix_multipart::Multipart;
use actix_web::http::header;
use actix_web::{HttpRequest, HttpResponse, web};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::StreamExt;
use rim_core::server_feature::model::ServerFeature;
use rim_core::storage::{StorageBackend, StoredFile};
use rim_core::user::avatar::{AvatarFormat, file_url, hashed_avatar_key, is_hashed_avatar_key};
use rim_core::user::model::UserId;
use rim_infra_services::avatar_check::{AvatarCheckError, AvatarChecker, AvatarVerdict};
use rim_infra_services::avatar_image::{AvatarImageError, MAX_SOURCE_SIDE, normalize_avatar};
use rim_services::auth::service::RequestAuthenticator;
use rim_services::feature::service::FeatureGate;
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::moderation::command::AuthorizeReportAttachmentQuery;
use rim_services::moderation::service::ModerationService;
use rim_services::operator::OperatorCaller;
use rim_services::relay::dto::FileAbuseReport;
use rim_services::relay::service::RelayAbuseService;
use rim_services::storage::service::StorageService;
use rim_services::user::service::UserService;
use sha2::{Digest, Sha256};

use crate::graphql_support::{extract_auth, extract_auth_without_secret, validate_csrf_header};

/// Map a ServiceError to an HTTP response for REST endpoints.
///
/// Shared by the non-GraphQL file transports in `rim_server` so REST error shapes stay
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

/// Maximum avatar upload size: 1 MiB.
const MAX_AVATAR_BYTES: u64 = 1024 * 1024;

/// The type of an encrypted conversation's blob. The service accepts it only for an attachment
/// created with a `blobSha256`, and checks the bytes against it.
const ENCRYPTED_BLOB_MIME: &str = "application/octet-stream";

/// The file name every encrypted blob is downloaded under; the real one is in the message.
const ENCRYPTED_BLOB_FILENAME: &str = "encrypted.bin";

use rim_core::util::sanitize_filename;

/// Refuse a route of a feature that is not enabled on this instance: 403 with the
/// `FEATURE_DISABLED` code.
#[allow(clippy::result_large_err)]
pub(crate) fn require_feature(
    features: &dyn FeatureGate,
    feature: ServerFeature,
) -> Result<(), HttpResponse> {
    features
        .require(feature)
        .map_err(map_service_error_to_response)
}

/// Enforce CSRF validation for REST endpoints. Returns 403 on failure.
#[allow(clippy::result_large_err)]
pub(crate) fn require_csrf(req: &HttpRequest) -> Result<(), HttpResponse> {
    validate_csrf_header(req)
        .map_err(|msg| HttpResponse::Forbidden().json(serde_json::json!({"error": msg})))
}

/// Require an authenticated user from the session cookie, and for an unkept session its secret
/// in the `X-RIM-Session-Secret` header. Returns user_id, or 401 without a session.
pub(crate) async fn require_auth(
    req: &HttpRequest,
) -> Result<rim_core::user::model::UserId, HttpResponse> {
    authenticated_user(req, true).await
}

/// [`require_auth`] for a plain GET download, which an `<img>` or a link makes without
/// headers: the session cookie alone, without an unkept session's secret.
pub(crate) async fn require_download_auth(
    req: &HttpRequest,
) -> Result<rim_core::user::model::UserId, HttpResponse> {
    authenticated_user(req, false).await
}

async fn authenticated_user(
    req: &HttpRequest,
    with_secret: bool,
) -> Result<rim_core::user::model::UserId, HttpResponse> {
    let authenticator = req
        .app_data::<web::Data<Arc<dyn RequestAuthenticator>>>()
        .map(|d| Arc::clone(d.get_ref()));

    let authenticator = authenticator.ok_or_else(|| {
        HttpResponse::InternalServerError()
            .json(serde_json::json!({"error": "auth not configured"}))
    })?;

    let auth = if with_secret {
        extract_auth(req, authenticator.as_ref()).await
    } else {
        extract_auth_without_secret(req, authenticator.as_ref()).await
    };
    let auth = auth.map_err(|_| {
        HttpResponse::InternalServerError().json(serde_json::json!({"error": "auth check failed"}))
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

    if !rim_services::message_attachment::is_plaintext_content_type(&upload.content_type)
        && !upload
            .content_type
            .eq_ignore_ascii_case(ENCRYPTED_BLOB_MIME)
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
    features: web::Data<Arc<dyn FeatureGate>>,
) -> HttpResponse {
    if let Err(resp) = require_feature(features.get_ref().as_ref(), ServerFeature::Avatars) {
        return resp;
    }
    if let Err(resp) = require_download_auth(&req).await {
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
            header::CACHE_CONTROL,
            if is_hashed_avatar_key(file_path) {
                "private, max-age=31536000, immutable"
            } else {
                "private, max-age=0, must-revalidate"
            },
        ))
        .insert_header((header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
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
    let user_id = match require_download_auth(&req).await {
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
/// Owner/Moderator of the report's workspace, or an instance operator for a report of the
/// direct space, who decrypts it with the secret from the report's verified content. `blob_sha256` is unpadded base64url. Every refusal is a bare 404.
pub async fn handle_report_attachment_download(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    moderation_service: web::Data<Arc<dyn ModerationService>>,
    storage_backend: web::Data<Arc<dyn StorageBackend>>,
) -> HttpResponse {
    let user_id = match require_download_auth(&req).await {
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

/// The profile picture check every uploaded picture goes through, and where the reports it
/// files go. Without a checker pictures are stored unchecked.
#[derive(Clone, Copy)]
pub(crate) struct AvatarScreening<'a> {
    pub(crate) checker: Option<&'a AvatarChecker>,
    pub(crate) abuse: &'a dyn RelayAbuseService,
}

/// The reason an automatic report of a picture starts with.
const AUTOMATIC_REPORT_PREFIX: &str = "automatic profile picture check";

/// POST /api/files/avatar
pub async fn handle_avatar_upload(
    req: HttpRequest,
    payload: Multipart,
    storage_service: web::Data<Arc<dyn StorageService>>,
    user_service: web::Data<Arc<dyn UserService>>,
    features: web::Data<Arc<dyn FeatureGate>>,
    checker: web::Data<Option<Arc<AvatarChecker>>>,
    abuse: web::Data<Arc<dyn RelayAbuseService>>,
) -> HttpResponse {
    if let Err(resp) = require_feature(features.get_ref().as_ref(), ServerFeature::Avatars) {
        return resp;
    }
    if let Err(resp) = require_csrf(&req) {
        return resp;
    }

    let user_id = match require_auth(&req).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    avatar_upload_as(
        user_id,
        payload,
        storage_service.get_ref().as_ref(),
        user_service.get_ref().as_ref(),
        AvatarScreening {
            checker: checker.get_ref().as_deref(),
            abuse: abuse.get_ref().as_ref(),
        },
    )
    .await
}

/// Store `user_id`'s avatar, then delete the user's other avatar files: the session path above
/// and forwarded requests from another relay share it.
///
/// The user service first decides whether the upload may start, before the body is read: a
/// locked profile answers 403, probation 400 and an upload limit 429 with `Retry-After`. The
/// declared type must be png, jpeg, gif or webp and the bytes must start with that format's
/// signature (415), within 1 MiB (413). The image is then decoded as its declared type and
/// re-encoded: its first frame, centre-cropped to a square, is stored as a 256×256 PNG without
/// metadata. Bytes that do not decode answer 415, an image wider or taller than 4096 pixels
/// 413.
///
/// With a checker in `screening`, the re-encoded picture is scored before it is stored: a
/// score at the block threshold answers 422 and one at the report threshold is stored and
/// reported to the operators; when the check cannot run the answer is 503. Nothing is stored
/// unless the check let it through.
pub(crate) async fn avatar_upload_as(
    user_id: UserId,
    payload: Multipart,
    storage_service: &dyn StorageService,
    user_service: &dyn UserService,
    screening: AvatarScreening<'_>,
) -> HttpResponse {
    if let Err(e) = user_service.begin_avatar_upload(user_id).await {
        return map_service_error_to_response(e);
    }

    let upload = match read_multipart_fields(payload, MAX_AVATAR_BYTES).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };

    let Some(format) = AvatarFormat::from_content_type(&upload.content_type) else {
        return HttpResponse::UnsupportedMediaType()
            .json(serde_json::json!({"error": "avatar must be an image (png, jpeg, gif, webp)"}));
    };
    if !format.matches_signature(&upload.data) {
        return HttpResponse::UnsupportedMediaType()
            .json(serde_json::json!({"error": "avatar content does not match its type"}));
    }

    let png = match reencode_avatar(upload.data, format).await {
        Ok(png) => png,
        Err(resp) => return resp,
    };
    let verdict = match screening.checker {
        Some(checker) => match check_avatar(user_id, &png, checker).await {
            Ok(verdict) => verdict,
            Err(resp) => return resp,
        },
        None => AvatarVerdict::Allow,
    };
    let stored = match store_avatar(user_id, &png, storage_service).await {
        Ok(stored) => stored,
        Err(resp) => return resp,
    };
    if let AvatarVerdict::Report { category, score } = verdict {
        report_avatar(
            user_id,
            &category,
            score,
            &stored.key,
            user_service,
            screening.abuse,
        )
        .await;
    }
    HttpResponse::Ok().json(serde_json::json!({
        "path": stored.key,
        "size_bytes": stored.size_bytes,
        "url": file_url(&stored.key),
    }))
}

/// The checker's verdict on `user_id`'s re-encoded `png`.
///
/// # Errors
/// 422 for a picture at the block threshold, logged as the audit event `avatar.refused`; 503
/// when the check cannot run.
async fn check_avatar(
    user_id: UserId,
    png: &[u8],
    checker: &AvatarChecker,
) -> Result<AvatarVerdict, HttpResponse> {
    match checker.check(png).await {
        Ok(AvatarVerdict::Block { category, score }) => {
            tracing::info!(
                target: "audit",
                event = "avatar.refused",
                user_id = %user_id,
                category = %category,
                score,
                "a profile picture was refused by the automatic check"
            );
            Err(HttpResponse::UnprocessableEntity().json(serde_json::json!({
                "error": "this image isn't allowed as a profile picture"
            })))
        }
        Ok(verdict) => Ok(verdict),
        Err(AvatarCheckError::Unavailable(why)) => {
            tracing::warn!(user_id = %user_id, error = %why, "the profile picture check is unavailable");
            Err(HttpResponse::ServiceUnavailable().json(serde_json::json!({
                "error": "profile pictures can't be checked right now; try again later"
            })))
        }
    }
}

/// File an abuse report of `user_id`'s stored picture at `key`, which scored `score` in
/// `category`, as the server. Skipped with a warning when the account has no principal id; a
/// failure is logged and never fails the upload.
async fn report_avatar(
    user_id: UserId,
    category: &str,
    score: f64,
    key: &str,
    user_service: &dyn UserService,
    abuse: &dyn RelayAbuseService,
) {
    let principal_id = match user_service.get_users_by_ids(&[user_id]).await {
        Ok(users) => users.into_iter().find_map(|user| user.principal_id),
        Err(e) => {
            tracing::error!(user_id = %user_id, error = %e, "could not look up the account to report its profile picture");
            return;
        }
    };
    let Some(principal_id) = principal_id else {
        tracing::warn!(
            user_id = %user_id,
            category,
            score,
            "a profile picture was flagged, but the account has no principal id to report"
        );
        return;
    };
    let reason = format!(
        "{AUTOMATIC_REPORT_PREFIX}: {category} {score:.2} — {}",
        file_url(key)
    );
    if let Err(e) = abuse
        .file_report(
            OperatorCaller::Server,
            FileAbuseReport {
                subject_principal_id: Some(principal_id),
                subject_server_id: None,
                reason,
            },
        )
        .await
    {
        tracing::error!(user_id = %user_id, error = %e, "could not report a flagged profile picture");
    }
}

/// `data`, of the declared `format`, re-encoded as the PNG that is stored, on the blocking
/// pool.
///
/// # Errors
/// 415 for bytes that do not decode, 413 past the pixel limit, 500 when the pool fails.
async fn reencode_avatar(data: Vec<u8>, format: AvatarFormat) -> Result<Vec<u8>, HttpResponse> {
    match web::block(move || normalize_avatar(&data, format)).await {
        Ok(Ok(png)) => Ok(png),
        Ok(Err(AvatarImageError::Unreadable)) => Err(HttpResponse::UnsupportedMediaType()
            .json(serde_json::json!({"error": "avatar could not be read as an image"}))),
        Ok(Err(AvatarImageError::TooLarge)) => {
            Err(HttpResponse::PayloadTooLarge().json(serde_json::json!({
                "error": format!(
                    "avatar image must be at most {MAX_SOURCE_SIDE}×{MAX_SOURCE_SIDE} pixels"
                )
            })))
        }
        Err(e) => {
            tracing::error!(error = %e, "avatar re-encoding failed");
            Err(HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "internal error"})))
        }
    }
}

/// Store `png` as `user_id`'s avatar under its content-addressed key and delete the user's
/// other avatar files.
///
/// # Errors
/// 500 when the file cannot be stored.
async fn store_avatar(
    user_id: UserId,
    png: &[u8],
    storage_service: &dyn StorageService,
) -> Result<StoredFile, HttpResponse> {
    let storage_key = hashed_avatar_key(user_id, AvatarFormat::Png, &Sha256::digest(png).into());
    let stored = match storage_service
        .upload_file(&storage_key, png, AvatarFormat::Png.mime())
        .await
    {
        Ok(stored) => stored,
        Err(e) => {
            tracing::error!(error = %e, "avatar upload failed");
            return Err(HttpResponse::InternalServerError()
                .json(serde_json::json!({"error": "upload failed"})));
        }
    };

    // The new file is stored; a leftover that cannot be deleted now goes with the next upload,
    // the next profile change or the account.
    if let Err(e) = storage_service
        .delete_avatar_files(user_id, Some(stored.key.clone()))
        .await
    {
        tracing::warn!(user_id = %user_id, error = %e, "could not delete the previous avatar files");
    }
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use actix_web::App;
    use actix_web::cookie::Cookie;
    use actix_web::test as actix_test;
    use rim_core::auth::model::SessionId;
    use rim_core::message_attachment::model::MessageAttachmentId;
    use rim_core::moderation::model::MessageReportId;
    use rim_core::storage::{StorageError, StoredFile};
    use rim_core::user::model::UserId;
    use rim_services::ServiceError;
    use rim_services::auth::dto::RequestAuthResultDto;
    use rim_services::auth::service::MockRequestAuthenticator;
    use rim_services::message_attachment::dto::{AttachmentDto, AttachmentStatus};
    use rim_services::message_attachment::service::MockMessageAttachmentService;
    use rim_services::moderation::dto::ReportAttachmentDto;
    use rim_services::moderation::service::MockModerationService;
    use rim_services::relay::service::MockRelayAbuseService;
    use rim_services::storage::service::MockStorageService;
    use rim_services::user::dto::UserDto;
    use rim_services::user::service::MockUserService;
    use serde_json::{Value, json};

    use super::*;
    use crate::graphql_support::{COOKIE_CSRF, COOKIE_SESSION};

    const SESSION: &str = "rim_sess_test";
    const BOUNDARY: &str = "rimboundary";
    const BLOB: &[u8] = b"opaque blob";
    const AVATAR: &[u8] = b"\x89PNG avatar";
    /// The first bytes of a PNG and of a JPEG file.
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\npng";
    const JPEG: &[u8] = b"\xFF\xD8\xFF\xE0jpeg";
    /// Unpadded base64url of 32 bytes of 0x07.
    const BLOB_SHA256: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc";

    /// Storage holding `uploads/blob` and `avatars/u/avatar.png`; `broken` fails to read.
    struct Files;

    #[async_trait::async_trait]
    impl StorageBackend for Files {
        async fn put(&self, _: &str, _: &[u8], _: &str) -> Result<StoredFile, StorageError> {
            Err(StorageError::Unsupported("read only".to_string()))
        }

        async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
            match key {
                "uploads/blob" => Ok(BLOB.to_vec()),
                "avatars/u/avatar.png" | "avatars/u/avatar-0123456789abcdef.png" => {
                    Ok(AVATAR.to_vec())
                }
                "broken" => Err(StorageError::Unsupported("disk gone".to_string())),
                _ => Err(StorageError::NotFound(key.to_string())),
            }
        }

        async fn delete(&self, _: &str) -> Result<(), StorageError> {
            Ok(())
        }

        async fn list(&self, _: &str) -> Result<Vec<String>, StorageError> {
            Ok(Vec::new())
        }
    }

    /// An authenticator that knows one session, of `user_id`.
    fn authenticator(user_id: UserId) -> MockRequestAuthenticator {
        let mut auth = MockRequestAuthenticator::new();
        auth.expect_authenticate_session_token()
            .returning(move |token, _| {
                let known = token == Some(SESSION);
                Box::pin(async move {
                    Ok(if known {
                        RequestAuthResultDto {
                            user_id: Some(user_id),
                            session_id: Some(SessionId::new()),
                            device_key_id: None,
                            expires_at: None,
                            secret_hash: None,
                        }
                    } else {
                        RequestAuthResultDto::default()
                    })
                })
            });
        auth
    }

    /// The services behind the routes, for the session of `user_id`. Unprimed mocks panic on
    /// any call, so a test that leaves one alone also pins that the handler never reached it.
    struct Deps {
        user_id: UserId,
        attachments: MockMessageAttachmentService,
        storage: MockStorageService,
        moderation: MockModerationService,
        users: MockUserService,
        disabled: Vec<ServerFeature>,
        checker: Option<Arc<AvatarChecker>>,
        abuse: MockRelayAbuseService,
    }

    impl Deps {
        fn new() -> Self {
            Self {
                user_id: UserId::new(),
                attachments: MockMessageAttachmentService::new(),
                storage: MockStorageService::new(),
                moderation: MockModerationService::new(),
                users: MockUserService::new(),
                disabled: Vec::new(),
                checker: None,
                abuse: MockRelayAbuseService::new(),
            }
        }
    }

    /// Every file route, served over `$deps`.
    macro_rules! app {
        ($deps:expr) => {{
            let deps: Deps = $deps;
            actix_test::init_service(
                App::new()
                    .service(
                        web::resource("/api/files/upload")
                            .route(web::post().to(handle_file_upload)),
                    )
                    .service(
                        web::resource("/api/files/avatar")
                            .route(web::post().to(handle_avatar_upload)),
                    )
                    .service(
                        web::resource("/api/files/{path:.*}")
                            .route(web::get().to(handle_file_download)),
                    )
                    .service(
                        web::resource("/api/attachments/{attachment_id}/download")
                            .route(web::get().to(handle_attachment_download)),
                    )
                    .service(
                        web::resource("/api/reports/{report_id}/attachments/{blob_sha256}")
                            .route(web::get().to(handle_report_attachment_download)),
                    )
                    .app_data(web::Data::new(
                        Arc::new(authenticator(deps.user_id)) as Arc<dyn RequestAuthenticator>
                    ))
                    .app_data(web::Data::new(
                        Arc::new(deps.attachments) as Arc<dyn MessageAttachmentService>
                    ))
                    .app_data(web::Data::new(
                        Arc::new(deps.storage) as Arc<dyn StorageService>
                    ))
                    .app_data(web::Data::new(
                        Arc::new(deps.moderation) as Arc<dyn ModerationService>
                    ))
                    .app_data(web::Data::new(Arc::new(deps.users) as Arc<dyn UserService>))
                    .app_data(web::Data::new(deps.checker))
                    .app_data(web::Data::new(
                        Arc::new(deps.abuse) as Arc<dyn RelayAbuseService>
                    ))
                    .app_data(web::Data::new(Arc::new(Files) as Arc<dyn StorageBackend>))
                    .app_data(web::Data::new(Arc::new(
                        rim_infra_services::feature_gate::InstanceFeatureGate::new(
                            &deps.disabled,
                            &[],
                        ),
                    ) as Arc<dyn FeatureGate>)),
            )
            .await
        }};
    }

    fn get(path: &str, signed_in: bool) -> actix_test::TestRequest {
        let req = actix_test::TestRequest::get().uri(path);
        if signed_in {
            req.cookie(Cookie::new(COOKIE_SESSION, SESSION))
        } else {
            req
        }
    }

    /// One part of a multipart body.
    #[derive(Clone, Copy)]
    enum Part<'a> {
        Text(&'a str, &'a [u8]),
        /// File name, content type, bytes.
        File(&'a str, &'a str, &'a [u8]),
    }

    /// A small text file.
    const TXT: Part<'static> = Part::File("a.txt", "text/plain", b"x");

    fn multipart_body(parts: &[Part<'_>]) -> Vec<u8> {
        let mut body = Vec::new();
        for part in parts {
            body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            let (head, data) = match part {
                Part::Text(name, value) => (
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
                    *value,
                ),
                Part::File(filename, content_type, data) => (
                    format!(
                        "Content-Disposition: form-data; name=\"file\"; \
                         filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
                    ),
                    *data,
                ),
            };
            body.extend_from_slice(head.as_bytes());
            body.extend_from_slice(data);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        body
    }

    /// A multipart POST of `body` to `path`; signed in with a valid CSRF pair when `signed_in`.
    fn post_body(path: &str, body: Vec<u8>, signed_in: bool) -> actix_test::TestRequest {
        let req = actix_test::TestRequest::post()
            .uri(path)
            .insert_header((
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            ))
            .set_payload(body);
        if !signed_in {
            return req;
        }
        let csrf = rim_auth::csrf::generate_csrf_token(SESSION);
        req.cookie(Cookie::new(COOKIE_SESSION, SESSION))
            .cookie(Cookie::new(COOKIE_CSRF, csrf.clone()))
            .insert_header(("x-csrf-token", csrf))
    }

    fn post(path: &str, parts: &[Part<'_>]) -> actix_test::TestRequest {
        post_body(path, multipart_body(parts), true)
    }

    /// The parts of a complete attachment upload of `data` typed `content_type`.
    fn upload_parts<'a>(
        token: &'a str,
        attachment_id: &'a str,
        content_type: &'a str,
        data: &'a [u8],
    ) -> [Part<'a>; 3] {
        [
            Part::Text("upload_token", token.as_bytes()),
            Part::Text("attachment_id", attachment_id.as_bytes()),
            Part::File("dir/a.txt", content_type, data),
        ]
    }

    /// Send `$req` to `$app` and return its status and JSON body (`null` for a non-JSON body).
    macro_rules! status_and_body {
        ($app:expr, $req:expr) => {{
            let resp = actix_test::call_service($app, $req.to_request()).await;
            let status = resp.status().as_u16();
            let bytes = actix_test::read_body(resp).await;
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            (status, body)
        }};
    }

    fn attachment(storage_key: Option<&str>, blob_sha256: Option<Vec<u8>>) -> AttachmentDto {
        AttachmentDto {
            id: MessageAttachmentId::new(),
            status: AttachmentStatus::Ready,
            filename: "re\"port.txt".to_string(),
            content_type: "text/plain".to_string(),
            size_bytes: BLOB.len() as u64,
            storage_key: storage_key.map(str::to_string),
            created_at: chrono::Utc::now(),
            blob_sha256,
        }
    }

    // ── Upload ──────────────────────────────────────────────────────────────

    /// A complete upload hands the service the caller, the parsed id, the token, the file's
    /// last path component, its type and bytes, and answers the stored size.
    #[actix_web::test]
    async fn upload_completes_through_the_service_and_answers_the_stored_size() {
        let mut deps = Deps::new();
        let user_id = deps.user_id;
        let attachment_id = MessageAttachmentId::new();
        deps.attachments
            .expect_complete_upload()
            .withf(move |cmd| {
                cmd.actor_user_id == user_id
                    && cmd.attachment_id == attachment_id
                    && cmd.upload_token == "tok"
                    && cmd.uploaded_filename == "a.txt"
                    && cmd.content_type == "text/plain"
                    && cmd.data == b"hello"
            })
            .times(1)
            .returning(|_| Box::pin(async { Ok(attachment(Some("uploads/blob"), None)) }));
        let app = app!(deps);

        let id = attachment_id.to_string();
        let req = post(
            "/api/files/upload",
            &upload_parts(" tok ", &id, "text/plain", b"hello"),
        );
        assert_eq!(
            status_and_body!(&app, req),
            (200, json!({ "size_bytes": BLOB.len() }))
        );
    }

    /// An encrypted conversation's blob type is accepted; a type outside the allow list is
    /// refused with 415 before the service is asked.
    #[actix_web::test]
    async fn upload_accepts_the_encrypted_blob_type_and_refuses_other_types() {
        let mut deps = Deps::new();
        deps.attachments
            .expect_complete_upload()
            .withf(|cmd| cmd.content_type == "application/octet-stream")
            .times(1)
            .returning(|_| Box::pin(async { Ok(attachment(Some("uploads/blob"), None)) }));
        let app = app!(deps);
        let id = MessageAttachmentId::new().to_string();

        let req = post(
            "/api/files/upload",
            &upload_parts("tok", &id, "application/octet-stream", b"x"),
        );
        assert_eq!(status_and_body!(&app, req).0, 200);

        let req = post(
            "/api/files/upload",
            &upload_parts("tok", &id, "application/x-msdownload", b"x"),
        );
        assert_eq!(
            status_and_body!(&app, req),
            (415, json!({ "error": "file type not allowed" }))
        );
    }

    /// Malformed or incomplete uploads answer 400 with the reason and never reach the service.
    #[actix_web::test]
    async fn upload_refuses_missing_and_malformed_fields() {
        let app = app!(Deps::new());
        let id = MessageAttachmentId::new().to_string();
        let long = [b'a'; 257];
        let cases: Vec<(Vec<Part<'_>>, &str)> = vec![
            (
                vec![Part::Text("attachment_id", id.as_bytes()), TXT],
                "upload_token is required",
            ),
            (
                vec![
                    Part::Text("upload_token", b"  "),
                    Part::Text("attachment_id", id.as_bytes()),
                    TXT,
                ],
                "upload_token is required",
            ),
            (
                vec![Part::Text("upload_token", b"tok"), TXT],
                "attachment_id is required",
            ),
            (
                upload_parts("tok", "nope", "text/plain", b"x").into(),
                "invalid attachment_id",
            ),
            (
                vec![Part::Text("upload_token", &long), TXT],
                "field value too long",
            ),
            (
                vec![Part::Text("attachment_id", &long), TXT],
                "field value too long",
            ),
            (
                vec![Part::Text("upload_token", b"tok")],
                "no file in upload",
            ),
            (
                vec![TXT, Part::File("b.txt", "text/plain", b"y")],
                "only one file allowed per upload",
            ),
            (
                vec![Part::File("../a.txt", "text/plain", b"x")],
                "invalid filename",
            ),
        ];
        for (parts, error) in cases {
            let req = post("/api/files/upload", &parts);
            assert_eq!(
                status_and_body!(&app, req),
                (400, json!({ "error": error })),
                "{error}"
            );
        }
    }

    /// A body cut off inside the file answers 400 with the read error.
    #[actix_web::test]
    async fn upload_refuses_a_truncated_body() {
        let app = app!(Deps::new());
        let mut body = multipart_body(&[Part::File("a.txt", "text/plain", b"hello")]);
        body.truncate(body.len() - 20);
        let (status, body) = status_and_body!(&app, post_body("/api/files/upload", body, true));
        assert_eq!(status, 400);
        let error = body["error"].as_str().unwrap_or_default();
        assert!(
            error.starts_with("read error") || error == "malformed multipart field",
            "{body}"
        );
    }

    /// A file one byte over the attachment cap answers 413 naming the limit.
    #[actix_web::test]
    async fn upload_refuses_a_file_over_the_attachment_cap() {
        let app = app!(Deps::new());
        let data = vec![b'a'; MAX_UPLOAD_BYTES as usize + 1];
        let id = MessageAttachmentId::new().to_string();
        let req = post(
            "/api/files/upload",
            &upload_parts("tok", &id, "text/plain", &data),
        );
        assert_eq!(
            status_and_body!(&app, req),
            (
                413,
                json!({ "error": format!("file exceeds {MAX_UPLOAD_BYTES} byte limit") })
            )
        );
    }

    /// A refusal from the attachment service reaches the client as a status and an `error` body.
    #[actix_web::test]
    async fn upload_maps_service_refusals_to_http_answers() {
        let mut deps = Deps::new();
        deps.attachments.expect_complete_upload().returning(|cmd| {
            let err = match cmd.upload_token.as_str() {
                "denied" => ServiceError::PermissionDenied,
                "gone" => ServiceError::NotFound,
                "bad" => ServiceError::Validation("size mismatch".to_string()),
                "busy" => ServiceError::RateLimited {
                    retry_after_seconds: 7,
                },
                _ => ServiceError::internal(anyhow::anyhow!("boom")),
            };
            Box::pin(async move { Err(err) })
        });
        let app = app!(deps);
        let id = MessageAttachmentId::new().to_string();
        let cases = [
            ("denied", 403, json!({ "error": "access denied" })),
            ("gone", 404, json!({ "error": "not found" })),
            ("bad", 400, json!({ "error": "size mismatch" })),
            (
                "busy",
                429,
                json!({ "error": "rate limit exceeded", "retryAfterSeconds": 7 }),
            ),
            ("other", 500, json!({ "error": "internal error" })),
        ];
        for (token, status, body) in cases {
            let req = post(
                "/api/files/upload",
                &upload_parts(token, &id, "text/plain", b"x"),
            );
            assert_eq!(status_and_body!(&app, req), (status, body), "{token}");
        }
    }

    /// With attachments disabled, upload and download answer 403 `FEATURE_DISABLED` before the
    /// session is looked at.
    #[actix_web::test]
    async fn a_disabled_feature_refuses_attachment_upload_and_download() {
        let mut deps = Deps::new();
        deps.disabled = vec![ServerFeature::Attachments];
        let app = app!(deps);
        let upload = post_body("/api/files/upload", multipart_body(&[TXT]), false);
        let download = get(
            &format!("/api/attachments/{}/download", MessageAttachmentId::new()),
            false,
        );
        for req in [upload, download] {
            let (status, body) = status_and_body!(&app, req);
            assert_eq!(status, 403);
            assert_eq!(body["code"], "FEATURE_DISABLED");
            assert_eq!(body["feature"], "attachments");
        }
    }

    /// With avatars off, avatar upload and stored-file download answer 403 with the feature
    /// before the session is looked at, and storage is never reached.
    #[actix_web::test]
    async fn a_disabled_feature_refuses_avatar_upload_and_download() {
        let mut deps = Deps::new();
        deps.disabled = vec![ServerFeature::Avatars];
        let app = app!(deps);
        let upload = post_body(
            "/api/files/avatar",
            multipart_body(&[Part::File("a.png", "image/png", b"x")]),
            false,
        );
        let download = get("/api/files/avatars/u/avatar.png", false);
        for req in [upload, download] {
            let (status, body) = status_and_body!(&app, req);
            assert_eq!(status, 403);
            assert_eq!(body["code"], "FEATURE_DISABLED");
            assert_eq!(body["feature"], "avatars");
        }
    }

    /// Without a CSRF pair, upload and avatar upload answer 403 without a feature code.
    #[actix_web::test]
    async fn uploads_without_csrf_are_refused() {
        let app = app!(Deps::new());
        for path in ["/api/files/upload", "/api/files/avatar"] {
            let req = post_body(
                path,
                multipart_body(&[Part::File("a.png", "image/png", b"x")]),
                false,
            )
            .cookie(Cookie::new(COOKIE_SESSION, SESSION));
            let (status, body) = status_and_body!(&app, req);
            assert_eq!(status, 403, "{path}");
            assert_eq!(body["error"], "csrf validation failed", "{path}");
        }
    }

    // ── Downloads ───────────────────────────────────────────────────────────

    /// A readable attachment is served with its type and a sanitised file name; an encrypted one
    /// as opaque bytes named `encrypted.bin`.
    #[actix_web::test]
    async fn attachment_download_serves_the_file_with_its_type_and_name() {
        let mut deps = Deps::new();
        let user_id = deps.user_id;
        let plain = MessageAttachmentId::new();
        deps.attachments
            .expect_authorize_download()
            .withf(move |q| q.actor_user_id == user_id)
            .returning(move |q| {
                let blob_sha256 = (q.attachment_id != plain).then(|| vec![7; 32]);
                Box::pin(async move { Ok(attachment(Some("uploads/blob"), blob_sha256)) })
            });
        let app = app!(deps);

        let cases = [
            (plain, "text/plain", "attachment; filename=\"report.txt\""),
            (
                MessageAttachmentId::new(),
                "application/octet-stream",
                "attachment; filename=\"encrypted.bin\"",
            ),
        ];
        for (id, content_type, disposition) in cases {
            let req = get(&format!("/api/attachments/{id}/download"), true);
            let resp = actix_test::call_service(&app, req.to_request()).await;
            assert_eq!(resp.status(), 200);
            let headers = resp.headers();
            assert_eq!(
                headers.get(header::CONTENT_TYPE).expect("type"),
                content_type
            );
            assert_eq!(
                headers
                    .get(header::CONTENT_DISPOSITION)
                    .expect("disposition"),
                disposition
            );
            assert_eq!(actix_test::read_body(resp).await, BLOB);
        }
    }

    /// Attachment download answers 400 for a malformed id, the service's refusal, 404 while the
    /// file is pending or missing from storage, and 500 when storage fails.
    #[actix_web::test]
    async fn attachment_download_refusals() {
        let mut deps = Deps::new();
        let [denied, gone, pending, lost, broken] = [(); 5].map(|_| MessageAttachmentId::new());
        deps.attachments
            .expect_authorize_download()
            .returning(move |q| {
                let id = q.attachment_id;
                let result = if id == denied {
                    Err(ServiceError::PermissionDenied)
                } else if id == gone {
                    Err(ServiceError::NotFound)
                } else if id == pending {
                    Ok(attachment(None, None))
                } else if id == lost {
                    Ok(attachment(Some("uploads/lost"), None))
                } else {
                    Ok(attachment(Some("broken"), None))
                };
                Box::pin(async move { result })
            });
        let app = app!(deps);

        let cases = [
            ("nope".to_string(), 400, "invalid attachment id"),
            (denied.to_string(), 403, "access denied"),
            (gone.to_string(), 404, "not found"),
            (pending.to_string(), 404, "attachment not yet uploaded"),
            (lost.to_string(), 404, "file not found"),
            (broken.to_string(), 500, "download failed"),
        ];
        for (id, status, error) in cases {
            let req = get(&format!("/api/attachments/{id}/download"), true);
            assert_eq!(
                status_and_body!(&app, req),
                (status, json!({ "error": error })),
                "{error}"
            );
        }
    }

    /// An avatar is served with the type its extension implies, `nosniff` and its file name; a
    /// missing one answers 404 and a signed-out caller 401.
    #[actix_web::test]
    async fn avatar_download_serves_the_file_or_answers_404() {
        let app = app!(Deps::new());
        let resp = actix_test::call_service(
            &app,
            get("/api/files/avatars/u/avatar.png", true).to_request(),
        )
        .await;
        assert_eq!(resp.status(), 200);
        let headers = resp.headers();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).expect("type"),
            "image/png"
        );
        assert_eq!(
            headers
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .expect("nosniff"),
            "nosniff"
        );
        assert_eq!(
            headers
                .get(header::CONTENT_DISPOSITION)
                .expect("disposition"),
            "attachment; filename=\"avatar.png\""
        );
        assert_eq!(actix_test::read_body(resp).await, AVATAR);

        let req = get("/api/files/avatars/u/missing.png", true);
        assert_eq!(
            status_and_body!(&app, req),
            (404, json!({ "error": "file not found" }))
        );
        let req = get("/api/files/avatars/u/avatar.png", false);
        assert_eq!(
            status_and_body!(&app, req),
            (401, json!({ "error": "authentication required" }))
        );
    }

    /// A reported encrypted blob is served opaque, with `nosniff`.
    #[actix_web::test]
    async fn a_reported_blob_is_served_opaque() {
        let mut deps = Deps::new();
        let user_id = deps.user_id;
        let report_id = MessageReportId::new();
        deps.moderation
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
        let app = app!(deps);

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
        let mut deps = Deps::new();
        let report_id = MessageReportId::new();
        deps.moderation
            .expect_authorize_report_attachment()
            .times(1)
            .returning(|_| Box::pin(async { Err(ServiceError::NotFound) }));
        let app = app!(deps);

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

    // ── Avatar upload ───────────────────────────────────────────────────────

    /// A user service that lets every avatar upload start.
    fn allowing(deps: &mut Deps) {
        deps.users
            .expect_begin_avatar_upload()
            .returning(|_| Box::pin(async { Ok(()) }));
    }

    /// Storage that stores every upload as asked.
    fn storing(deps: &mut Deps) {
        deps.storage.expect_upload_file().returning(|k, data, _| {
            let stored = StoredFile {
                key: k.to_string(),
                size_bytes: data.len() as u64,
            };
            Box::pin(async move { Ok(stored) })
        });
    }

    /// A `width`×`height` image of one `color`, encoded as `format`.
    fn image_of(format: image::ImageFormat, width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb(color),
        ));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, format).expect("encode");
        bytes.into_inner()
    }

    /// The stored PNG decoded, after checking it is 256×256.
    fn stored_png(data: &[u8]) -> image::RgbaImage {
        let image = image::load_from_memory_with_format(data, image::ImageFormat::Png)
            .expect("a png")
            .to_rgba8();
        assert_eq!(image.dimensions(), (256, 256));
        image
    }

    /// A JPEG upload is stored re-encoded as a 256×256 PNG under the caller's key, whatever the
    /// client's file name, the caller's other avatar files are deleted, and the answer carries
    /// its path, size and URL.
    #[actix_web::test]
    async fn a_jpeg_avatar_is_stored_as_a_png_under_the_callers_key() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        let user_id = deps.user_id;
        let jpeg = image_of(image::ImageFormat::Jpeg, 300, 200, [200, 30, 30]);
        let png = normalize_avatar(&jpeg, AvatarFormat::Jpeg).expect("normalized");
        let key = hashed_avatar_key(user_id, AvatarFormat::Png, &Sha256::digest(&png).into());
        assert!(key.ends_with(".png"), "{key}");
        let expected = key.clone();
        let stored = png.clone();
        deps.storage
            .expect_upload_file()
            .withf(move |k, data, content_type| {
                k == expected && data == stored && content_type == "image/png"
            })
            .times(1)
            .returning(|k, data, _| {
                stored_png(data);
                let stored = StoredFile {
                    key: k.to_string(),
                    size_bytes: data.len() as u64,
                };
                Box::pin(async move { Ok(stored) })
            });
        let kept = key.clone();
        deps.storage
            .expect_delete_avatar_files()
            .withf(move |id, keep| *id == user_id && keep.as_deref() == Some(kept.as_str()))
            .times(1)
            .returning(move |id, _| {
                let deleted = vec![format!("avatars/{id}/avatar.png")];
                Box::pin(async move { Ok(deleted) })
            });
        let app = app!(deps);

        let req = post(
            "/api/files/avatar",
            &[Part::File("me.jpeg", "image/jpeg", &jpeg)],
        );
        assert_eq!(
            status_and_body!(&app, req),
            (
                200,
                json!({ "path": key, "size_bytes": png.len(), "url": format!("/api/files/{key}") })
            )
        );
    }

    /// A GIF upload is stored as a PNG too.
    #[actix_web::test]
    async fn a_gif_avatar_is_stored_as_a_png() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        deps.storage
            .expect_upload_file()
            .withf(|k, data, content_type| {
                k.ends_with(".png") && content_type == "image/png" && {
                    stored_png(data);
                    true
                }
            })
            .times(1)
            .returning(|k, data, _| {
                let stored = StoredFile {
                    key: k.to_string(),
                    size_bytes: data.len() as u64,
                };
                Box::pin(async move { Ok(stored) })
            });
        deps.storage
            .expect_delete_avatar_files()
            .returning(|_, _| Box::pin(async { Ok(Vec::new()) }));
        let app = app!(deps);

        let gif = image_of(image::ImageFormat::Gif, 64, 64, [0, 0, 255]);
        let (status, body) = status_and_body!(
            &app,
            post(
                "/api/files/avatar",
                &[Part::File("me.gif", "image/gif", &gif)]
            )
        );
        assert_eq!(status, 200, "{body}");
        assert!(body["url"].as_str().unwrap().ends_with(".png"));
    }

    #[actix_web::test]
    async fn replacing_an_avatar_changes_its_url_and_deletes_the_previous_key() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        let files = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            String,
            Vec<u8>,
        >::new()));
        let stored = files.clone();
        deps.storage
            .expect_upload_file()
            .returning(move |key, data, _| {
                stored
                    .lock()
                    .unwrap()
                    .insert(key.to_string(), data.to_vec());
                let file = StoredFile {
                    key: key.to_string(),
                    size_bytes: data.len() as u64,
                };
                Box::pin(async move { Ok(file) })
            });
        let stored = files.clone();
        deps.storage
            .expect_delete_avatar_files()
            .returning(move |_, keep| {
                stored
                    .lock()
                    .unwrap()
                    .retain(|key, _| Some(key) == keep.as_ref());
                Box::pin(async { Ok(Vec::new()) })
            });
        let app = app!(deps);
        let mut urls = Vec::new();
        for color in [[255, 0, 0], [0, 255, 0]] {
            let png = image_of(image::ImageFormat::Png, 32, 32, color);
            let (status, body) = status_and_body!(
                &app,
                post(
                    "/api/files/avatar",
                    &[Part::File("me.png", "image/png", &png)]
                )
            );
            assert_eq!(status, 200);
            urls.push(body["url"].as_str().unwrap().to_string());
        }
        assert_ne!(urls[0], urls[1]);
        let files = files.lock().unwrap();
        assert_eq!(files.len(), 1);
        assert!(files.contains_key(urls[1].strip_prefix("/api/files/").unwrap()));
    }

    #[actix_web::test]
    async fn hashed_avatar_downloads_are_private_and_immutable() {
        let app = app!(Deps::new());
        let resp = actix_test::call_service(
            &app,
            get("/api/files/avatars/u/avatar-0123456789abcdef.png", true).to_request(),
        )
        .await;
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, max-age=31536000, immutable"
        );
    }

    /// A stored avatar is answered with 200 even when the caller's other avatar files cannot be
    /// deleted.
    #[actix_web::test]
    async fn avatar_upload_succeeds_when_the_old_files_cannot_be_deleted() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        storing(&mut deps);
        deps.storage
            .expect_delete_avatar_files()
            .times(1)
            .returning(|_, _| Box::pin(async { Err(ServiceError::Unavailable) }));
        let app = app!(deps);

        let png = image_of(image::ImageFormat::Png, 16, 16, [1, 2, 3]);
        let req = post(
            "/api/files/avatar",
            &[Part::File("me.png", "image/png", &png)],
        );
        assert_eq!(status_and_body!(&app, req).0, 200);
    }

    /// Bytes that do not start with the declared format's signature answer 415 before anything
    /// is stored or deleted.
    #[actix_web::test]
    async fn avatar_upload_refuses_bytes_that_are_not_the_declared_format() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        let app = app!(deps);
        let mismatch = json!({ "error": "avatar content does not match its type" });
        let cases: [(&str, &[u8]); 6] = [
            ("image/png", JPEG),
            ("image/jpeg", PNG),
            ("image/gif", b"GIF88a"),
            ("image/webp", b"RIFF\x10\0\0\0WAVEfmt "),
            ("image/png", b"<svg onload=alert(1)>"),
            ("image/png", b""),
        ];
        for (content_type, data) in cases {
            let req = post(
                "/api/files/avatar",
                &[Part::File("me.png", content_type, data)],
            );
            assert_eq!(
                status_and_body!(&app, req),
                (415, mismatch.clone()),
                "{content_type} {data:?}"
            );
        }
    }

    /// Bytes behind a valid signature that do not decode answer 415, and an image wider than
    /// 4096 pixels 413, before anything is stored.
    #[actix_web::test]
    async fn avatar_upload_refuses_unreadable_images_and_too_many_pixels() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        let app = app!(deps);

        for (content_type, data) in [("image/png", PNG), ("image/jpeg", JPEG)] {
            let req = post(
                "/api/files/avatar",
                &[Part::File("me.png", content_type, data)],
            );
            assert_eq!(
                status_and_body!(&app, req),
                (
                    415,
                    json!({ "error": "avatar could not be read as an image" })
                ),
                "{content_type}"
            );
        }

        let wide = image_of(image::ImageFormat::Png, 4097, 1, [0, 0, 0]);
        let req = post(
            "/api/files/avatar",
            &[Part::File("me.png", "image/png", &wide)],
        );
        assert_eq!(
            status_and_body!(&app, req),
            (
                413,
                json!({ "error": "avatar image must be at most 4096×4096 pixels" })
            )
        );
    }

    /// An avatar must be a png, jpeg, gif or webp image of at most 1 MiB; a storage failure
    /// answers 500.
    #[actix_web::test]
    async fn avatar_upload_refuses_other_types_large_files_and_storage_failures() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        deps.storage
            .expect_upload_file()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Err(ServiceError::Unavailable) }));
        let app = app!(deps);
        let image_error = json!({ "error": "avatar must be an image (png, jpeg, gif, webp)" });

        for content_type in ["image/svg+xml", "text/plain"] {
            let req = post(
                "/api/files/avatar",
                &[Part::File("me.png", content_type, b"x")],
            );
            assert_eq!(
                status_and_body!(&app, req),
                (415, image_error.clone()),
                "{content_type}"
            );
        }

        assert_eq!(MAX_AVATAR_BYTES, 1024 * 1024);
        let large = vec![b'a'; MAX_AVATAR_BYTES as usize + 1];
        let req = post(
            "/api/files/avatar",
            &[Part::File("me.png", "image/png", &large)],
        );
        assert_eq!(
            status_and_body!(&app, req),
            (
                413,
                json!({ "error": format!("file exceeds {MAX_AVATAR_BYTES} byte limit") })
            )
        );

        let png = image_of(image::ImageFormat::Png, 16, 16, [1, 2, 3]);
        let req = post(
            "/api/files/avatar",
            &[Part::File("me.png", "image/png", &png)],
        );
        assert_eq!(
            status_and_body!(&app, req),
            (500, json!({ "error": "upload failed" }))
        );
    }

    /// The user service's refusal is answered before the body is read: an upload limit as 429
    /// with `Retry-After`, a lock as 403 and probation as 400, each with its message, and
    /// nothing is stored.
    #[actix_web::test]
    async fn a_refused_upload_answers_the_services_refusal_without_reading_the_body() {
        let refusals = [
            (
                ServiceError::RateLimited {
                    retry_after_seconds: 42,
                },
                429,
                json!({ "error": "rate limit exceeded", "retryAfterSeconds": 42 }),
            ),
            (
                ServiceError::Forbidden("this server's operators locked your profile".to_string()),
                403,
                json!({ "error": "this server's operators locked your profile" }),
            ),
            (
                ServiceError::Validation(
                    "a new account can set a profile picture in 5 minutes".to_string(),
                ),
                400,
                json!({ "error": "a new account can set a profile picture in 5 minutes" }),
            ),
        ];
        for (refusal, status, body) in refusals {
            let mut deps = Deps::new();
            let user_id = deps.user_id;
            let refusal = std::sync::Mutex::new(Some(refusal));
            deps.users
                .expect_begin_avatar_upload()
                .withf(move |id| *id == user_id)
                .times(1)
                .returning(move |_| {
                    let refusal = refusal.lock().unwrap().take().expect("one refusal");
                    Box::pin(async move { Err(refusal) })
                });
            deps.storage.expect_upload_file().never();
            deps.storage.expect_delete_avatar_files().never();
            let app = app!(deps);

            // A body that would otherwise be refused as too large.
            let large = vec![b'a'; MAX_AVATAR_BYTES as usize + 1];
            let resp = actix_test::call_service(
                &app,
                post(
                    "/api/files/avatar",
                    &[Part::File("me.png", "image/png", &large)],
                )
                .to_request(),
            )
            .await;
            assert_eq!(resp.status().as_u16(), status);
            let retry_after = resp
                .headers()
                .get("Retry-After")
                .map(|v| v.to_str().unwrap().to_string());
            assert_eq!(retry_after.as_deref(), (status == 429).then_some("42"));
            let got: Value = serde_json::from_slice(&actix_test::read_body(resp).await).unwrap();
            assert_eq!(got, body);
        }
    }

    // ── Profile picture check ───────────────────────────────────────────────

    /// What the stub image check service received: content type and body of each request.
    type Received = Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

    /// A loopback image check service that answers every request with `status` and `body`,
    /// and a checker of it that blocks at 0.85 and reports at 0.60.
    fn check_service(status: &'static str, body: &str) -> (Arc<AvatarChecker>, Received) {
        use std::io::{BufRead, Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let received = Received::default();
        let log = Arc::clone(&received);
        let body = body.to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
                let (mut content_type, mut len) = (String::new(), 0);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.trim().split_once(':') {
                        match name.to_ascii_lowercase().as_str() {
                            "content-type" => content_type = value.trim().to_string(),
                            "content-length" => len = value.trim().parse().unwrap_or(0),
                            _ => {}
                        }
                    }
                }
                let mut data = vec![0; len];
                let _ = reader.read_exact(&mut data);
                log.lock().unwrap().push((content_type, data));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let checker = AvatarChecker::new(&format!("http://{addr}"), 0.85, 0.60).expect("client");
        (Arc::new(checker), received)
    }

    fn scores(sexual: f64, gore: f64, violence: f64) -> String {
        json!({
            "scores": { "sexual": sexual, "gore": gore, "violence": violence },
            "model": "test",
        })
        .to_string()
    }

    /// A user service that finds `user_id` with `principal_id`.
    fn finding(deps: &mut Deps, principal_id: Option<&str>) {
        let user_id = deps.user_id;
        let principal_id = principal_id.map(str::to_string);
        deps.users
            .expect_get_users_by_ids()
            .withf(move |ids| ids == [user_id])
            .returning(move |_| {
                let user = UserDto {
                    id: user_id,
                    username: "ada".to_string(),
                    display_name: None,
                    avatar_url: None,
                    status_text: None,
                    bio: None,
                    principal_id: principal_id.clone(),
                    deletion_scheduled_for: None,
                    profile_locked_until: None,
                    handle: None,
                };
                Box::pin(async move { Ok(vec![user]) })
            });
    }

    /// Storage that stores every upload and deletes nothing else.
    fn storing_avatars(deps: &mut Deps) {
        storing(deps);
        deps.storage
            .expect_delete_avatar_files()
            .returning(|_, _| Box::pin(async { Ok(Vec::new()) }));
    }

    /// A JPEG upload and the PNG it is re-encoded to.
    fn jpeg_and_png() -> (Vec<u8>, Vec<u8>) {
        let jpeg = image_of(image::ImageFormat::Jpeg, 300, 200, [200, 30, 30]);
        let png = normalize_avatar(&jpeg, AvatarFormat::Jpeg).expect("normalized");
        (jpeg, png)
    }

    const PRINCIPAL: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";

    /// Below the report threshold the re-encoded PNG is checked, stored and not reported.
    #[actix_web::test]
    async fn a_picture_the_check_allows_is_stored_unreported() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        storing_avatars(&mut deps);
        deps.abuse.expect_file_report().never();
        let (checker, received) = check_service("200 OK", &scores(0.1, 0.59, 0.0));
        deps.checker = Some(checker);
        let app = app!(deps);
        let (jpeg, png) = jpeg_and_png();

        let req = post(
            "/api/files/avatar",
            &[Part::File("me.jpeg", "image/jpeg", &jpeg)],
        );
        assert_eq!(status_and_body!(&app, req).0, 200);
        assert_eq!(
            received.lock().unwrap().as_slice(),
            [("image/png".to_string(), png)]
        );
    }

    /// At the report threshold the picture is stored, then reported by the server against the
    /// account's principal with the category, score and the stored file's URL.
    #[actix_web::test]
    async fn a_picture_at_the_report_threshold_is_stored_and_reported() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        storing_avatars(&mut deps);
        finding(&mut deps, Some(PRINCIPAL));
        let user_id = deps.user_id;
        let (jpeg, png) = jpeg_and_png();
        let key = hashed_avatar_key(user_id, AvatarFormat::Png, &Sha256::digest(&png).into());
        let reason = format!("automatic profile picture check: gore 0.72 — /api/files/{key}");
        deps.abuse
            .expect_file_report()
            .withf(move |by, report| {
                *by == OperatorCaller::Server
                    && report.subject_principal_id.as_deref() == Some(PRINCIPAL)
                    && report.subject_server_id.is_none()
                    && report.reason == reason
            })
            .times(1)
            .returning(|_, _| Box::pin(async { Err(ServiceError::NotFound) }));
        let (checker, _) = check_service("200 OK", &scores(0.1, 0.72, 0.3));
        deps.checker = Some(checker);
        let app = app!(deps);

        // A failed report does not fail the upload.
        let req = post(
            "/api/files/avatar",
            &[Part::File("me.jpeg", "image/jpeg", &jpeg)],
        );
        let (status, body) = status_and_body!(&app, req);
        assert_eq!(status, 200);
        assert_eq!(body["path"], key);
    }

    /// A flagged picture of an account without a principal id is stored without a report.
    #[actix_web::test]
    async fn a_flagged_picture_without_a_principal_is_stored_unreported() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        storing_avatars(&mut deps);
        finding(&mut deps, None);
        deps.abuse.expect_file_report().never();
        let (checker, _) = check_service("200 OK", &scores(0.6, 0.0, 0.0));
        deps.checker = Some(checker);
        let app = app!(deps);
        let (jpeg, _) = jpeg_and_png();

        let req = post(
            "/api/files/avatar",
            &[Part::File("me.jpeg", "image/jpeg", &jpeg)],
        );
        assert_eq!(status_and_body!(&app, req).0, 200);
    }

    /// At the block threshold the answer is 422 and nothing is stored or reported.
    #[actix_web::test]
    async fn a_picture_at_the_block_threshold_is_refused_and_not_stored() {
        let mut deps = Deps::new();
        allowing(&mut deps);
        deps.storage.expect_upload_file().never();
        deps.storage.expect_delete_avatar_files().never();
        deps.abuse.expect_file_report().never();
        let (checker, _) = check_service("200 OK", &scores(0.85, 0.0, 0.0));
        deps.checker = Some(checker);
        let app = app!(deps);
        let (jpeg, _) = jpeg_and_png();

        let req = post(
            "/api/files/avatar",
            &[Part::File("me.jpeg", "image/jpeg", &jpeg)],
        );
        assert_eq!(
            status_and_body!(&app, req),
            (
                422,
                json!({ "error": "this image isn't allowed as a profile picture" })
            )
        );
    }

    /// When the check cannot run the answer is 503 and nothing is stored.
    #[actix_web::test]
    async fn an_unavailable_check_answers_503_and_stores_nothing() {
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr")
        };
        let checkers = [
            check_service("500 Internal Server Error", "{}").0,
            check_service("200 OK", "not json").0,
            Arc::new(AvatarChecker::new(&format!("http://{closed}"), 0.85, 0.6).expect("client")),
        ];
        for checker in checkers {
            let mut deps = Deps::new();
            allowing(&mut deps);
            deps.storage.expect_upload_file().never();
            deps.storage.expect_delete_avatar_files().never();
            deps.checker = Some(checker);
            let app = app!(deps);
            let (jpeg, _) = jpeg_and_png();

            let req = post(
                "/api/files/avatar",
                &[Part::File("me.jpeg", "image/jpeg", &jpeg)],
            );
            assert_eq!(
                status_and_body!(&app, req),
                (
                    503,
                    json!({ "error": "profile pictures can't be checked right now; try again later" })
                )
            );
        }
    }
}
