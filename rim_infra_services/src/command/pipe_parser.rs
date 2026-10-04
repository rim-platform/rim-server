use rim_services::{ServiceError, ServiceResult};
use serde_json::Value;

// SECURITY: All keys from the previous stage's output pass through to the next
// stage without filtering. A crafted first stage could inject unexpected keys
// into downstream commands. This is acceptable for Phase 1 because builtin
// commands extract only the args they declare and ignore unknown keys. Phase 2
// should introduce whitelist filtering based on the receiving command's declared
// arguments so that only expected keys propagate.
pub(super) fn merge_pipe_args(piped: Option<&Value>, explicit: Option<&Value>) -> Option<Value> {
    match (piped, explicit) {
        (None, None) => None,
        (Some(p), None) => Some(p.clone()),
        (None, Some(e)) => Some(e.clone()),
        (Some(p), Some(e)) => {
            let mut merged = match p {
                Value::Object(m) => m.clone(),
                _ => serde_json::Map::new(),
            };
            if let Value::Object(explicit_map) = e {
                for (k, v) in explicit_map {
                    merged.insert(k.clone(), v.clone());
                }
            }
            Some(Value::Object(merged))
        }
    }
}

const MAX_PIPE_SEGMENTS: usize = 10;

#[derive(Debug)]
pub(super) struct PipeSegment {
    pub(super) command_id: String,
    pub(super) args: Option<Value>,
}

/// Parse a pipe expression like "cmd1 | cmd2 | cmd3" into segments.
///
/// Each segment is a command_id optionally followed by key=value args.
/// Returns error for empty segments, trailing pipes, or exceeding MAX_PIPE_SEGMENTS.
pub(super) fn parse_pipe_expression(expr: &str) -> ServiceResult<Vec<PipeSegment>> {
    let raw_segments: Vec<&str> = expr.split('|').collect();

    if raw_segments.is_empty() {
        return Err(ServiceError::Validation(
            "pipe expression is empty".to_string(),
        ));
    }

    if raw_segments.len() > MAX_PIPE_SEGMENTS {
        return Err(ServiceError::Validation(format!(
            "pipe chain exceeds maximum of {} segments",
            MAX_PIPE_SEGMENTS
        )));
    }

    let mut segments = Vec::with_capacity(raw_segments.len());

    for (i, raw) in raw_segments.iter().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ServiceError::Validation(format!(
                "empty pipe segment at position {}",
                i + 1
            )));
        }

        let mut parts = trimmed.split_whitespace();
        let command_id = parts
            .next()
            .ok_or_else(|| {
                ServiceError::Validation(format!("empty pipe segment at position {}", i + 1))
            })?
            .to_string();

        if command_id.len() > 256 {
            return Err(ServiceError::Validation(format!(
                "command ID in pipe segment {} exceeds maximum length of 256 characters",
                i + 1
            )));
        }

        let mut args_map = serde_json::Map::new();
        for token in parts {
            if let Some((key, value)) = token.split_once('=') {
                if key.is_empty() {
                    return Err(ServiceError::Validation(format!(
                        "invalid key=value pair in segment {}: '{}'",
                        i + 1,
                        token
                    )));
                }
                args_map.insert(key.to_string(), Value::String(value.to_string()));
            } else {
                return Err(ServiceError::Validation(format!(
                    "invalid argument '{}' in segment {} — expected key=value format",
                    token,
                    i + 1
                )));
            }
        }

        let args = if args_map.is_empty() {
            None
        } else {
            Some(Value::Object(args_map))
        };

        segments.push(PipeSegment { command_id, args });
    }

    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_command() {
        let segments = parse_pipe_expression("chat.post_message").unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].command_id, "chat.post_message");
        assert!(segments[0].args.is_none());
    }

    #[test]
    fn parse_two_commands() {
        let segments = parse_pipe_expression("cmd1 | cmd2").unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].command_id, "cmd1");
        assert_eq!(segments[1].command_id, "cmd2");
    }

    #[test]
    fn parse_with_args() {
        let segments =
            parse_pipe_expression("chat.post_message text=hello | notify channel=#ops").unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].command_id, "chat.post_message");
        let args0 = segments[0].args.as_ref().unwrap();
        assert_eq!(args0["text"], "hello");
        assert_eq!(segments[1].command_id, "notify");
        let args1 = segments[1].args.as_ref().unwrap();
        assert_eq!(args1["channel"], "#ops");
    }

    #[test]
    fn parse_trailing_pipe_error() {
        let result = parse_pipe_expression("cmd1 |");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn parse_empty_segment_error() {
        let result = parse_pipe_expression("cmd1 | | cmd2");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn parse_whitespace_handling() {
        let segments = parse_pipe_expression("  cmd1  |  cmd2  ").unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].command_id, "cmd1");
        assert_eq!(segments[1].command_id, "cmd2");
    }

    #[test]
    fn parse_max_exceeded() {
        let expr = (1..=11)
            .map(|i| format!("cmd{}", i))
            .collect::<Vec<_>>()
            .join(" | ");
        let result = parse_pipe_expression(&expr);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn parse_exactly_max_segments_succeeds() {
        let expr = (1..=10)
            .map(|i| format!("cmd{}", i))
            .collect::<Vec<_>>()
            .join(" | ");
        let segments = parse_pipe_expression(&expr).unwrap();
        assert_eq!(segments.len(), 10);
    }

    #[test]
    fn parse_empty_expression_error() {
        let result = parse_pipe_expression("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_command_id_too_long() {
        let long_id = "a".repeat(257);
        let result = parse_pipe_expression(&long_id);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn parse_multiple_args_per_segment() {
        let segments = parse_pipe_expression("cmd1 key1=val1 key2=val2").unwrap();
        assert_eq!(segments.len(), 1);
        let args = segments[0].args.as_ref().unwrap();
        assert_eq!(args["key1"], "val1");
        assert_eq!(args["key2"], "val2");
    }
}
