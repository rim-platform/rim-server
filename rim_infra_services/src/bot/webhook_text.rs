//! How the body of an incoming webhook becomes the text of a message. Pure functions: a body in,
//! text with no mention left in it out, or nothing to post.

use serde_json::Value;

use crate::notification::decide::CHANNEL_MENTION_TOKENS;

/// What a mention token is broken with: it shows as nothing.
const ZERO_WIDTH_SPACE: char = '\u{200b}';
/// The most characters of the text of a `plain` post, after trimming.
const MAX_PLAIN_CHARS: usize = 4_000;
/// The most characters of the line a GitHub event becomes.
const MAX_LINE_CHARS: usize = 1_000;
/// The longest link a line keeps, in characters. GitHub's own stay far below it.
const MAX_LINK_CHARS: usize = 512;
/// Where a link in a line may point.
const GITHUB_ORIGIN: &str = "https://github.com/";

/// A body the preset cannot read. It says why, in words a sender can act on and with nothing of
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

/// Text with Markdown characters escaped, for values taken from another service.
pub(crate) fn escape_markdown(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '<' | '>' | '#' | '~' | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(c);
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

/// The line a GitHub `event` becomes. `None` for an event or action that posts nothing, whatever
/// the body is, and for a handled one missing a field the line needs.
///
/// # Errors
/// [`DecodeError`] when a handled event's body is not JSON.
pub(crate) fn github_text(event: &str, body: &[u8]) -> Result<Option<String>, DecodeError> {
    if !matches!(event, "release" | "pull_request" | "issues") {
        return Ok(None);
    }
    let payload: Value =
        serde_json::from_slice(body).map_err(|_| DecodeError("The body must be JSON."))?;
    Ok(match event {
        "release" => release_line(&payload),
        "pull_request" => change_line(&payload, "pull_request", "pull request", true),
        _ => change_line(&payload, "issue", "issue", false),
    })
}

/// `{repo}: {sender} published release {name or tag}`, for a published release alone.
fn release_line(payload: &Value) -> Option<String> {
    if payload["action"].as_str() != Some("published") {
        return None;
    }
    let (repo, sender) = repo_and_sender(payload)?;
    let release = &payload["release"];
    let name = value_of(&release["name"]).or_else(|| value_of(&release["tag_name"]))?;
    Some(line(
        &format!("{repo}: {sender} published release {name}"),
        &release["html_url"],
    ))
}

/// `{repo}: {sender} {verb} {noun} #{number}: {title}`, for what `key` holds being opened or
/// closed. A closed one that `can_merge` and was merged is `merged`.
fn change_line(payload: &Value, key: &str, noun: &str, can_merge: bool) -> Option<String> {
    let item = &payload[key];
    let verb = match payload["action"].as_str()? {
        "opened" => "opened",
        "closed" if can_merge && item["merged"].as_bool() == Some(true) => "merged",
        "closed" => "closed",
        _ => return None,
    };
    let (repo, sender) = repo_and_sender(payload)?;
    let number = item["number"].as_u64()?;
    let title = value_of(&item["title"])?;
    Some(line(
        &format!("{repo}: {sender} {verb} {noun} #{number}: {title}"),
        &item["html_url"],
    ))
}

fn repo_and_sender(payload: &Value) -> Option<(String, String)> {
    Some((
        value_of(&payload["repository"]["full_name"])?,
        value_of(&payload["sender"]["login"])?,
    ))
}

/// A string from the payload ready for a line: on one line, with its Markdown escaped. `None`
/// when it is absent, not a string or blank.
fn value_of(value: &Value) -> Option<String> {
    let one_line = value
        .as_str()?
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!one_line.is_empty()).then(|| escape_markdown(&one_line))
}

/// `html_url` when it can be a link: a plain address under [`GITHUB_ORIGIN`] of a sensible length,
/// holding nothing Markdown or a mention could use.
fn link_of(html_url: &Value) -> Option<&str> {
    let url = html_url.as_str()?;
    let rest = url.strip_prefix(GITHUB_ORIGIN)?;
    let plain = rest.bytes().all(|b| {
        b.is_ascii_graphic()
            && !matches!(
                b,
                b'<' | b'>' | b'(' | b')' | b'[' | b']' | b'\\' | b'`' | b'"' | b'\'' | b'*' | b'|'
            )
    });
    (plain && !rest.is_empty() && url.len() <= MAX_LINK_CHARS).then_some(url)
}

/// The finished line: `text` neutralized and cut so that it and the link fit in
/// [`MAX_LINE_CHARS`]. Neutralizing comes first, so what it adds counts and the cut cannot make a
/// token, which needs its closing `>`.
fn line(text: &str, html_url: &Value) -> String {
    let link = link_of(html_url);
    let room = MAX_LINE_CHARS - link.map_or(0, |url| url.len() + 1);
    let text = neutralize_mentions(text);
    let text = cut_chars(&text, room);
    match link {
        Some(url) => format!("{text} {url}"),
        None => text.to_string(),
    }
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
        assert_eq!(github("pull_request", &payload), None);

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
                r"a\_b/c\*d: a\_b opened pull request #7: \*\*bold\*\* \_em\_ \[x\]\(https://evil.example\) \<img src=x onerror=y\> \`c\` \~\~s\~\~ \# h \| t {LINK}"
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
