//! How the body of an incoming webhook becomes the text of a message. Pure functions: a body in,
//! text with no mention left in it out, or nothing to post. The pieces a format's message is made
//! of are here too.

use serde_json::Value;

use crate::display_name::is_control_or_format;
use crate::notification::decide::CHANNEL_MENTION_TOKENS;

/// What a mention token is broken with: it shows as nothing.
const ZERO_WIDTH_SPACE: char = '\u{200b}';
/// The most characters of the text of a `plain` post, after trimming.
const MAX_PLAIN_CHARS: usize = 4_000;
/// The most characters of a message a format makes, its link included.
const MAX_LINE_CHARS: usize = 1_000;
/// The longest link a message keeps, in characters. GitHub's own stay far below it.
pub(super) const MAX_LINK_CHARS: usize = 512;
/// The most characters of a delivery's value a message reads, after its leading whitespace: a
/// message holds no more.
const MAX_VALUE_CHARS: usize = MAX_LINE_CHARS;
/// How far past [`MAX_LINE_CHARS`] a message's text is filled before the rest is left out:
/// enough for neutralizing, which looks a few characters ahead, to treat the kept part as it
/// treats the whole.
pub(super) const MAX_FILLED_CHARS: usize = MAX_LINE_CHARS + 16;

/// A body the webhook cannot read. It says why, in words a sender can act on and with nothing of
/// the body in it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DecodeError(pub(crate) &'static str);

/// Text with mention tokens made plain, so a webhook never notifies anyone: a zero-width space goes
/// between the `<` and the `@` of everything that could begin a `<@id>` token, and after the `@` of
/// a mention of everyone, whatever its case and whether or not it stands as a whole word.
pub(crate) fn neutralize_mentions(text: &str) -> String {
    let mut neutral = String::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        neutral.push(c);
        let rest = &text[at..];
        let begins_token = (c == '<' && rest[1..].starts_with('@'))
            || (c == '@'
                && CHANNEL_MENTION_TOKENS
                    .iter()
                    .any(|token| begins_with(rest, token)));
        if begins_token {
            neutral.push(ZERO_WIDTH_SPACE);
        }
    }
    neutral
}

/// Whether `text` begins with `token`, whose letters are ASCII, in any case.
fn begins_with(text: &str, token: &str) -> bool {
    text.get(..token.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(token))
}

/// Text with Markdown characters escaped, for values taken from another service. The `:` of a
/// `://` and the `.` of a `www.` are escaped too, so the text renders the same but no client turns
/// it into a link.
pub(crate) fn escape_markdown(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    let mut skip = 0;
    for (at, c) in text.char_indices() {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let rest = &text[at..];
        if c == ':' && rest.starts_with("://") {
            escaped.push_str("\\:");
        } else if begins_with(rest, "www.") {
            escaped.push_str(&rest[..3]);
            escaped.push_str("\\.");
            skip = 3;
        } else if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '<' | '>' | '#' | '~' | '|'
        ) {
            escaped.push('\\');
            escaped.push(c);
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// The text of a `plain` post: a JSON object with a string `text`, at most [`MAX_PLAIN_CHARS`]
/// after trimming, with its Markdown left as the sender wrote it. `None` when it is empty.
///
/// # Errors
/// [`DecodeError`] for any other body.
pub(crate) fn plain_text(body: &[u8]) -> Result<Option<String>, DecodeError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| DecodeError("The body must be JSON with a string `text`."))?;
    let text = value
        .as_object()
        .and_then(|object| object.get("text"))
        .and_then(Value::as_str)
        .ok_or(DecodeError("The body must be JSON with a string `text`."))?
        .trim();
    if text.chars().count() > MAX_PLAIN_CHARS {
        return Err(DecodeError("`text` is over 4,000 characters."));
    }
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(neutralize_mentions(text)))
}

/// A value from a delivery ready for a message: a string that isn't blank, read up to its first
/// [`MAX_VALUE_CHARS`] characters after its leading whitespace, a number or a boolean, on one
/// line, with whitespace and control characters turned into single spaces, format characters
/// removed and its Markdown escaped. `None` for anything else, and for a string left blank by
/// this.
pub(super) fn value_of(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(text) => first_chars(text.trim_start(), MAX_VALUE_CHARS).to_string(),
        other => raw_of(other)?,
    };
    let cleaned: String = text
        .chars()
        .filter_map(|c| {
            if c.is_whitespace() || c.is_control() {
                Some(' ')
            } else if is_control_or_format(c) {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    let one_line = cleaned
        .split(' ')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!one_line.is_empty()).then(|| escape_markdown(&one_line))
}

/// A value from a delivery as it was sent: a string that isn't blank, a number in decimal or a
/// boolean as `true` or `false`. `None` for anything else.
fn raw_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// A value from a delivery for a link, as it was sent, as [`raw_of`] reads it but with at most
/// one character more than [`MAX_LINK_CHARS`]: a longer one could not make a link anyway.
pub(super) fn link_value_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => {
            Some(first_chars(text, MAX_LINK_CHARS + 1).to_string())
        }
        other => raw_of(other),
    }
}

/// Whether a value from a delivery, as [`raw_of`] reads it, is exactly `expected`, compared
/// without copying it.
pub(super) fn raw_equals(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(text) => !text.trim().is_empty() && text == expected,
        Value::Number(_) | Value::Bool(_) => raw_of(value).as_deref() == Some(expected),
        _ => false,
    }
}

/// `text` up to its first `max` characters.
fn first_chars(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// `url` as the link a message ends with, when it can be one: a plain address under `origin`
/// holding nothing Markdown or a `<@id>` token could use, with the `@` of a mention of everyone
/// percent-encoded, and at most [`MAX_LINK_CHARS`] long after that.
pub(super) fn link_of(url: &str, origin: &str) -> Option<String> {
    let rest = url.strip_prefix(origin)?;
    let plain = rest.bytes().all(|b| {
        b.is_ascii_graphic()
            && !matches!(
                b,
                b'<' | b'>' | b'(' | b')' | b'[' | b']' | b'\\' | b'`' | b'"' | b'\'' | b'*' | b'|'
            )
    });
    if !plain || rest.is_empty() {
        return None;
    }
    let link = format!("{origin}{}", encode_mentions(rest));
    (link.len() <= MAX_LINK_CHARS).then_some(link)
}

/// `text` with each `@` that begins a mention of everyone, in any case, written as `%40`: an
/// address reads it as the same `@`, and a message shows no mention. Any other `@` stays.
fn encode_mentions(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        let begins_token = c == '@'
            && CHANNEL_MENTION_TOKENS
                .iter()
                .any(|token| begins_with(&text[at..], token));
        if begins_token {
            encoded.push_str("%40");
        } else {
            encoded.push(c);
        }
    }
    encoded
}

/// `text` neutralized and cut so that it and `link`, after a space, fit in [`MAX_LINE_CHARS`].
/// Neutralizing comes first, so what it adds counts and the cut cannot make a token, which needs
/// its closing `>`.
pub(super) fn fitted(text: &str, link: Option<&str>) -> String {
    let room = MAX_LINE_CHARS - link.map_or(0, |url| url.chars().count() + 1);
    let text = neutralize_mentions(text);
    cut_chars(&text, room).to_string()
}

/// `text` cut to at most `max` characters on a character boundary, without a backslash left at the
/// end escaping what follows it.
fn cut_chars(text: &str, max: usize) -> &str {
    let Some((end, _)) = text.char_indices().nth(max) else {
        return text;
    };
    let cut = &text[..end];
    let trailing = cut.len() - cut.trim_end_matches('\\').len();
    if trailing % 2 == 1 {
        &cut[..cut.len() - 1]
    } else {
        cut
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::bot::webhook_format::{GITHUB, render};
    use crate::mention::parse_mention_ids;
    use crate::notification::decide::contains_whole_word;

    use super::*;

    const LINK: &str = "https://github.com/acme/widgets/pull/7";
    const MENTION: &str = "<@0190b5a0-0000-7000-8000-000000000001>";

    fn pull_request(action: &str, merged: bool) -> Value {
        json!({
            "action": action,
            "repository": { "full_name": "acme/widgets" },
            "sender": { "login": "ada" },
            "pull_request": { "number": 7, "title": "Fix it", "html_url": LINK, "merged": merged },
        })
    }

    fn issue(action: &str) -> Value {
        json!({
            "action": action,
            "repository": { "full_name": "acme/widgets" },
            "sender": { "login": "ada" },
            "issue": {
                "number": 9,
                "title": "It broke",
                "html_url": "https://github.com/acme/widgets/issues/9",
            },
        })
    }

    fn release(action: &str) -> Value {
        json!({
            "action": action,
            "repository": { "full_name": "acme/widgets" },
            "sender": { "login": "ada" },
            "release": {
                "name": "Spring",
                "tag_name": "v1.2.0",
                "html_url": "https://github.com/acme/widgets/releases/tag/v1.2.0",
            },
        })
    }

    /// What a delivery with the `X-GitHub-Event` header `event` and `body` posts under the shipped
    /// GitHub format.
    fn github_text(event: &str, body: &[u8]) -> Result<Option<String>, DecodeError> {
        let format = GITHUB.parsed().expect("the shipped format");
        let headers = [("x-github-event".to_string(), event.to_string())];
        render(&format, &headers, body).map(|rendered| rendered.message())
    }

    fn github(event: &str, payload: &Value) -> Option<String> {
        github_text(event, payload.to_string().as_bytes()).expect("a readable body")
    }

    /// `payload` with the field at `path` removed.
    fn without(mut payload: Value, path: &[&str]) -> Value {
        let (last, parents) = path.split_last().expect("a path");
        let mut at = &mut payload;
        for key in parents {
            at = &mut at[*key];
        }
        at.as_object_mut().expect("an object").remove(*last);
        payload
    }

    // ── Mentions and escaping ────────────────────────────────────────────

    #[test]
    fn neutralizing_leaves_nothing_the_parser_finds_and_shows_the_same_characters() {
        for text in [
            MENTION.to_string(),
            format!("a {MENTION} b {MENTION}"),
            format!("<{MENTION}>"),
            format!("<@{MENTION}"),
            format!("\\{MENTION}"),
            "<@not-an-id>".to_string(),
        ] {
            let neutral = neutralize_mentions(&text);
            assert!(parse_mention_ids(&neutral).is_empty(), "{neutral}");
            assert!(!neutral.contains("<@"), "{neutral}");
            assert_eq!(neutral.replace('\u{200b}', ""), text);
        }
    }

    /// Whether the notification rules would take `text` to mention everyone.
    fn mentions_everyone(text: &str) -> bool {
        let lower = text.to_lowercase();
        CHANNEL_MENTION_TOKENS
            .iter()
            .any(|token| contains_whole_word(&lower, token))
    }

    #[test]
    fn a_mention_of_everyone_is_neutralized_in_any_case_and_position() {
        for text in [
            "@here",
            "@channel",
            "@HERE now",
            "hey @Channel!",
            "(@hErE)",
            "@here @channel",
            "a @@here b",
            "<@here>",
            "\\@here",
            "é@here",
        ] {
            let neutral = neutralize_mentions(text);
            assert!(!mentions_everyone(&neutral), "{neutral}");
            assert_eq!(neutral.replace(ZERO_WIDTH_SPACE, ""), text);
        }
        assert!(mentions_everyone("hey @Channel!"), "the check sees them");
        assert_eq!(neutralize_mentions("me@example.com"), "me@example.com");
        assert_eq!(neutralize_mentions("@ada"), "@ada");
    }

    #[test]
    fn no_character_lowercases_into_a_letter_of_a_mention_of_everyone() {
        let letters: Vec<char> = CHANNEL_MENTION_TOKENS
            .iter()
            .flat_map(|token| token.chars())
            .collect();
        for c in (char::MIN..=char::MAX).filter(|c| !c.is_ascii()) {
            assert!(
                !c.to_lowercase().any(|lower| letters.contains(&lower)),
                "{c:?} would hide from an ASCII comparison"
            );
        }
    }

    #[test]
    fn a_mention_of_everyone_in_a_body_goes_plain_in_both_presets() {
        let body = json!({ "text": "@here the deploy is done, @CHANNEL" }).to_string();
        let text = plain_text(body.as_bytes()).expect("read").expect("text");
        assert!(!mentions_everyone(&text), "{text}");

        let mut payload = pull_request("opened", false);
        payload["pull_request"]["title"] = json!("Ping @here");
        payload["sender"]["login"] = json!("@channel");
        let line = github("pull_request", &payload).expect("a line");
        assert!(!mentions_everyone(&line), "{line}");
    }

    #[test]
    fn every_markdown_character_is_escaped() {
        assert_eq!(
            escape_markdown(r"\ ` * _ [ ] ( ) < > # ~ |"),
            r"\\ \` \* \_ \[ \] \( \) \< \> \# \~ \|"
        );
        assert_eq!(escape_markdown("> quote"), r"\> quote");
        assert_eq!(
            escape_markdown("Fix https://evil.example/x and WWW.evil.example"),
            r"Fix https\://evil.example/x and WWW\.evil.example"
        );
        assert_eq!(
            escape_markdown("plain words, ünïcode"),
            "plain words, ünïcode"
        );
    }

    // ── plain ────────────────────────────────────────────────────────────

    #[test]
    fn a_plain_body_keeps_its_markdown_and_loses_its_mentions() {
        let body = json!({ "text": format!("  **done** {MENTION}  ") }).to_string();
        let text = plain_text(body.as_bytes()).expect("read").expect("text");
        assert!(text.starts_with("**done** <\u{200b}@"), "{text}");
        assert!(parse_mention_ids(&text).is_empty());
    }

    #[test]
    fn empty_text_posts_nothing() {
        for text in ["", "   ", "\n\t"] {
            let body = json!({ "text": text }).to_string();
            assert_eq!(plain_text(body.as_bytes()), Ok(None));
        }
    }

    #[test]
    fn a_plain_body_of_any_other_shape_is_refused() {
        for body in [
            "not json",
            "[]",
            "\"text\"",
            "{}",
            r#"{"text": 7}"#,
            r#"{"text": null}"#,
            "",
        ] {
            assert!(plain_text(body.as_bytes()).is_err(), "{body}");
        }
    }

    #[test]
    fn the_plain_limit_counts_characters_after_trimming() {
        let at_limit = "é".repeat(MAX_PLAIN_CHARS);
        let body = json!({ "text": format!("  {at_limit}  ") }).to_string();
        assert_eq!(plain_text(body.as_bytes()), Ok(Some(at_limit.clone())));

        let body = json!({ "text": format!("{at_limit}é") }).to_string();
        assert!(plain_text(body.as_bytes()).is_err());
    }

    // ── GitHub: each event and action ────────────────────────────────────

    #[test]
    fn a_published_release_is_a_line_with_its_name_or_its_tag() {
        let link = "https://github.com/acme/widgets/releases/tag/v1.2.0";
        assert_eq!(
            github("release", &release("published")).expect("a line"),
            format!(r"acme/widgets: ada published release Spring {link}")
        );
        for payload in [
            without(release("published"), &["release", "name"]),
            {
                let mut unnamed = release("published");
                unnamed["release"]["name"] = json!("");
                unnamed
            },
            {
                let mut unnamed = release("published");
                unnamed["release"]["name"] = Value::Null;
                unnamed
            },
        ] {
            assert_eq!(
                github("release", &payload).expect("a line"),
                format!("acme/widgets: ada published release v1.2.0 {link}")
            );
        }
    }

    #[test]
    fn a_pull_request_is_opened_closed_or_merged() {
        for (action, merged, verb) in [
            ("opened", false, "opened"),
            ("closed", false, "closed"),
            ("closed", true, "merged"),
        ] {
            assert_eq!(
                github("pull_request", &pull_request(action, merged)).expect("a line"),
                format!("acme/widgets: ada {verb} pull request #7: Fix it {LINK}")
            );
        }
    }

    #[test]
    fn an_issue_is_opened_or_closed() {
        for action in ["opened", "closed"] {
            assert_eq!(
                github("issues", &issue(action)).expect("a line"),
                format!(
                    "acme/widgets: ada {action} issue #9: It broke \
                     https://github.com/acme/widgets/issues/9"
                )
            );
        }
    }

    #[test]
    fn other_actions_and_events_post_nothing() {
        for action in ["created", "edited", "deleted", "prereleased", "released"] {
            assert_eq!(github("release", &release(action)), None, "{action}");
        }
        for action in ["edited", "reopened", "synchronize", "labeled"] {
            assert_eq!(
                github("pull_request", &pull_request(action, false)),
                None,
                "{action}"
            );
            assert_eq!(github("issues", &issue(action)), None, "{action}");
        }
        for event in ["ping", "push", "star", "workflow_run", "", "ISSUES"] {
            assert_eq!(github(event, &issue("opened")), None, "{event}");
        }
    }

    #[test]
    fn an_issue_with_merged_set_stays_closed() {
        let mut payload = issue("closed");
        payload["issue"]["merged"] = json!(true);
        let line = github("issues", &payload).expect("a line");
        assert!(line.contains("ada closed issue"), "{line}");
    }

    #[test]
    fn every_field_a_line_needs_absent_posts_nothing() {
        for path in [
            &["action"][..],
            &["repository"][..],
            &["repository", "full_name"][..],
            &["sender"][..],
            &["sender", "login"][..],
            &["release"][..],
        ] {
            assert_eq!(
                github("release", &without(release("published"), path)),
                None
            );
        }
        let mut no_name = without(release("published"), &["release", "name"]);
        no_name = without(no_name, &["release", "tag_name"]);
        assert_eq!(github("release", &no_name), None);

        for path in [
            &["action"][..],
            &["repository", "full_name"][..],
            &["sender", "login"][..],
            &["pull_request"][..],
            &["pull_request", "number"][..],
            &["pull_request", "title"][..],
        ] {
            let payload = without(pull_request("opened", false), path);
            assert_eq!(github("pull_request", &payload), None, "{path:?}");
        }
        for path in [
            &["issue"][..],
            &["issue", "number"][..],
            &["issue", "title"][..],
        ] {
            assert_eq!(github("issues", &without(issue("opened"), path)), None);
        }
    }

    #[test]
    fn a_field_of_the_wrong_type_or_blank_posts_nothing() {
        let mut payload = pull_request("opened", false);
        payload["pull_request"]["number"] = json!("7");
        assert_eq!(
            github("pull_request", &payload).as_deref(),
            Some(format!("acme/widgets: ada opened pull request #7: Fix it {LINK}").as_str())
        );

        let mut payload = pull_request("opened", false);
        payload["pull_request"]["title"] = json!(" \n ");
        assert_eq!(github("pull_request", &payload), None);

        let mut payload = pull_request("opened", false);
        payload["sender"] = json!("ada");
        assert_eq!(github("pull_request", &payload), None);
    }

    #[test]
    fn a_handled_event_with_a_body_that_is_not_json_is_refused_and_others_are_not() {
        for event in ["release", "pull_request", "issues"] {
            assert!(github_text(event, b"payload=%7B%7D").is_err(), "{event}");
        }
        assert_eq!(github_text("ping", b"payload=%7B%7D"), Ok(None));
        assert_eq!(github_text("push", b""), Ok(None));
    }

    // ── GitHub: what is in a value ───────────────────────────────────────

    #[test]
    fn markdown_and_html_in_a_value_are_escaped() {
        let mut payload = pull_request("opened", false);
        payload["pull_request"]["title"] = json!(
            "**bold** _em_ [x](https://evil.example) <img src=x onerror=y> `c` ~~s~~ # h | t"
        );
        payload["sender"]["login"] = json!("a_b");
        payload["repository"]["full_name"] = json!("a_b/c*d");
        let line = github("pull_request", &payload).expect("a line");
        assert_eq!(
            line,
            format!(
                r"a\_b/c\*d: a\_b opened pull request #7: \*\*bold\*\* \_em\_ \[x\]\(https\://evil.example\) \<img src=x onerror=y\> \`c\` \~\~s\~\~ \# h \| t {LINK}"
            )
        );
    }

    #[test]
    fn a_value_stays_on_one_line() {
        let mut payload = pull_request("opened", false);
        payload["pull_request"]["title"] = json!("one\n# two\r\n\tthree\u{0007}four\u{2028}five");
        let line = github("pull_request", &payload).expect("a line");
        assert!(line.contains(r"#7: one \# two three four five "), "{line}");
        assert!(!line.contains('\n'));
    }

    #[test]
    fn a_mention_in_any_value_gives_no_mention() {
        for field in ["title", "login", "full_name"] {
            let mut payload = pull_request("opened", false);
            match field {
                "title" => payload["pull_request"]["title"] = json!(MENTION),
                "login" => payload["sender"]["login"] = json!(MENTION),
                _ => payload["repository"]["full_name"] = json!(MENTION),
            }
            let line = github("pull_request", &payload).expect("a line");
            assert!(parse_mention_ids(&line).is_empty(), "{line}");
            assert!(!line.contains("<@"), "{line}");
        }
    }

    #[test]
    fn a_mention_split_across_two_values_gives_no_mention() {
        let mut payload = pull_request("opened", false);
        payload["repository"]["full_name"] = json!("<@");
        payload["sender"]["login"] = json!(">");
        let line = github("pull_request", &payload).expect("a line");
        assert!(parse_mention_ids(&line).is_empty(), "{line}");
    }

    // ── GitHub: links ────────────────────────────────────────────────────

    #[test]
    fn a_link_anywhere_else_or_holding_anything_odd_leaves_the_line_without_one() {
        for url in [
            json!("https://evil.example/acme"),
            json!("http://github.com/acme/widgets/pull/7"),
            json!("https://github.com.evil.example/acme"),
            json!("https://github.com@evil.example/acme"),
            json!("https://github.com/"),
            json!("javascript:alert(1)"),
            json!(format!("https://github.com/a {MENTION}")),
            json!(format!("https://github.com/{MENTION}")),
            json!("https://github.com/a)[b](https://evil.example"),
            json!("https://github.com/a\nb"),
            json!(format!("https://github.com/{}", "a".repeat(MAX_LINK_CHARS))),
            json!(7),
            Value::Null,
        ] {
            let mut payload = pull_request("opened", false);
            payload["pull_request"]["html_url"] = url.clone();
            assert_eq!(
                github("pull_request", &payload).expect("a line"),
                "acme/widgets: ada opened pull request #7: Fix it",
                "{url}"
            );
        }
        let line = github(
            "pull_request",
            &without(pull_request("opened", false), &["pull_request", "html_url"]),
        );
        assert_eq!(
            line.as_deref(),
            Some("acme/widgets: ada opened pull request #7: Fix it")
        );
    }

    #[test]
    fn a_link_github_really_makes_is_kept() {
        for url in [
            "https://github.com/acme/widgets/releases/tag/pkg@1.0.0",
            "https://github.com/acme/widgets/releases/tag/v1.0%2Bbuild",
            "https://github.com/a-b/c_d.e/issues/1#issuecomment-2",
        ] {
            let mut payload = pull_request("opened", false);
            payload["pull_request"]["html_url"] = json!(url);
            let line = github("pull_request", &payload).expect("a line");
            assert!(line.ends_with(&format!(" {url}")), "{line}");
        }
    }

    #[test]
    fn a_mention_of_everyone_in_a_github_link_is_percent_encoded() {
        let mut payload = pull_request("opened", false);
        payload["pull_request"]["html_url"] = json!("https://github.com/acme/@here/pull/@Channel");
        let line = github("pull_request", &payload).expect("a line");
        assert!(
            line.ends_with(" https://github.com/acme/%40here/pull/%40Channel"),
            "{line}"
        );
        assert!(!mentions_everyone(&line), "{line}");
    }

    #[test]
    fn a_link_is_measured_after_its_mentions_are_encoded() {
        let origin = "https://example.com/";
        let fits = format!("{origin}{}", "a".repeat(MAX_LINK_CHARS - origin.len()));
        assert_eq!(link_of(&fits, origin).as_deref(), Some(fits.as_str()));
        let over = format!(
            "{origin}@here{}",
            "a".repeat(MAX_LINK_CHARS - origin.len() - 5)
        );
        assert_eq!(over.len(), MAX_LINK_CHARS);
        assert_eq!(link_of(&over, origin), None);
        assert_eq!(
            link_of("https://example.com/@here", origin).as_deref(),
            Some("https://example.com/%40here")
        );
        assert_eq!(
            link_of(&format!("https://example.com/{MENTION}"), origin),
            None
        );
    }

    // ── GitHub: the limit ────────────────────────────────────────────────

    #[test]
    fn a_long_line_is_cut_on_a_character_boundary_and_keeps_its_link() {
        let mut payload = pull_request("opened", false);
        payload["pull_request"]["title"] = json!("é".repeat(MAX_LINE_CHARS * 2));
        let line = github("pull_request", &payload).expect("a line");
        assert_eq!(line.chars().count(), MAX_LINE_CHARS);
        assert!(line.ends_with(&format!("é {LINK}")), "{line}");

        let mut payload = without(pull_request("opened", false), &["pull_request", "html_url"]);
        payload["pull_request"]["title"] = json!("🦀".repeat(MAX_LINE_CHARS * 2));
        let line = github("pull_request", &payload).expect("a line");
        assert_eq!(line.chars().count(), MAX_LINE_CHARS);
        assert!(line.ends_with('🦀'));
    }

    #[test]
    fn a_short_enough_line_is_not_cut() {
        let prefix = "acme/widgets: ada opened pull request #7: ";
        let mut payload = without(pull_request("opened", false), &["pull_request", "html_url"]);
        let title = "é".repeat(MAX_LINE_CHARS - prefix.len());
        payload["pull_request"]["title"] = json!(title);
        assert_eq!(
            github("pull_request", &payload).expect("a line"),
            format!("{prefix}{title}")
        );
    }

    #[test]
    fn a_line_full_of_mentions_stays_within_the_limit_and_mentions_nobody() {
        let mut payload = pull_request("opened", false);
        payload["pull_request"]["title"] = json!(MENTION.repeat(100));
        let line = github("pull_request", &payload).expect("a line");
        assert!(line.chars().count() <= MAX_LINE_CHARS);
        assert!(parse_mention_ids(&line).is_empty(), "{line}");
    }

    #[test]
    fn a_cut_leaves_no_backslash_escaping_the_link() {
        assert_eq!(cut_chars(r"abc\*def", 4), "abc");
        assert_eq!(cut_chars(r"abc\\def", 5), r"abc\\");
        assert_eq!(cut_chars("short", 10), "short");
        assert_eq!(cut_chars("exact", 5), "exact");
    }
}
