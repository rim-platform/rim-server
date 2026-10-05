pub mod command;
pub mod dto;
pub mod service;

/// Longest timeout a moderator may set, in days.
pub const MAX_TIMEOUT_DAYS: i64 = 30;

/// Longest slow-mode interval, in seconds (6 hours).
pub const MAX_SLOW_MODE_SECONDS: i32 = 21_600;
