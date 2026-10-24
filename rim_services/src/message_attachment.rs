/// Largest attachment a client may declare or upload (25 MiB).
pub const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;

/// How long an upload ticket stays valid. A pending upload holds its declared size against the
/// workspace quota for this long, and can't be completed afterwards.
pub const UPLOAD_TICKET_TTL_MINUTES: i64 = 15;

/// Whether a plaintext attachment's media type is supported.
pub fn is_plaintext_content_type(content_type: &str) -> bool {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    ["image/", "text/", "audio/", "video/"]
        .iter()
        .any(|prefix| media_type.starts_with(prefix) && media_type.len() > prefix.len())
        || matches!(
            media_type.as_str(),
            "application/pdf" | "application/json" | "application/zip" | "application/gzip"
        )
}

pub mod command;
pub mod dto;
pub mod service;

#[cfg(test)]
mod tests {
    use super::is_plaintext_content_type;

    #[test]
    fn accepts_media_and_documents_but_reserves_opaque_blobs_for_encryption() {
        for kind in [
            "video/mp4",
            "audio/ogg",
            "image/png",
            "text/plain; charset=utf-8",
            "APPLICATION/PDF",
            "application/json",
            "application/zip",
            "application/gzip",
        ] {
            assert!(is_plaintext_content_type(kind), "{kind}");
        }
        for kind in [
            "",
            "video/",
            "application/octet-stream",
            "application/x-executable",
            "application/pdf-executable",
        ] {
            assert!(!is_plaintext_content_type(kind), "{kind}");
        }
    }
}
