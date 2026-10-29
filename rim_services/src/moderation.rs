pub mod command;
pub mod dto;
pub mod service;

/// Longest timeout a moderator may set, in days.
pub const MAX_TIMEOUT_DAYS: i64 = 30;

/// Longest slow-mode interval, in seconds (6 hours).
pub const MAX_SLOW_MODE_SECONDS: i32 = 21_600;

/// Most messages one delete_member_messages call deletes; the caller calls again while has_more.
pub const MAX_MEMBER_MESSAGES_PER_DELETE: u32 = 1000;

/// How long a verified report's content is kept after the report is resolved or dismissed.
pub const FRANKED_CONTENT_RETENTION_DAYS: i64 = 90;
