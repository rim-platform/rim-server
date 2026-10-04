/// Largest attachment a client may declare or upload (25 MiB).
pub const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;

/// How long an upload ticket stays valid. A pending upload holds its declared size against the
/// workspace quota for this long, and can't be completed afterwards.
pub const UPLOAD_TICKET_TTL_MINUTES: i64 = 15;

pub mod command;
pub mod dto;
pub mod service;
