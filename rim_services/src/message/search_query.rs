//! Server-side search-syntax parser for [`crate::message::command::SearchMessagesQuery`].
//!
//! **Why not the channel-lens `FilterSpec`** (`rim_core::channel_lens::model`): the lens model is
//! applied *in-memory* over a bounded batch and has neither date-range fields nor comparison
//! operators, so it is the wrong tool for a SQL full-text-search predicate. Field *names/semantics*
//! are aligned with lenses (author, has-link, has-attachment) for consistency, but this parser and
//! its SQL application (see `rim_infra_db::sea_orm_impl::message`) are new and independent.
//!
//! This module is pure (no I/O): it only tokenizes and validates the raw query string. Resolving
//! `from:`/`in:` tokens to concrete `UserId`/`ConversationId` values (and applying visibility) is
//! the service layer's job — see `DefaultMessageService::search_messages`.

use crate::{ServiceError, ServiceResult};

/// Maximum accepted length (in `char`s) for a raw search query string. Chosen to be generous for
/// legitimate queries while rejecting pathological input before it reaches Postgres.
const MAX_QUERY_LEN: usize = 512;

/// A user search string parsed into free-text terms plus structured operators.
/// Operators are stripped from the free text; the remainder becomes the tsquery.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedSearchQuery {
    /// Free-text remainder, fed to `websearch_to_tsquery`. May be empty (operator-only search).
    pub text: String,
    /// `from:@name` / `from:<uuid>` — resolved to a `UserId` by the service layer, not here.
    pub from: Option<FromToken>,
    /// `in:#channel` / `in:<uuid>` — resolved to a `ConversationId` by the service layer.
    pub in_channel: Option<ChannelToken>,
    /// `has:link`, `has:attachment` (repeatable; AND-combined).
    pub has: Vec<HasFilter>,
    /// `after:YYYY-MM-DD` — inclusive lower bound on `sent_at` (UTC midnight).
    pub after: Option<chrono::DateTime<chrono::Utc>>,
    /// `before:YYYY-MM-DD` — exclusive upper bound on `sent_at`. Treated as
    /// `sent_at < YYYY-MM-DD 00:00:00Z` (i.e. the given day itself is excluded, not included).
    pub before: Option<chrono::DateTime<chrono::Utc>>,
}

/// The `from:` operator, either an unresolved username or an already-known user id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromToken {
    Username(String),
    UserId(uuid::Uuid),
}

/// The `in:` operator, either an unresolved channel name or an already-known conversation id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelToken {
    Name(String),
    ConversationId(uuid::Uuid),
}

/// The `has:` operator's accepted values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HasFilter {
    Link,
    Attachment,
}

/// Parse a raw search string into free text plus structured operators.
///
/// Tokenizes on whitespace; a token matching `^(from|in|has|before|after):(.+)$` is treated as an
/// operator, everything else (including `key:value` pairs with an unrecognized `key`, or an
/// operator with an empty value) is free text — this parser never errors on unknown syntax,
/// matching the forgiving behavior of Postgres's `websearch_to_tsquery`. The only error cases are:
/// an invalid `before:`/`after:` date, an unrecognized `has:` value, and input longer than
/// [`MAX_QUERY_LEN`] characters.
///
/// # Errors
/// Returns [`ServiceError::Validation`] for an over-length input, a malformed `before:`/`after:`
/// date, or an unrecognized `has:` value.
pub fn parse_search_query(raw: &str) -> ServiceResult<ParsedSearchQuery> {
    if raw.chars().count() > MAX_QUERY_LEN {
        return Err(ServiceError::Validation(format!(
            "search query too long: max {MAX_QUERY_LEN} characters"
        )));
    }

    let mut parsed = ParsedSearchQuery::default();
    let mut free_text_terms: Vec<&str> = Vec::new();

    for token in raw.split_whitespace() {
        match split_operator(token) {
            Some(("from", value)) => parsed.from = Some(parse_from_token(value)),
            Some(("in", value)) => parsed.in_channel = Some(parse_channel_token(value)),
            Some(("has", value)) => parsed.has.push(parse_has_filter(value)?),
            Some(("before", value)) => parsed.before = Some(parse_date_boundary(value)?),
            Some(("after", value)) => parsed.after = Some(parse_date_boundary(value)?),
            Some(_) | None => free_text_terms.push(token),
        }
    }

    parsed.text = free_text_terms.join(" ");
    Ok(parsed)
}

/// Split a whitespace-delimited token into `(key, value)` if it looks like a recognized
/// `key:value` operator (non-empty key from the fixed operator set, non-empty value). Returns
/// `None` for anything else, which the caller treats as free text.
fn split_operator(token: &str) -> Option<(&str, &str)> {
    let colon = token.find(':')?;
    let key = &token[..colon];
    let value = &token[colon + 1..];
    if value.is_empty() {
        return None;
    }
    match key {
        "from" | "in" | "has" | "before" | "after" => Some((key, value)),
        _ => None,
    }
}

/// `from:@x` and `from:x` both parse as `Username("x")`; a value that parses as a UUID (with or
/// without a leading `@`, though `@<uuid>` is not an expected real-world input) becomes `UserId`.
fn parse_from_token(value: &str) -> FromToken {
    let stripped = value.strip_prefix('@').unwrap_or(value);
    match uuid::Uuid::parse_str(stripped) {
        Ok(uuid) => FromToken::UserId(uuid),
        Err(_) => FromToken::Username(stripped.to_string()),
    }
}

/// `in:#x` parses as `Name("x")`; a value that parses as a UUID becomes `ConversationId`.
fn parse_channel_token(value: &str) -> ChannelToken {
    let stripped = value.strip_prefix('#').unwrap_or(value);
    match uuid::Uuid::parse_str(stripped) {
        Ok(uuid) => ChannelToken::ConversationId(uuid),
        Err(_) => ChannelToken::Name(stripped.to_string()),
    }
}

/// `has:link` → `Link`; `has:attachment` or `has:file` → `Attachment`; anything else is rejected
/// (unlike unknown top-level `key:`, an unrecognized *value* for a known `has:` operator is an
/// error, not silently treated as free text — the user clearly meant to use the operator).
fn parse_has_filter(value: &str) -> ServiceResult<HasFilter> {
    match value {
        "link" => Ok(HasFilter::Link),
        "attachment" | "file" => Ok(HasFilter::Attachment),
        other => Err(ServiceError::Validation(format!(
            "unknown has: filter `{other}` (expected `link`, `attachment`, or `file`)"
        ))),
    }
}

/// Parse a `before:`/`after:` value as `YYYY-MM-DD`, returning UTC midnight of that date.
fn parse_date_boundary(value: &str) -> ServiceResult<chrono::DateTime<chrono::Utc>> {
    let date = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
        ServiceError::Validation(format!("invalid date `{value}`, expected YYYY-MM-DD"))
    })?;
    Ok(date.and_time(chrono::NaiveTime::MIN).and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_free_text_has_no_operators() {
        let parsed = parse_search_query("deploy staging").unwrap();
        assert_eq!(parsed.text, "deploy staging");
        assert_eq!(parsed.from, None);
        assert_eq!(parsed.in_channel, None);
        assert!(parsed.has.is_empty());
        assert_eq!(parsed.after, None);
        assert_eq!(parsed.before, None);
    }

    #[test]
    fn from_username_with_at_prefix() {
        let parsed = parse_search_query("from:@alice").unwrap();
        assert_eq!(parsed.from, Some(FromToken::Username("alice".to_string())));
        assert_eq!(parsed.text, "");
    }

    #[test]
    fn from_username_without_at_prefix() {
        let parsed = parse_search_query("from:alice").unwrap();
        assert_eq!(parsed.from, Some(FromToken::Username("alice".to_string())));
    }

    #[test]
    fn from_uuid_resolves_to_user_id() {
        let id = uuid::Uuid::now_v7();
        let parsed = parse_search_query(&format!("from:{id}")).unwrap();
        assert_eq!(parsed.from, Some(FromToken::UserId(id)));
    }

    #[test]
    fn in_channel_name_with_hash_prefix() {
        let parsed = parse_search_query("in:#general").unwrap();
        assert_eq!(
            parsed.in_channel,
            Some(ChannelToken::Name("general".to_string()))
        );
    }

    #[test]
    fn in_channel_uuid_resolves_to_conversation_id() {
        let id = uuid::Uuid::now_v7();
        let parsed = parse_search_query(&format!("in:{id}")).unwrap();
        assert_eq!(parsed.in_channel, Some(ChannelToken::ConversationId(id)));
    }

    #[test]
    fn has_link_parses() {
        let parsed = parse_search_query("has:link").unwrap();
        assert_eq!(parsed.has, vec![HasFilter::Link]);
    }

    #[test]
    fn has_attachment_and_file_both_parse_to_attachment() {
        let parsed = parse_search_query("has:attachment has:file").unwrap();
        assert_eq!(
            parsed.has,
            vec![HasFilter::Attachment, HasFilter::Attachment]
        );
    }

    #[test]
    fn unknown_has_value_is_validation_error() {
        let err = parse_search_query("has:bogus").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn before_and_after_parse_to_utc_midnight() {
        let parsed = parse_search_query("after:2026-01-01 before:2026-02-01").unwrap();
        assert_eq!(
            parsed.after,
            Some(
                chrono::NaiveDate::from_ymd_opt(2026, 1, 1)
                    .unwrap()
                    .and_time(chrono::NaiveTime::MIN)
                    .and_utc()
            )
        );
        assert_eq!(
            parsed.before,
            Some(
                chrono::NaiveDate::from_ymd_opt(2026, 2, 1)
                    .unwrap()
                    .and_time(chrono::NaiveTime::MIN)
                    .and_utc()
            )
        );
    }

    #[test]
    fn bad_date_is_validation_error() {
        let err = parse_search_query("after:not-a-date").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));

        let err = parse_search_query("before:2026-13-40").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn unknown_key_is_treated_as_free_text() {
        let parsed = parse_search_query("priority:high deploy").unwrap();
        assert_eq!(parsed.text, "priority:high deploy");
        assert_eq!(parsed.from, None);
    }

    #[test]
    fn operator_with_empty_value_is_treated_as_free_text() {
        let parsed = parse_search_query("from: deploy").unwrap();
        assert_eq!(parsed.text, "from: deploy");
        assert_eq!(parsed.from, None);
    }

    #[test]
    fn mixed_free_text_and_operators() {
        let parsed = parse_search_query("deploy from:@bob in:#ops has:link staging").unwrap();
        assert_eq!(parsed.text, "deploy staging");
        assert_eq!(parsed.from, Some(FromToken::Username("bob".to_string())));
        assert_eq!(
            parsed.in_channel,
            Some(ChannelToken::Name("ops".to_string()))
        );
        assert_eq!(parsed.has, vec![HasFilter::Link]);
    }

    #[test]
    fn operator_only_query_has_empty_free_text() {
        let parsed = parse_search_query("from:@alice has:link").unwrap();
        assert_eq!(parsed.text, "");
        assert!(parsed.from.is_some());
    }

    #[test]
    fn over_length_query_is_validation_error() {
        let long = "a".repeat(MAX_QUERY_LEN + 1);
        let err = parse_search_query(&long).unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn max_length_query_is_accepted() {
        let exact = "a".repeat(MAX_QUERY_LEN);
        assert!(parse_search_query(&exact).is_ok());
    }

    #[test]
    fn empty_query_parses_to_empty_everything() {
        let parsed = parse_search_query("").unwrap();
        assert_eq!(parsed, ParsedSearchQuery::default());
    }
}
