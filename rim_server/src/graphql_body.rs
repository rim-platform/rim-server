//! The body of a GraphQL request: read up to a size cap and refused, before it is parsed, when
//! it could expand into more parsed values or document nodes than any request the API takes.

use actix_web::{HttpRequest, HttpResponse, web};
use async_graphql::http::MultipartOptions;
use async_graphql::{BatchRequest, ParseRequestError};

use crate::body::{BodyError, read_bounded};

/// Largest GraphQL request body. The largest request the API takes is `submitMlsCommit` with a
/// Commit and a Welcome of up to 4 MiB each, base64 in its variables (about 10.7 MiB); the rest
/// leaves room for the query and the other variables.
pub(crate) const MAX_GRAPHQL_BODY_BYTES: usize = 12 * 1024 * 1024;

/// Most `,`, `[` and `{` bytes in a GraphQL body. A JSON document holds at most one value more
/// than it has of these bytes, and each value parses into tens of bytes, so the cap bounds what
/// a body expands into at a few tens of MiB. They are counted anywhere in the body, inside
/// strings too, so no quoting hides one; base64 holds none.
pub(crate) const MAX_GRAPHQL_STRUCTURAL_BYTES: usize = 256 * 1024;

/// Most bytes of GraphQL documents in one body, every query of a batch together. A document
/// parses into a node per field, so its length bounds its parse; the schema's complexity limit
/// already refuses an operation far shorter.
pub(crate) const MAX_GRAPHQL_DOCUMENT_BYTES: usize = 128 * 1024;

/// Why a GraphQL body is refused before anything in it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Oversize {
    /// More than [`MAX_GRAPHQL_BODY_BYTES`].
    Body,
    /// More than [`MAX_GRAPHQL_STRUCTURAL_BYTES`].
    Values,
    /// More than [`MAX_GRAPHQL_DOCUMENT_BYTES`] of queries.
    Documents,
}

impl Oversize {
    fn message(self) -> &'static str {
        match self {
            Self::Body => "the request body is too large",
            Self::Values => "the request holds too many JSON values",
            Self::Documents => "the GraphQL documents are too long",
        }
    }

    /// `{"error":"too_large","message":...}`.
    pub(crate) fn body(self) -> serde_json::Value {
        serde_json::json!({ "error": "too_large", "message": self.message() })
    }

    /// `413` with [`Oversize::body`].
    pub(crate) fn response(self) -> HttpResponse {
        HttpResponse::PayloadTooLarge().json(self.body())
    }
}

/// Whether `body` fits [`MAX_GRAPHQL_BODY_BYTES`] and [`MAX_GRAPHQL_STRUCTURAL_BYTES`].
///
/// # Errors
/// The cap `body` is over.
pub(crate) fn check_body(body: &[u8]) -> Result<(), Oversize> {
    if body.len() > MAX_GRAPHQL_BODY_BYTES {
        return Err(Oversize::Body);
    }
    let structural = body
        .iter()
        .filter(|b| matches!(b, b',' | b'[' | b'{'))
        .count();
    if structural > MAX_GRAPHQL_STRUCTURAL_BYTES {
        return Err(Oversize::Values);
    }
    Ok(())
}

/// Whether the queries of `batch` fit [`MAX_GRAPHQL_DOCUMENT_BYTES`] together.
///
/// # Errors
/// [`Oversize::Documents`] when they do not.
pub(crate) fn check_documents(batch: &BatchRequest) -> Result<(), Oversize> {
    let total: usize = batch.iter().map(|request| request.query.len()).sum();
    if total > MAX_GRAPHQL_DOCUMENT_BYTES {
        return Err(Oversize::Documents);
    }
    Ok(())
}

/// The GraphQL batch a `POST /api` body holds, read and parsed as async-graphql's extractor
/// does (JSON by default, multipart by its `Content-Type`), under the caps of this module.
///
/// # Errors
/// The response to send instead: `413` past a cap, `400` for a body that could not be read or
/// does not parse.
pub(crate) async fn read_batch(
    req: &HttpRequest,
    payload: web::Payload,
) -> Result<BatchRequest, HttpResponse> {
    let body = read_bounded(req, payload, MAX_GRAPHQL_BODY_BYTES)
        .await
        .map_err(|e| match e {
            BodyError::TooLarge => Oversize::Body.response(),
            BodyError::Read => HttpResponse::BadRequest().json(serde_json::json!({
                "error": "bad_request",
                "message": "the request body could not be read",
            })),
        })?;
    check_body(&body).map_err(Oversize::response)?;
    let content_type = req
        .headers()
        .get(actix_web::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let batch = async_graphql::http::receive_batch_body(
        content_type,
        body.as_slice(),
        MultipartOptions::default(),
    )
    .await
    .map_err(|err| match err {
        ParseRequestError::PayloadTooLarge => Oversize::Body.response(),
        _ => actix_web::error::ErrorBadRequest(err).error_response(),
    })?;
    check_documents(&batch).map_err(Oversize::response)?;
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use actix_web::http::StatusCode;
    use actix_web::test::{self, TestRequest};
    use actix_web::{App, web};

    use super::*;

    /// Answers the batch's size, or the refusal.
    async fn handler(req: HttpRequest, payload: web::Payload) -> HttpResponse {
        match read_batch(&req, payload).await {
            Ok(batch) => {
                HttpResponse::Ok().json(serde_json::json!({ "requests": batch.iter().count() }))
            }
            Err(response) => response,
        }
    }

    macro_rules! app {
        () => {
            test::init_service(App::new().route("/api", web::post().to(handler))).await
        };
    }

    async fn post(body: Vec<u8>, content_type: Option<&str>) -> (StatusCode, serde_json::Value) {
        let app = app!();
        let mut req = TestRequest::post().uri("/api").set_payload(body);
        if let Some(content_type) = content_type {
            req = req.insert_header(("content-type", content_type));
        }
        let res = test::call_service(&app, req.to_request()).await;
        let status = res.status();
        let bytes = test::read_body(res).await;
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    #[actix_web::test]
    async fn single_requests_and_batches_parse_as_before() {
        let one = br#"{"query":"{ ok }"}"#.to_vec();
        assert_eq!(
            post(one.clone(), Some("application/json")).await,
            (StatusCode::OK, serde_json::json!({ "requests": 1 }))
        );
        assert_eq!(
            post(one, None).await,
            (StatusCode::OK, serde_json::json!({ "requests": 1 })),
            "JSON without a content type"
        );
        let two = br#"[{"query":"{ ok }"},{"query":"{ ok }"}]"#.to_vec();
        assert_eq!(
            post(two, Some("application/json")).await,
            (StatusCode::OK, serde_json::json!({ "requests": 2 }))
        );
        let (status, _) = post(b"{".to_vec(), Some("application/json")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "malformed JSON");
    }

    #[actix_web::test]
    async fn a_commit_and_a_welcome_at_their_caps_fit() {
        let b64 = "QUJD".repeat(4 * 1024 * 1024 / 3 + 1);
        let body = serde_json::to_vec(&serde_json::json!({
            "query": "mutation($c: String!, $w: String) { submitMlsCommit(commit: $c, welcome: $w) { epoch } }",
            "variables": { "c": b64, "w": b64 },
        }))
        .expect("json");
        assert!(body.len() > 10 * 1024 * 1024);
        assert_eq!(
            post(body, Some("application/json")).await,
            (StatusCode::OK, serde_json::json!({ "requests": 1 }))
        );
    }

    #[actix_web::test]
    async fn a_body_over_the_cap_gets_413() {
        let filler = "a".repeat(MAX_GRAPHQL_BODY_BYTES);
        let body = serde_json::to_vec(&serde_json::json!({
            "query": "{ ok }",
            "variables": { "x": filler },
        }))
        .expect("json");
        let (status, body) = post(body, Some("application/json")).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body, Oversize::Body.body());
    }

    #[actix_web::test]
    async fn a_declared_length_over_the_cap_is_refused_unread() {
        let app = app!();
        let req = TestRequest::post()
            .uri("/api")
            .insert_header(("content-length", (MAX_GRAPHQL_BODY_BYTES + 1).to_string()))
            .to_request();
        let res = test::call_service(&app, req).await;
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[actix_web::test]
    async fn a_body_of_many_small_values_gets_413_before_it_is_parsed() {
        let values = "0,".repeat(MAX_GRAPHQL_STRUCTURAL_BYTES);
        let body = format!(r#"{{"query":"{{ ok }}","variables":{{"x":[{values}0]}}}}"#);
        assert!(body.len() < 1024 * 1024);
        let (status, body) = post(body.into_bytes(), Some("application/json")).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body, Oversize::Values.body());
    }

    #[actix_web::test]
    async fn a_long_document_gets_413() {
        let query = format!("{{ {} }}", "ok ".repeat(MAX_GRAPHQL_DOCUMENT_BYTES / 3 + 1));
        let body = serde_json::to_vec(&serde_json::json!({ "query": query })).expect("json");
        let (status, body) = post(body, Some("application/json")).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body, Oversize::Documents.body());
    }

    #[test]
    fn structural_bytes_count_inside_strings_too() {
        let at_cap = ",".repeat(MAX_GRAPHQL_STRUCTURAL_BYTES);
        assert_eq!(check_body(at_cap.as_bytes()), Ok(()));
        let quoted = format!("\"{}\"", "[".repeat(MAX_GRAPHQL_STRUCTURAL_BYTES + 1));
        assert_eq!(check_body(quoted.as_bytes()), Err(Oversize::Values));
    }
}
