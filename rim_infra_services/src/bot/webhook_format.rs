//! Webhook formats: rules an Owner or Moderator writes as JSON that say when a delivery posts and
//! what. Pure functions: [`parse_format`] checks a format once, and [`render`] turns a delivery's
//! headers and body into the message it posts, or says why nothing posts.

use std::fmt;

use serde::Deserialize;
use serde_json::Value;

use super::webhook_text::{
    DecodeError, MAX_FILLED_CHARS, MAX_LINK_CHARS, fitted, link_of, link_value_of, raw_equals,
    value_of,
};

/// The most rules a format holds.
const MAX_RULES: usize = 20;
/// The most conditions a rule holds.
const MAX_CONDITIONS: usize = 5;
/// The most characters of a rule's `post` as written.
const MAX_POST_CHARS: usize = 1_000;
/// The most placeholders a rule holds, in its `post` and its link together.
const MAX_PLACEHOLDERS: usize = 30;
/// The most characters of a path.
const MAX_PATH_CHARS: usize = 256;
/// The most characters of a header's name.
const MAX_HEADER_NAME_CHARS: usize = 64;
/// The largest format, as JSON, in bytes.
const MAX_FORMAT_BYTES: usize = 16 * 1024;
/// What a header's name may not contain, in any case: the usual places for a shared secret.
const REFUSED_HEADER_WORDS: &[&str] = &["token", "secret", "signature", "auth", "cookie", "key"];

/// Why a format is refused, with the number of the rule it is about, counted from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FormatError {
    pub(crate) rule: Option<usize>,
    pub(crate) reason: String,
}

impl FormatError {
    fn whole(reason: impl Into<String>) -> Self {
        Self {
            rule: None,
            reason: reason.into(),
        }
    }

    fn at(rule: usize, reason: impl Into<String>) -> Self {
        Self {
            rule: Some(rule),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.rule {
            Some(rule) => write!(f, "Rule {rule}: {}", self.reason),
            None => f.write_str(&self.reason),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFormat {
    version: u32,
    rules: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    #[serde(default)]
    when: Vec<RawCondition>,
    post: String,
    #[serde(default)]
    link: Option<RawLink>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCondition {
    header: Option<String>,
    field: Option<String>,
    equals: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLink {
    value: String,
    origin: String,
}

/// A format that passed every check, ready to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebhookFormat {
    rules: Vec<WebhookRule>,
}

/// One rule: when it holds, what it posts and the link after it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WebhookRule {
    when: Vec<WebhookCondition>,
    post: Template,
    link: Option<WebhookLink>,
    /// Whether a condition or a placeholder of this rule reads the body.
    reads_body: bool,
}

/// A test of one header or one field of the body.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WebhookCondition {
    Header { name: String, equals: String },
    Field { path: Vec<String>, equals: String },
}

/// A link added after the text when what it becomes starts with `origin`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WebhookLink {
    value: Template,
    origin: String,
}

/// Text with placeholders, split into its pieces.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Template {
    pieces: Vec<Piece>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    /// `{a|b}`: the first of its paths that has a value. `written` is what is between the braces.
    Placeholder {
        written: String,
        paths: Vec<Vec<String>>,
    },
}

impl Template {
    fn placeholders(&self) -> usize {
        self.pieces
            .iter()
            .filter(|piece| matches!(piece, Piece::Placeholder { .. }))
            .count()
    }

    /// The text with each placeholder filled by `fill`, or the placeholder that had no value.
    /// Once the text is over `limit` characters, the pieces after it are left out, though every
    /// placeholder must still have a value.
    fn fill(
        &self,
        payload: &Value,
        fill: impl Fn(&Value) -> Option<String>,
        limit: usize,
    ) -> Result<String, String> {
        let mut text = String::new();
        let mut chars = 0;
        for piece in &self.pieces {
            let value;
            let piece_text = match piece {
                Piece::Text(literal) => literal.as_str(),
                Piece::Placeholder { written, paths } => {
                    value = paths
                        .iter()
                        .find_map(|path| read(payload, path).and_then(&fill))
                        .ok_or_else(|| written.clone())?;
                    value.as_str()
                }
            };
            if chars <= limit {
                text.push_str(piece_text);
                chars += piece_text.chars().count();
            }
        }
        Ok(text)
    }
}

/// What a delivery comes to under a format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Rendered {
    /// Rule `rule`, counted from 1, held: `text` has no mention left in it and fits with `link`.
    Post {
        rule: usize,
        text: String,
        link: Option<String>,
    },
    /// No rule held.
    NoRule,
    /// Rule `rule` held, and the placeholder written as `placeholder` had no value.
    NoValue { rule: usize, placeholder: String },
}

impl Rendered {
    /// The message to post: the text, then the link after a space. `None` when nothing posts.
    pub(crate) fn message(&self) -> Option<String> {
        match self {
            Self::Post {
                text,
                link: Some(link),
                ..
            } => Some(format!("{text} {link}")),
            Self::Post { text, .. } => Some(text.clone()),
            Self::NoRule | Self::NoValue { .. } => None,
        }
    }
}

/// `json` checked as a format: every limit, an unknown key anywhere and every refused header name
/// refused, naming the rule.
///
/// # Errors
/// [`FormatError`] for the first problem found.
pub(crate) fn parse_format(json: &Value) -> Result<WebhookFormat, FormatError> {
    let bytes = serde_json::to_vec(json).map_or(usize::MAX, |bytes| bytes.len());
    if bytes > MAX_FORMAT_BYTES {
        return Err(FormatError::whole("The format is over 16 KiB."));
    }
    let raw = RawFormat::deserialize(json).map_err(|e| FormatError::whole(e.to_string()))?;
    if raw.version != 1 {
        return Err(FormatError::whole("Only `version` 1 is known."));
    }
    if raw.rules.len() > MAX_RULES {
        return Err(FormatError::whole("A format has at most 20 rules."));
    }
    let rules = raw
        .rules
        .iter()
        .enumerate()
        .map(|(at, rule)| {
            let number = at + 1;
            let raw =
                RawRule::deserialize(rule).map_err(|e| FormatError::at(number, e.to_string()))?;
            parse_rule(raw).map_err(|reason| FormatError::at(number, reason))
        })
        .collect::<Result<_, _>>()?;
    Ok(WebhookFormat { rules })
}

fn parse_rule(raw: RawRule) -> Result<WebhookRule, String> {
    if raw.when.len() > MAX_CONDITIONS {
        return Err("A rule has at most 5 conditions.".to_string());
    }
    let when = raw
        .when
        .into_iter()
        .map(parse_condition)
        .collect::<Result<Vec<_>, _>>()?;
    if raw.post.chars().count() > MAX_POST_CHARS {
        return Err("`post` is over 1,000 characters.".to_string());
    }
    if raw.post.trim().is_empty() {
        return Err("`post` is blank.".to_string());
    }
    let post = parse_template(&raw.post)?;
    let link = raw
        .link
        .map(|link| {
            if !is_origin(&link.origin) {
                return Err(format!(
                    "`{}` is not an origin like `https://example.com/`.",
                    link.origin
                ));
            }
            Ok(WebhookLink {
                value: parse_template(&link.value)?,
                origin: link.origin,
            })
        })
        .transpose()?;
    let placeholders =
        post.placeholders() + link.as_ref().map_or(0, |link| link.value.placeholders());
    if placeholders > MAX_PLACEHOLDERS {
        return Err("A rule has at most 30 placeholders.".to_string());
    }
    let reads_body = placeholders > 0
        || when
            .iter()
            .any(|condition| matches!(condition, WebhookCondition::Field { .. }));
    Ok(WebhookRule {
        when,
        post,
        link,
        reads_body,
    })
}

fn parse_condition(raw: RawCondition) -> Result<WebhookCondition, String> {
    match (raw.header, raw.field) {
        (Some(name), None) => {
            check_header_name(&name)?;
            Ok(WebhookCondition::Header {
                name,
                equals: raw.equals,
            })
        }
        (None, Some(field)) => Ok(WebhookCondition::Field {
            path: parse_path(&field)?,
            equals: raw.equals,
        }),
        _ => Err("A condition has either `header` or `field`.".to_string()),
    }
}

/// A header name a condition may test: `x-` and none of [`REFUSED_HEADER_WORDS`], in any case.
fn check_header_name(name: &str) -> Result<(), String> {
    if name.chars().count() > MAX_HEADER_NAME_CHARS {
        return Err("A header name is at most 64 characters.".to_string());
    }
    let lower = name.to_ascii_lowercase();
    if !lower.starts_with("x-") {
        return Err(format!(
            "Only headers starting with `x-` can be used, not `{name}`."
        ));
    }
    if let Some(word) = refused_word(&lower) {
        return Err(format!(
            "The header `{name}` can't be used: a name holding `{word}` usually carries a secret."
        ));
    }
    Ok(())
}

/// The first of [`REFUSED_HEADER_WORDS`] that `lower`, a header name in lowercase, contains.
fn refused_word(lower: &str) -> Option<&'static str> {
    REFUSED_HEADER_WORDS
        .iter()
        .find(|word| lower.contains(*word))
        .copied()
}

/// Whether a format may not test the header `name`, in any case, because it usually carries a
/// secret. A test session drops such a header by the same list.
pub(crate) fn is_refused_header(name: &str) -> bool {
    refused_word(&name.to_ascii_lowercase()).is_some()
}

/// Keys with dots between them, each part `[A-Za-z0-9_-]+`.
fn parse_path(written: &str) -> Result<Vec<String>, String> {
    if written.chars().count() > MAX_PATH_CHARS {
        return Err("A path is at most 256 characters.".to_string());
    }
    let parts: Vec<String> = written.split('.').map(str::to_string).collect();
    let good = parts.iter().all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    });
    if !good {
        return Err(format!(
            "`{written}` is not a path: keys with dots between them, each of letters, digits, `_` and `-`."
        ));
    }
    Ok(parts)
}

/// Literal text with `{path}` and `{a|b}` placeholders, and `{{` and `}}` for braces. Anything
/// else in braces and a lone brace are refused.
fn parse_template(text: &str) -> Result<Template, String> {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'{' | b'}' if bytes.get(at + 1) == Some(&bytes[at]) => {
                literal.push_str(&text[start..=at]);
                at += 2;
                start = at;
            }
            b'{' => {
                literal.push_str(&text[start..at]);
                let close = text[at + 1..]
                    .find('}')
                    .ok_or("A `{` has no `}`. Write `{{` for a brace.")?;
                let written = &text[at + 1..at + 1 + close];
                let paths = written
                    .split('|')
                    .map(parse_path)
                    .collect::<Result<Vec<_>, _>>()?;
                if !literal.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut literal)));
                }
                pieces.push(Piece::Placeholder {
                    written: written.to_string(),
                    paths,
                });
                at += close + 2;
                start = at;
            }
            b'}' => return Err("A `}` has no `{`. Write `}}` for a brace.".to_string()),
            _ => at += 1,
        }
    }
    literal.push_str(&text[start..]);
    if !literal.is_empty() {
        pieces.push(Piece::Text(literal));
    }
    Ok(Template { pieces })
}

/// Whether `origin` is `https://`, a host, an optional port and a single `/`, and nothing else.
fn is_origin(origin: &str) -> bool {
    let Some(authority) = origin
        .strip_prefix("https://")
        .and_then(|rest| rest.strip_suffix('/'))
    else {
        return false;
    };
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let host_ok = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    let port_ok = port.is_none_or(|port| {
        !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok()
    });
    host_ok && port_ok
}

/// The value at `path` in `payload`: a key of an object, or an index of an array.
fn read<'a>(payload: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(payload, |at, part| match at {
        Value::Object(object) => object.get(part),
        Value::Array(items) => part
            .parse::<usize>()
            .ok()
            .and_then(|index| items.get(index)),
        _ => None,
    })
}

/// Whether one of `headers` has `name`, in any case, and exactly the value `equals`.
fn header_holds(headers: &[(String, String)], name: &str, equals: &str) -> bool {
    headers
        .iter()
        .any(|(header, value)| header.eq_ignore_ascii_case(name) && value == equals)
}

/// What a delivery of `headers` and `body` comes to under `format`. Header conditions are checked
/// first, and the body is parsed only when a rule that could still hold reads it.
///
/// # Errors
/// [`DecodeError`] when a rule that could still hold reads a body that isn't JSON.
pub(crate) fn render(
    format: &WebhookFormat,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<Rendered, DecodeError> {
    let mut parsed: Option<Value> = None;
    for (at, rule) in format.rules.iter().enumerate() {
        let number = at + 1;
        let headers_hold = rule.when.iter().all(|condition| match condition {
            WebhookCondition::Header { name, equals } => header_holds(headers, name, equals),
            WebhookCondition::Field { .. } => true,
        });
        if !headers_hold {
            continue;
        }
        if rule.reads_body && parsed.is_none() {
            parsed = Some(
                serde_json::from_slice(body).map_err(|_| DecodeError("The body must be JSON."))?,
            );
        }
        let payload = parsed.as_ref().unwrap_or(&Value::Null);
        let fields_hold = rule.when.iter().all(|condition| match condition {
            WebhookCondition::Header { .. } => true,
            WebhookCondition::Field { path, equals } => {
                read(payload, path).is_some_and(|value| raw_equals(value, equals))
            }
        });
        if !fields_hold {
            continue;
        }
        let text = match rule.post.fill(payload, value_of, MAX_FILLED_CHARS) {
            Ok(text) => text,
            Err(placeholder) => {
                return Ok(Rendered::NoValue {
                    rule: number,
                    placeholder,
                });
            }
        };
        let link = rule.link.as_ref().and_then(|link| {
            let value = link
                .value
                .fill(payload, link_value_of, MAX_LINK_CHARS)
                .ok()?;
            link_of(&value, &link.origin)
        });
        return Ok(Rendered::Post {
            rule: number,
            text: fitted(&text, link.as_deref()),
            link,
        });
    }
    Ok(Rendered::NoRule)
}

/// A ready-made format RIM ships, with what a client shows beside it.
pub(crate) struct ShippedFormat {
    pub(crate) key: &'static str,
    pub(crate) name: &'static str,
    pub(crate) setup_note: &'static str,
    /// The format as JSON.
    pub(crate) format: &'static str,
    pub(crate) sample_headers: &'static [(&'static str, &'static str)],
    /// A delivery's body as the service sends it.
    pub(crate) sample_body: &'static str,
}

impl ShippedFormat {
    /// The format as JSON.
    ///
    /// # Errors
    /// The parser's error for a file that isn't JSON, which the tests rule out.
    pub(crate) fn json(&self) -> Result<Value, serde_json::Error> {
        serde_json::from_str(self.format)
    }

    /// The format, checked.
    ///
    /// # Errors
    /// [`FormatError`] for a file that doesn't pass, which the tests rule out.
    pub(crate) fn parsed(&self) -> Result<WebhookFormat, FormatError> {
        let json = self
            .json()
            .map_err(|_| FormatError::whole("The shipped format is not JSON."))?;
        parse_format(&json)
    }
}

/// GitHub's releases, pull requests and issues, read by every webhook of the preset `github`.
pub(crate) const GITHUB: ShippedFormat = ShippedFormat {
    key: "github",
    name: "GitHub",
    setup_note: "In the repository's Settings > Webhooks, paste this as the Payload URL, choose \
                 application/json, and leave Secret empty. RIM posts releases, pull requests and \
                 issues.",
    format: include_str!("webhook_formats/github.json"),
    sample_headers: &[("X-GitHub-Event", "pull_request")],
    sample_body: include_str!("webhook_formats/github.sample.json"),
};

/// Grafana's alert notifications, as its webhook contact point sends them.
pub(crate) const GRAFANA: ShippedFormat = ShippedFormat {
    key: "grafana",
    name: "Grafana",
    setup_note: "In Grafana, under Alerting > Contact points, add a Webhook integration with this \
                 as the URL and the method POST. RIM posts firing and resolved alerts.",
    format: include_str!("webhook_formats/grafana.json"),
    sample_headers: &[],
    sample_body: include_str!("webhook_formats/grafana.sample.json"),
};

/// Every ready-made format, in the order a client offers them.
pub(crate) const SHIPPED: [&ShippedFormat; 2] = [&GITHUB, &GRAFANA];

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use crate::mention::parse_mention_ids;
    use crate::notification::decide::{CHANNEL_MENTION_TOKENS, contains_whole_word};

    use super::*;

    const MENTION: &str = "<@0190b5a0-0000-7000-8000-000000000001>";

    fn format(json: Value) -> WebhookFormat {
        parse_format(&json).expect("a format that passes")
    }

    fn refusal(json: Value) -> FormatError {
        parse_format(&json).expect_err("a format that is refused")
    }

    /// A format of one rule with `when` that posts `post`.
    fn one_rule(when: Value, post: &str) -> Value {
        json!({ "version": 1, "rules": [{ "when": when, "post": post }] })
    }

    fn deliver(format: &WebhookFormat, headers: &[(&str, &str)], body: &str) -> Rendered {
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        render(format, &headers, body.as_bytes()).expect("a readable body")
    }

    fn posted(rendered: &Rendered) -> String {
        rendered.message().expect("a post")
    }

    fn mentions_everyone(text: &str) -> bool {
        let lower = text.to_lowercase();
        CHANNEL_MENTION_TOKENS
            .iter()
            .any(|token| contains_whole_word(&lower, token))
    }

    // ── Rules ────────────────────────────────────────────────────────────

    #[test]
    fn the_first_rule_that_holds_decides_and_the_rest_are_not_read() {
        let format = format(json!({ "version": 1, "rules": [
            { "when": [{ "field": "kind", "equals": "a" }], "post": "first" },
            { "when": [{ "field": "kind", "equals": "a" }], "post": "second" },
            { "post": "fallback" },
        ]}));
        assert_eq!(
            deliver(&format, &[], r#"{"kind":"a"}"#),
            Rendered::Post {
                rule: 1,
                text: "first".to_string(),
                link: None
            }
        );
        assert_eq!(
            posted(&deliver(&format, &[], r#"{"kind":"b"}"#)),
            "fallback"
        );
    }

    #[test]
    fn no_rule_holding_posts_nothing() {
        let format = format(one_rule(json!([{ "field": "kind", "equals": "a" }]), "x"));
        assert_eq!(deliver(&format, &[], r#"{"kind":"b"}"#), Rendered::NoRule);
        assert_eq!(deliver(&format, &[], "{}"), Rendered::NoRule);
        assert_eq!(
            format.clone(),
            parse_format(&one_rule(json!([{ "field": "kind", "equals": "a" }]), "x"))
                .expect("again")
        );
    }

    #[test]
    fn a_rule_with_no_conditions_always_holds_and_needs_no_body() {
        let format = format(json!({ "version": 1, "rules": [{ "post": "**ping**" }] }));
        assert_eq!(
            posted(&deliver(&format, &[], "not json at all")),
            "**ping**"
        );
    }

    #[test]
    fn a_placeholder_with_no_value_posts_nothing_and_tries_no_later_rule() {
        let format = format(json!({ "version": 1, "rules": [
            { "post": "{title}" },
            { "post": "fallback" },
        ]}));
        for body in [
            "{}",
            r#"{"title":""}"#,
            r#"{"title":"  \n "}"#,
            r#"{"title":null}"#,
            r#"{"title":{}}"#,
            r#"{"title":[1]}"#,
            r#"{"title":"\u202e\u200b"}"#,
        ] {
            assert_eq!(
                deliver(&format, &[], body),
                Rendered::NoValue {
                    rule: 1,
                    placeholder: "title".to_string()
                },
                "{body}"
            );
        }
    }

    // ── Conditions ───────────────────────────────────────────────────────

    #[test]
    fn a_field_compares_as_text() {
        for (body, equals, holds) in [
            (r#"{"v":"open"}"#, "open", true),
            (r#"{"v":"open"}"#, "Open", false),
            (r#"{"v":7}"#, "7", true),
            (r#"{"v":-2.5}"#, "-2.5", true),
            (r#"{"v":true}"#, "true", true),
            (r#"{"v":false}"#, "false", true),
            (r#"{"v":"true"}"#, "true", true),
            (r#"{"v":null}"#, "null", false),
            (r#"{"v":{}}"#, "{}", false),
            (r#"{"v":[]}"#, "[]", false),
            ("{}", "", false),
        ] {
            let format = format(one_rule(json!([{ "field": "v", "equals": equals }]), "yes"));
            assert_eq!(
                deliver(&format, &[], body) != Rendered::NoRule,
                holds,
                "{body} {equals}"
            );
        }
    }

    #[test]
    fn a_header_matches_its_name_in_any_case_and_its_value_exactly() {
        let format = format(one_rule(
            json!([{ "header": "X-Event", "equals": "push" }]),
            "pushed",
        ));
        assert_eq!(
            posted(&deliver(&format, &[("x-event", "push")], "{}")),
            "pushed"
        );
        assert_eq!(
            posted(&deliver(&format, &[("X-EVENT", "push")], "{}")),
            "pushed"
        );
        assert_eq!(
            deliver(&format, &[("x-event", "Push")], "{}"),
            Rendered::NoRule
        );
        assert_eq!(deliver(&format, &[], "{}"), Rendered::NoRule);
    }

    #[test]
    fn a_header_that_could_carry_a_secret_is_refused_by_name() {
        for name in [
            "X-Hub-Signature-256",
            "x-gitlab-token",
            "X-Api-Key",
            "x-webhook-secret",
            "X-Authorization",
            "x-cookie-jar",
            "X-AUTH",
        ] {
            let refused = refusal(one_rule(json!([{ "header": name, "equals": "x" }]), "y"));
            assert_eq!(refused.rule, Some(1));
            assert!(refused.reason.contains(name), "{refused}");
        }
        for name in ["Authorization", "Content-Type", "github-event"] {
            let refused = refusal(one_rule(json!([{ "header": name, "equals": "x" }]), "y"));
            assert!(refused.reason.contains("x-"), "{refused}");
        }
        let long = format!("x-{}", "e".repeat(MAX_HEADER_NAME_CHARS));
        assert!(parse_format(&one_rule(json!([{ "header": long, "equals": "x" }]), "y")).is_err());
    }

    #[test]
    fn a_header_cannot_be_printed() {
        let format = format(one_rule(json!([]), "{X-Event} {x-event}"));
        assert_eq!(
            deliver(&format, &[("x-event", "secret")], "{}"),
            Rendered::NoValue {
                rule: 1,
                placeholder: "X-Event".to_string()
            }
        );
    }

    #[test]
    fn a_condition_names_one_header_or_one_field() {
        for when in [
            json!([{ "equals": "x" }]),
            json!([{ "header": "x-a", "field": "a", "equals": "x" }]),
        ] {
            assert_eq!(refusal(one_rule(when, "y")).rule, Some(1));
        }
        assert!(parse_format(&one_rule(json!([{ "field": "a", "equals": true }]), "y")).is_err());
    }

    // ── Paths ────────────────────────────────────────────────────────────

    #[test]
    fn a_path_reads_keys_and_array_indexes() {
        let format = format(one_rule(json!([]), "{alerts.0.labels.alertname} {a.1}"));
        let body = r#"{"alerts":[{"labels":{"alertname":"Disk"}}],"a":{"1":"key one"}}"#;
        assert_eq!(posted(&deliver(&format, &[], body)), "Disk key one");
        let missing = r#"{"alerts":[],"a":{"1":"x"}}"#;
        assert!(matches!(
            deliver(&format, &[], missing),
            Rendered::NoValue { .. }
        ));
    }

    #[test]
    fn a_path_that_is_not_one_is_refused() {
        for path in ["", "a..b", ".a", "a.", "a b", "a/b", "é"] {
            assert!(
                parse_format(&one_rule(json!([{ "field": path, "equals": "x" }]), "y")).is_err(),
                "{path}"
            );
        }
        let long = "a".repeat(MAX_PATH_CHARS + 1);
        assert!(parse_format(&one_rule(json!([{ "field": long, "equals": "x" }]), "y")).is_err());
        let at_limit = "a".repeat(MAX_PATH_CHARS);
        assert!(
            parse_format(&one_rule(
                json!([{ "field": at_limit, "equals": "x" }]),
                "y"
            ))
            .is_ok()
        );
    }

    // ── post ─────────────────────────────────────────────────────────────

    #[test]
    fn the_first_path_with_a_value_fills_a_placeholder() {
        let format = format(one_rule(json!([]), "release {name|tag}"));
        assert_eq!(
            posted(&deliver(&format, &[], r#"{"name":"","tag":"v1"}"#)),
            "release v1"
        );
        assert_eq!(
            posted(&deliver(&format, &[], r#"{"name":"Spring","tag":"v1"}"#)),
            "release Spring"
        );
        assert_eq!(
            deliver(&format, &[], "{}"),
            Rendered::NoValue {
                rule: 1,
                placeholder: "name|tag".to_string()
            }
        );
    }

    #[test]
    fn doubled_braces_are_literal_and_anything_else_in_braces_is_refused() {
        let format = format(one_rule(json!([]), "{{not}} {a} }}{{"));
        assert_eq!(posted(&deliver(&format, &[], r#"{"a":"x"}"#)), "{not} x }{");
        for post in [
            "{", "}", "a { b", "a } b", "{}", "{a|}", "{a b}", "{{a}", "{a}}",
        ] {
            assert!(parse_format(&one_rule(json!([]), post)).is_err(), "{post}");
        }
    }

    #[test]
    fn a_blank_or_long_post_is_refused() {
        assert!(parse_format(&one_rule(json!([]), " \n ")).is_err());
        assert!(parse_format(&one_rule(json!([]), &"é".repeat(MAX_POST_CHARS))).is_ok());
        assert!(parse_format(&one_rule(json!([]), &"é".repeat(MAX_POST_CHARS + 1))).is_err());
    }

    #[test]
    fn the_literal_text_keeps_its_markdown_and_line_breaks() {
        let format = format(one_rule(
            json!([]),
            "**Backup failed** on {host}\n> {reason}",
        ));
        assert_eq!(
            posted(&deliver(
                &format,
                &[],
                r#"{"host":"db-1","reason":"disk *full*"}"#
            )),
            "**Backup failed** on db-1\n> disk \\*full\\*"
        );
    }

    #[test]
    fn a_value_is_never_expanded_again_nor_a_mention() {
        let format = format(one_rule(json!([]), "{title} by {user}"));
        let body = json!({
            "title": format!("{{user}} {MENTION} @here [x](https://evil.example) `c`"),
            "user": "@channel",
        })
        .to_string();
        let text = posted(&deliver(&format, &[], &body));
        assert!(text.starts_with("{user} "), "{text}");
        assert!(parse_mention_ids(&text).is_empty(), "{text}");
        assert!(!text.contains("<@"), "{text}");
        assert!(!mentions_everyone(&text), "{text}");
        assert!(text.contains(r"\[x\]\(https://evil.example\)"), "{text}");
    }

    #[test]
    fn the_literal_texts_own_mentions_are_made_plain() {
        let format = format(one_rule(json!([]), &format!("@here {MENTION} look")));
        let text = posted(&deliver(&format, &[], "{}"));
        assert!(!mentions_everyone(&text), "{text}");
        assert!(parse_mention_ids(&text).is_empty(), "{text}");
    }

    #[test]
    fn a_value_loses_its_control_and_format_characters() {
        let format = format(one_rule(json!([]), "[{title}]"));
        let body = json!({ "title": "a\u{202e}b\u{200b}c\u{2066}d\nnext\u{0007}line\u{feff}" })
            .to_string();
        assert_eq!(posted(&deliver(&format, &[], &body)), "[abcd next line]");
    }

    #[test]
    fn numbers_and_booleans_are_values() {
        let format = format(one_rule(json!([]), "#{n} {ok} {x}"));
        assert_eq!(
            posted(&deliver(&format, &[], r#"{"n":7,"ok":false,"x":1.5}"#)),
            "#7 false 1.5"
        );
    }

    #[test]
    fn a_long_post_is_cut_to_the_limit_with_its_link() {
        let format = format(json!({ "version": 1, "rules": [{
            "post": "{t}",
            "link": { "value": "{u}", "origin": "https://example.com/" },
        }]}));
        let body = json!({ "t": "🦀".repeat(3_000), "u": "https://example.com/a" }).to_string();
        let message = posted(&deliver(&format, &[], &body));
        assert_eq!(message.chars().count(), 1_000);
        assert!(message.ends_with("🦀 https://example.com/a"), "{message}");
    }

    #[test]
    fn a_long_value_is_read_only_as_far_as_a_line_holds() {
        let escaped = value_of(&json!("*".repeat(1_000_000))).expect("a value");
        assert!(escaped.len() <= 2_000, "{} bytes", escaped.len());
        assert_eq!(
            value_of(&json!(format!("{}a b", " ".repeat(5_000)))).as_deref(),
            Some("a b")
        );
    }

    #[test]
    fn a_rule_does_not_expand_a_long_value_past_the_line() {
        let thirty = format(json!({ "version": 1, "rules": [{ "post": "{a}".repeat(30) }] }));
        let payload = json!({ "a": "*".repeat(1_000_000) });
        let filled = thirty.rules[0]
            .post
            .fill(&payload, value_of, MAX_FILLED_CHARS)
            .expect("filled");
        assert!(filled.len() < 10_000, "filled {} bytes", filled.len());

        // A placeholder past the cut still needs a value.
        let missing = format(json!({ "version": 1, "rules": [{
            "post": format!("{}{{b}}", "{a}".repeat(29)),
        }]}));
        assert!(matches!(
            deliver(&missing, &[], &payload.to_string()),
            Rendered::NoValue { .. }
        ));
    }

    #[test]
    fn the_message_from_a_long_value_is_what_it_was() {
        let format = format(one_rule(json!([]), "{a} and {b}"));
        let body = json!({ "a": "x".repeat(5_000), "b": "y" }).to_string();
        assert_eq!(posted(&deliver(&format, &[], &body)), "x".repeat(1_000));
    }

    #[test]
    fn a_long_link_value_is_still_no_link() {
        let format = linked("{u}", "https://example.com/");
        let body = json!({ "t": "x", "u": format!("https://example.com/{}", "a".repeat(100_000)) })
            .to_string();
        assert!(matches!(
            deliver(&format, &[], &body),
            Rendered::Post { link: None, .. }
        ));
    }

    // ── link ─────────────────────────────────────────────────────────────

    fn linked(value: &str, origin: &str) -> WebhookFormat {
        format(json!({ "version": 1, "rules": [{
            "post": "done",
            "link": { "value": value, "origin": origin },
        }]}))
    }

    #[test]
    fn a_link_is_kept_only_under_its_origin_and_otherwise_the_post_still_goes() {
        let format = linked("{url}", "https://grafana.example:3000/");
        for (url, kept) in [
            ("https://grafana.example:3000/d/abc?x=1", true),
            ("https://grafana.example:3000/", false),
            ("https://grafana.example/d/abc", false),
            ("https://grafana.example:3000.evil/d", false),
            ("https://grafana.example:3000/a b", false),
            (
                "https://grafana.example:3000/a)[b](https://evil.example",
                false,
            ),
            ("javascript:alert(1)", false),
        ] {
            let body = json!({ "url": url }).to_string();
            let expected = if kept {
                format!("done {url}")
            } else {
                "done".to_string()
            };
            assert_eq!(posted(&deliver(&format, &[], &body)), expected, "{url}");
        }
        assert_eq!(posted(&deliver(&format, &[], "{}")), "done");
    }

    #[test]
    fn a_link_value_takes_literal_text_and_placeholders() {
        let format = linked("https://ci.example/runs/{id}", "https://ci.example/");
        assert_eq!(
            posted(&deliver(&format, &[], r#"{"id":42}"#)),
            "done https://ci.example/runs/42"
        );
    }

    #[test]
    fn a_mention_of_everyone_from_the_delivery_in_a_link_is_percent_encoded() {
        let format = format(json!({ "version": 1, "rules": [{
            "when": [],
            "post": format!("x {MENTION} @channel **b** {{{{ }}}} "),
            "link": { "value": "https://example.com/{c}", "origin": "https://example.com/" },
        }]}));
        let message = posted(&deliver(&format, &[], r#"{"c":"@here"}"#));
        assert_eq!(
            message,
            "x <\u{200b}@0190b5a0-0000-7000-8000-000000000001> @\u{200b}channel **b** { }  \
             https://example.com/%40here"
        );
        assert!(!mentions_everyone(&message), "{message}");
        assert!(parse_mention_ids(&message).is_empty(), "{message}");
    }

    #[test]
    fn a_link_never_mentions_anyone_whatever_its_value_holds() {
        let format = linked("https://example.com/{c}", "https://example.com/");
        for (value, link) in [
            ("@here", Some("https://example.com/%40here")),
            ("@channel", Some("https://example.com/%40channel")),
            (
                "a/@CHANNEL/b?x=@HeRe",
                Some("https://example.com/a/%40CHANNEL/b?x=%40HeRe"),
            ),
            ("@@here", Some("https://example.com/@%40here")),
            ("pkg@1.0.0", Some("https://example.com/pkg@1.0.0")),
            ("@everyone", Some("https://example.com/@everyone")),
            (MENTION, None),
            ("@0190b5a0-0000-7000-8000-000000000001>", None),
        ] {
            let body = json!({ "c": value }).to_string();
            let rendered = deliver(&format, &[], &body);
            let Rendered::Post { link: got, .. } = &rendered else {
                panic!("a post for {value}");
            };
            assert_eq!(got.as_deref(), link, "{value}");
            let message = posted(&rendered);
            assert!(!mentions_everyone(&message), "{message}");
            assert!(parse_mention_ids(&message).is_empty(), "{message}");
        }
    }

    #[test]
    fn a_mention_of_everyone_in_a_links_literal_text_is_percent_encoded() {
        let format = linked("https://example.com/@here/{id}", "https://example.com/");
        let message = posted(&deliver(&format, &[], r#"{"id":"@Channel"}"#));
        assert_eq!(message, "done https://example.com/%40here/%40Channel");
        assert!(!mentions_everyone(&message), "{message}");
    }

    #[test]
    fn an_origin_that_is_not_one_is_refused() {
        for origin in [
            "http://example.com/",
            "https://example.com",
            "https://example.com/path/",
            "https://user@example.com/",
            "https://example.com:99999/",
            "https://example.com:/",
            "https:///",
            "https://exa mple.com/",
        ] {
            let json = json!({ "version": 1, "rules": [{
                "post": "x", "link": { "value": "{u}", "origin": origin },
            }]});
            assert!(parse_format(&json).is_err(), "{origin}");
        }
    }

    // ── When the body is read ────────────────────────────────────────────

    #[test]
    fn the_body_is_read_only_when_a_rule_that_could_still_hold_needs_it() {
        let format = format(json!({ "version": 1, "rules": [
            { "when": [{ "header": "x-event", "equals": "push" }, { "field": "a", "equals": "b" }],
              "post": "pushed" },
        ]}));
        assert_eq!(
            deliver(&format, &[("x-event", "ping")], "not json"),
            Rendered::NoRule
        );
        let headers = [("x-event".to_string(), "push".to_string())];
        assert_eq!(
            render(&format, &headers, b"payload=%7B%7D"),
            Err(DecodeError("The body must be JSON."))
        );
    }

    #[test]
    fn a_body_nested_too_deep_is_refused_without_a_crash() {
        let format = format(one_rule(json!([{ "field": "a", "equals": "b" }]), "x"));
        let deep = "[".repeat(1024 * 1024);
        assert!(render(&format, &[], deep.as_bytes()).is_err());
        let deep_object = format!("{}{}", r#"{"a":"#.repeat(200), "1");
        assert!(render(&format, &[], deep_object.as_bytes()).is_err());
    }

    // ── The whole format ─────────────────────────────────────────────────

    #[test]
    fn an_unknown_key_anywhere_or_another_version_is_refused() {
        for (json, rule) in [
            (json!({ "version": 2, "rules": [] }), None),
            (json!({ "rules": [] }), None),
            (json!({ "version": 1, "rules": [], "extra": 1 }), None),
            (
                json!({ "version": 1, "rules": [{ "post": "x", "posts": "y" }] }),
                Some(1),
            ),
            (
                json!({ "version": 1, "rules": [
                    { "post": "x" },
                    { "when": [{ "field": "a", "equal": "b" }], "post": "x" },
                ]}),
                Some(2),
            ),
            (
                json!({ "version": 1, "rules": [{ "post": "x",
                    "link": { "value": "{u}", "origin": "https://a.example/", "title": "t" } }] }),
                Some(1),
            ),
            (json!({ "version": 1, "rules": [{ "when": [] }] }), Some(1)),
            (json!([]), None),
        ] {
            assert_eq!(refusal(json.clone()).rule, rule, "{json}");
        }
    }

    #[test]
    fn every_limit_is_refused_naming_the_rule() {
        let rules: Vec<Value> = (0..=MAX_RULES).map(|_| json!({ "post": "x" })).collect();
        assert_eq!(refusal(json!({ "version": 1, "rules": rules })).rule, None);
        let rules: Vec<Value> = (0..MAX_RULES).map(|_| json!({ "post": "x" })).collect();
        assert!(parse_format(&json!({ "version": 1, "rules": rules })).is_ok());

        let when: Vec<Value> = (0..=MAX_CONDITIONS)
            .map(|_| json!({ "field": "a", "equals": "b" }))
            .collect();
        let refused = refusal(json!({ "version": 1, "rules": [
            { "post": "x" }, { "when": when, "post": "x" },
        ]}));
        assert_eq!(refused.rule, Some(2));
        assert!(refused.to_string().starts_with("Rule 2: "), "{refused}");

        let post = "{a}".repeat(MAX_PLACEHOLDERS + 1);
        assert_eq!(refusal(one_rule(json!([]), &post)).rule, Some(1));
        let post = "{a}".repeat(MAX_PLACEHOLDERS);
        assert!(parse_format(&one_rule(json!([]), &post)).is_ok());

        let big = json!({ "version": 1, "rules": [{
            "when": [{ "field": "a", "equals": "x".repeat(MAX_FORMAT_BYTES) }], "post": "x",
        }]});
        assert_eq!(refusal(big).rule, None);
    }

    // ── The shipped formats ──────────────────────────────────────────────

    #[test]
    fn every_shipped_format_passes_and_its_sample_posts() {
        for shipped in SHIPPED {
            let format = shipped.parsed().expect(shipped.key);
            let headers: Vec<(String, String)> = shipped
                .sample_headers
                .iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), (*value).to_string()))
                .collect();
            let rendered =
                render(&format, &headers, shipped.sample_body.as_bytes()).expect(shipped.key);
            assert!(
                rendered.message().is_some(),
                "{}: {rendered:?}",
                shipped.key
            );
        }
        let keys: Vec<&str> = SHIPPED.iter().map(|shipped| shipped.key).collect();
        assert_eq!(keys, ["github", "grafana"]);
    }

    #[test]
    fn the_github_sample_posts_a_merged_pull_request() {
        let format = GITHUB.parsed().expect("github");
        let headers = [("x-github-event".to_string(), "pull_request".to_string())];
        assert_eq!(
            render(&format, &headers, GITHUB.sample_body.as_bytes())
                .expect("read")
                .message()
                .as_deref(),
            Some(
                "acme/widgets: ada merged pull request #7: Fix the login redirect \
                 https://github.com/acme/widgets/pull/7"
            )
        );
    }

    #[test]
    fn grafanas_test_notification_posts_its_alert_and_summary() {
        let format = GRAFANA.parsed().expect("grafana");
        assert_eq!(
            render(&format, &[], GRAFANA.sample_body.as_bytes())
                .expect("read")
                .message()
                .as_deref(),
            Some("**Firing:** TestAlert: Notification test")
        );
        let mut resolved: Value = serde_json::from_str(GRAFANA.sample_body).expect("json");
        resolved["status"] = json!("resolved");
        resolved["commonAnnotations"] = json!({});
        resolved["alerts"][0]["annotations"] = json!({ "description": "Disk is back" });
        assert_eq!(
            render(&format, &[], resolved.to_string().as_bytes())
                .expect("read")
                .message()
                .as_deref(),
            Some("**Resolved:** TestAlert: Disk is back")
        );
    }

    // ── Properties ───────────────────────────────────────────────────────

    /// Any JSON, a few levels deep, with strings drawn to hold mentions, Markdown and braces.
    fn any_json() -> impl Strategy<Value = Value> {
        let text = prop_oneof![
            any::<String>(),
            Just(MENTION.to_string()),
            Just("@here".to_string()),
            Just("{a} }} {{ \u{202e}".to_string()),
            Just("https://example.com/x".to_string()),
            "[a-z<>@{}\\[\\]()*_`#|~\\\\ .-]{0,40}",
        ];
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| json!(n)),
            any::<f64>().prop_map(|n| json!(n)),
            text.prop_map(Value::String),
        ];
        leaf.prop_recursive(4, 48, 6, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                prop::collection::btree_map("[a-c0-2]", inner, 0..4)
                    .prop_map(|map| Value::Object(map.into_iter().collect())),
            ]
        })
    }

    /// A format of up to 3 rules over the keys `any_json` uses.
    fn any_format() -> impl Strategy<Value = Value> {
        let path = "[a-c0-2](\\.[a-c0-2]){0,2}";
        let condition = prop_oneof![
            (path, "[a-z0-9]{0,3}")
                .prop_map(|(field, equals)| json!({ "field": field, "equals": equals })),
            ("x-[a-c]", "[a-z]{0,3}")
                .prop_map(|(header, equals)| json!({ "header": header, "equals": equals })),
        ];
        let piece = prop_oneof![
            path.prop_map(|path| format!("{{{path}}}")),
            (path, path).prop_map(|(a, b)| format!("{{{a}|{b}}}")),
            Just(format!(" {MENTION} @channel **b** {{{{ }}}} ")),
            "[a-z ]{1,8}",
            Just("é".repeat(400)),
        ];
        let rule = (
            prop::collection::vec(condition, 0..3),
            prop::collection::vec(piece, 1..6),
            prop::option::of(path),
        )
            .prop_map(|(when, pieces, link)| {
                let mut rule = json!({ "when": when, "post": format!("x{}", pieces.concat()) });
                if let Some(link) = link {
                    rule["link"] =
                        json!({ "value": format!("https://example.com/{{{link}}}"), "origin": "https://example.com/" });
                }
                rule
            });
        prop::collection::vec(rule, 0..4).prop_map(|rules| json!({ "version": 1, "rules": rules }))
    }

    proptest! {
        #[test]
        fn render_never_panics_and_never_posts_a_mention_or_more_than_the_limit(
            format_json in any_format(),
            body in any_json(),
            event in "[a-z]{0,3}",
        ) {
            let Ok(format) = parse_format(&format_json) else { return Ok(()); };
            let headers = [("x-a".to_string(), event)];
            let rendered = render(&format, &headers, body.to_string().as_bytes());
            if let Ok(Rendered::Post { text, link, .. }) = &rendered {
                let message = rendered.as_ref().ok().and_then(Rendered::message).unwrap_or_default();
                prop_assert!(message.chars().count() <= 1_000, "{}", message.chars().count());
                prop_assert!(parse_mention_ids(&message).is_empty(), "{}", message);
                prop_assert!(!mentions_everyone(&message), "{}", message);
                prop_assert!(!text.is_empty());
                if let Some(link) = link {
                    prop_assert!(link.starts_with("https://example.com/"));
                }
            }
        }

        #[test]
        fn parsing_any_json_never_panics(json in any_json()) {
            let _ = parse_format(&json);
        }

        #[test]
        fn any_bytes_as_a_body_never_panic(body in prop::collection::vec(any::<u8>(), 0..256)) {
            let format = format(one_rule(json!([{ "field": "a", "equals": "b" }]), "{a}"));
            let _ = render(&format, &[], &body);
        }
    }
}
