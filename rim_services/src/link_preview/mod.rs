use std::sync::LazyLock;

pub mod dto;
pub mod service;

pub use service::LinkPreviewService;

/// Preview fetches one process runs at once when `RIM_LINK_PREVIEW_CONCURRENCY` is unset.
pub const DEFAULT_FETCH_CONCURRENCY: usize = 8;

/// Most preview fetches one process may run at once.
pub const MAX_FETCH_CONCURRENCY: usize = 64;

static URL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"https?://[^\s<>\[\](){}|\\^`]+").unwrap());

/// Extracts all `http://` and `https://` URLs from a text string.
pub fn extract_urls(text: &str) -> Vec<String> {
    URL_RE
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}
