//! Size bounds on command arguments and on what user commands, macros and pipelines store.

use serde_json::Value;

use crate::{ServiceError, ServiceResult};

/// Bytes the serialized arguments of one `executeCommand` call may take: room for the longest
/// message text `chat.post_message` accepts.
pub const MAX_EXECUTE_ARGS_BYTES: usize = 64 * 1024;

/// Bytes one command id may take.
pub const MAX_COMMAND_ID_BYTES: usize = 256;

/// Bytes a pipe expression (`executeCommand` with `isPipe`) may take, its stages' arguments
/// included.
pub const MAX_PIPE_EXPRESSION_BYTES: usize = 4096;

/// Bytes the serialized arguments of one stored step may take: a user command's or a macro's
/// step, or a pipeline's action.
pub const MAX_STEP_ARGS_BYTES: usize = 4096;

/// Characters of a stored description: a user command's, a macro's or a pipeline's.
pub const MAX_DESCRIPTION_CHARS: usize = 1000;

/// Bytes of redacted arguments one audit entry keeps; larger arguments are replaced by
/// [`bound_audit_args`].
pub const MAX_AUDIT_ARGS_BYTES: usize = 4096;

/// Bytes of the serialized arguments a truncated audit entry keeps as its prefix.
const AUDIT_ARGS_PREFIX_BYTES: usize = 1024;

/// Counts the bytes written to it and keeps none.
struct ByteCount(usize);

impl std::io::Write for ByteCount {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Bytes of `value` serialized as compact JSON, counted without building the text.
pub fn json_size(value: &Value) -> usize {
    let mut count = ByteCount(0);
    match serde_json::to_writer(&mut count, value) {
        Ok(()) => count.0,
        Err(_) => usize::MAX,
    }
}

/// Refuses `args` whose serialized form is longer than `max` bytes; `what` names them in the
/// refusal, e.g. "a macro step's args".
///
/// # Errors
///
/// `Validation` saying "`what` may be at most `max` bytes".
pub fn ensure_args_size(args: Option<&Value>, max: usize, what: &str) -> ServiceResult<()> {
    match args {
        Some(args) if json_size(args) > max => Err(ServiceError::Validation(format!(
            "{what} may be at most {max} bytes"
        ))),
        _ => Ok(()),
    }
}

/// Refuses `text` longer than `max` characters; `what` names it in the refusal.
///
/// # Errors
///
/// `Validation` saying "`what` may be at most `max` characters".
pub fn ensure_chars(text: &str, max: usize, what: &str) -> ServiceResult<()> {
    if text.chars().take(max + 1).count() > max {
        return Err(ServiceError::Validation(format!(
            "{what} may be at most {max} characters"
        )));
    }
    Ok(())
}

/// Refuses a command id longer than [`MAX_COMMAND_ID_BYTES`]; `what` names it in the refusal.
///
/// # Errors
///
/// `Validation` saying "`what` may be at most 256 bytes".
pub fn ensure_command_id_size(command_id: &str, what: &str) -> ServiceResult<()> {
    if command_id.len() > MAX_COMMAND_ID_BYTES {
        return Err(ServiceError::Validation(format!(
            "{what} may be at most {MAX_COMMAND_ID_BYTES} bytes"
        )));
    }
    Ok(())
}

/// `args` as an audit entry keeps them: unchanged up to [`MAX_AUDIT_ARGS_BYTES`] serialized,
/// otherwise `{"truncated": true, "bytes": <size>, "prefix": <first 1024 bytes>}`, the prefix
/// cut at a character boundary of the serialized text.
pub fn bound_audit_args(args: Value) -> Value {
    let size = json_size(&args);
    if size <= MAX_AUDIT_ARGS_BYTES {
        return args;
    }
    let text = args.to_string();
    let mut end = AUDIT_ARGS_PREFIX_BYTES.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    serde_json::json!({
        "truncated": true,
        "bytes": size,
        "prefix": &text[..end],
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_size_matches_serialized_length() {
        let value = json!({"text": "žluťoučký kůň", "n": [1, 2.5, null, true]});
        assert_eq!(json_size(&value), value.to_string().len());
    }

    #[test]
    fn ensure_args_size_refuses_only_past_the_cap() {
        let at_cap = json!("x".repeat(MAX_STEP_ARGS_BYTES - 2));
        assert_eq!(json_size(&at_cap), MAX_STEP_ARGS_BYTES);
        assert!(ensure_args_size(Some(&at_cap), MAX_STEP_ARGS_BYTES, "args").is_ok());
        assert!(ensure_args_size(None, MAX_STEP_ARGS_BYTES, "args").is_ok());

        let over = json!("x".repeat(MAX_STEP_ARGS_BYTES - 1));
        let err = ensure_args_size(Some(&over), MAX_STEP_ARGS_BYTES, "a step's args")
            .expect_err("over the cap");
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "a step's args may be at most 4096 bytes"),
            "{err:?}"
        );
    }

    #[test]
    fn ensure_chars_counts_characters_not_bytes() {
        assert!(ensure_chars(&"č".repeat(10), 10, "title").is_ok());
        let err = ensure_chars(&"č".repeat(11), 10, "title").expect_err("11 characters");
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "title may be at most 10 characters"),
            "{err:?}"
        );
    }

    #[test]
    fn ensure_command_id_size_refuses_past_256_bytes() {
        assert!(ensure_command_id_size(&"a".repeat(256), "a step's command id").is_ok());
        assert!(ensure_command_id_size(&"a".repeat(257), "a step's command id").is_err());
    }

    #[test]
    fn bound_audit_args_keeps_small_args() {
        let args = json!({"text": "hello"});
        assert_eq!(bound_audit_args(args.clone()), args);
    }

    #[test]
    fn bound_audit_args_truncates_large_args_at_a_char_boundary() {
        let args = json!({"text": "ž".repeat(MAX_AUDIT_ARGS_BYTES)});
        let size = json_size(&args);
        let bounded = bound_audit_args(args);
        assert_eq!(bounded["truncated"], json!(true));
        assert_eq!(bounded["bytes"], json!(size));
        let prefix = bounded["prefix"].as_str().expect("prefix");
        assert!(prefix.len() <= AUDIT_ARGS_PREFIX_BYTES);
        assert!(prefix.starts_with("{\"text\":\"žž"));
        assert!(json_size(&bounded) <= MAX_AUDIT_ARGS_BYTES);
    }
}
