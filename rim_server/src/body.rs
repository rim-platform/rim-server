//! Reading a request body up to a size limit.

use actix_web::http::header;
use actix_web::{HttpRequest, web};
use futures_util::StreamExt;

/// Why a body could not be read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BodyError {
    /// The declared `Content-Length` or the bytes received exceed the limit.
    TooLarge,
    /// The connection failed while the body was arriving.
    Read,
}

/// The whole body of `req`, refused as soon as its declared `Content-Length` or the bytes
/// received so far exceed `limit`.
pub(crate) async fn read_bounded(
    req: &HttpRequest,
    mut payload: web::Payload,
    limit: usize,
) -> Result<Vec<u8>, BodyError> {
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|len| len > limit) {
        return Err(BodyError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = payload.next().await {
        let chunk = chunk.map_err(|_| BodyError::Read)?;
        if body.len() + chunk.len() > limit {
            return Err(BodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
