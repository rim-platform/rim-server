//! Comment hygiene across the workspace: no comment in any crate's `src` or `tests` carries
//! history, a pointer to a doc, a plan, ticket or pull request reference, a finding label, a
//! leftover task marker, the first-person plural voice of a design note, a section sign or a
//! persona name. Comments say what the code is and does. Reads source files only, so it needs
//! no database.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// A class of text no comment may contain.
struct Token {
    /// The name a failure shows for this class.
    name: &'static str,
    /// The regular expression that finds it in a comment line.
    pattern: &'static str,
    /// Whether text inside backtick code spans and fenced code blocks is exempt. When the
    /// pattern has a capture group, a failure shows that group instead of the whole match.
    prose_only: bool,
}

/// Every banned token class, in the order a failure lists them.
const TOKENS: &[Token] = &[
    Token {
        name: "section sign",
        pattern: "§",
        prose_only: false,
    },
    Token {
        name: "persona name",
        pattern: r"Fury|Widow|Hulk|Iron[ -]?Man|Hawkeye|Strange|Falcon|Tony|Spider-Man|Ant-Man",
        prose_only: false,
    },
    Token {
        name: "Phase N",
        pattern: r"Phase \d",
        prose_only: false,
    },
    Token {
        name: "Track N",
        pattern: r"Track \d",
        prose_only: false,
    },
    Token {
        name: "plan item id",
        pattern: r"P\d-[A-Z]\d",
        prose_only: false,
    },
    Token {
        name: "design-note voice",
        pattern: r"(?i:\b(?:we|our|ours|ourselves)\b)|\b[Uu]s\b",
        prose_only: true,
    },
    Token {
        name: "legacy",
        pattern: r"(?i)\blegacy\b",
        prose_only: true,
    },
    Token {
        name: "follow-up",
        pattern: r"(?i)\bfollow-?ups?\b",
        prose_only: true,
    },
    Token {
        name: "previously",
        pattern: r"(?i)\bpreviously\b",
        prose_only: true,
    },
    Token {
        name: "used to",
        pattern: r"(?i)\b(?:it|this|that|they|which|who|we|there)\s+used\s+to\b|\bused\s+to\s+be\b",
        prose_only: true,
    },
    Token {
        name: "task marker",
        pattern: r"\b(?:TODO|FIXME)\b",
        prose_only: true,
    },
    Token {
        name: "pull request number",
        pattern: r"\bPRs?\s*#?\d+",
        prose_only: true,
    },
    Token {
        name: "issue reference",
        pattern: r"(?:^|[^\w&:#])(#\d+)",
        prose_only: true,
    },
    Token {
        name: "finding label",
        pattern: r"\bFL\d+",
        prose_only: true,
    },
    Token {
        name: "docs pointer",
        pattern: r"(?i)\bsee\s+`?docs/",
        prose_only: false,
    },
    Token {
        name: "spec section",
        pattern: r"(?i)\bspec(?:ification)?\s+section\b",
        prose_only: true,
    },
];

/// `TOKENS` with each pattern compiled.
static COMPILED: LazyLock<Vec<(&'static Token, Regex)>> = LazyLock::new(|| {
    TOKENS
        .iter()
        .map(|t| (t, Regex::new(t.pattern).expect("token pattern compiles")))
        .collect()
});

/// A backtick code span on one line.
static CODE_SPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`[^`\n]*`").expect("code span pattern compiles"));

/// One banned token found in a comment.
#[derive(Debug, PartialEq)]
struct Offence {
    /// The 1-based source line the comment text is on.
    line: usize,
    /// The name of the token class.
    token: &'static str,
    /// The text that matched.
    found: String,
}

#[test]
fn no_comment_carries_history_references_or_personas() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("rim_server sits inside the workspace root");

    let mut files = Vec::new();
    for crate_dir in fs::read_dir(root).expect("read workspace root").flatten() {
        let crate_dir = crate_dir.path();
        if !crate_dir.join("Cargo.toml").is_file() {
            continue;
        }
        for sub in ["src", "tests"] {
            collect_rust_files(&crate_dir.join(sub), &mut files);
        }
    }
    files.sort();
    assert!(
        !files.is_empty(),
        "no Rust sources found under {}",
        root.display()
    );

    let mut report = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("read Rust source");
        let lines: Vec<&str> = text.lines().collect();
        let shown = file.strip_prefix(root).unwrap_or(file).display();
        for offence in offences(&text) {
            let source_line = lines.get(offence.line - 1).map_or("", |l| l.trim());
            report.push(format!(
                "{shown}:{}: {} `{}` in: {source_line}",
                offence.line, offence.token, offence.found
            ));
        }
    }

    assert!(
        report.is_empty(),
        "comments say what the code is and does: no history, no pointers to docs, plans, \
         tickets or pull requests, no finding labels, no task markers, no first-person plural \
         voice, no section signs and no persona names. Rewrite each comment below:\n{}",
        report.join("\n")
    );
}

/// Every banned token in the comments of `source`.
///
/// Prose-only token classes skip backtick code spans and the lines of fenced code blocks
/// inside a run of comment lines.
fn offences(source: &str) -> Vec<Offence> {
    let mut found = Vec::new();
    let mut in_fence = false;
    let mut previous_line = 0;
    for (line, text) in comments(source) {
        if line != previous_line + 1 && line != previous_line {
            in_fence = false;
        }
        previous_line = line;
        let fence_marker = text.trim_start().trim_start_matches(['/', '!', '*']);
        let is_fence = fence_marker.trim_start().starts_with("```");
        if is_fence {
            in_fence = !in_fence;
        }
        let prose = if is_fence || in_fence {
            String::new()
        } else {
            CODE_SPAN.replace_all(&text, " ").into_owned()
        };
        for (token, regex) in COMPILED.iter() {
            let haystack = if token.prose_only { &prose } else { &text };
            if let Some(caps) = regex.captures(haystack) {
                let m = caps.get(1).or_else(|| caps.get(0));
                found.push(Offence {
                    line,
                    token: token.name,
                    found: m.map_or_else(String::new, |m| m.as_str().to_string()),
                });
            }
        }
    }
    found
}

/// Every comment in `source` as its 1-based line number and the text after the comment
/// opener, one entry per source line a comment covers. String, raw string and char literals
/// are skipped, so a `//` inside one does not open a comment.
fn comments(source: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = source.chars().collect();
    let at = |i: usize| chars.get(i).copied();
    let mut out = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while let Some(c) = at(i) {
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            '/' if at(i + 1) == Some('/') => {
                let end = chars[i..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(chars.len(), |p| i + p);
                out.push((line, chars[i + 2..end].iter().collect()));
                i = end;
            }
            '/' if at(i + 1) == Some('*') => {
                let mut depth = 1;
                let mut text = String::new();
                i += 2;
                while depth > 0 {
                    match (at(i), at(i + 1)) {
                        (None, _) => break,
                        (Some('/'), Some('*')) => {
                            depth += 1;
                            text.push_str("/*");
                            i += 2;
                        }
                        (Some('*'), Some('/')) => {
                            depth -= 1;
                            i += 2;
                        }
                        (Some('\n'), _) => {
                            out.push((line, std::mem::take(&mut text)));
                            line += 1;
                            i += 1;
                        }
                        (Some(c), _) => {
                            text.push(c);
                            i += 1;
                        }
                    }
                }
                out.push((line, text));
            }
            'r' if starts_raw_string(&chars, i) => {
                let hashes = chars[i + 1..].iter().take_while(|&&c| c == '#').count();
                let mut j = i + 2 + hashes;
                while let Some(c) = at(j) {
                    if c == '\n' {
                        line += 1;
                    }
                    if c == '"' && (1..=hashes).all(|k| at(j + k) == Some('#')) {
                        j += 1 + hashes;
                        break;
                    }
                    j += 1;
                }
                i = j;
            }
            '"' => {
                let mut j = i + 1;
                while let Some(c) = at(j) {
                    match c {
                        '\\' => {
                            if at(j + 1) == Some('\n') {
                                line += 1;
                            }
                            j += 2;
                        }
                        '"' => break,
                        _ => {
                            if c == '\n' {
                                line += 1;
                            }
                            j += 1;
                        }
                    }
                }
                i = j + 1;
            }
            '\'' => {
                i = if at(i + 1) == Some('\\') {
                    chars
                        .get(i + 3..)
                        .unwrap_or_default()
                        .iter()
                        .position(|&c| c == '\'')
                        .map_or(chars.len(), |p| i + 3 + p + 1)
                } else if at(i + 2) == Some('\'') {
                    i + 3
                } else {
                    i + 1
                };
            }
            _ => i += 1,
        }
    }
    out
}

/// Whether the `r` at `i` opens a raw string literal (`r"`, `r#"`, `br"`, `cr#"` and so on)
/// rather than ending an identifier or starting a raw identifier.
fn starts_raw_string(chars: &[char], i: usize) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let prefix_ok = match i.checked_sub(1).map(|p| chars[p]) {
        None => true,
        Some('b' | 'c') => i.checked_sub(2).is_none_or(|p| !is_ident(chars[p])),
        Some(c) => !is_ident(c),
    };
    let after_hashes = chars[i + 1..].iter().find(|&&c| c != '#');
    prefix_ok && after_hashes == Some(&'"')
}

/// Pushes every `.rs` file under `dir` into `out`, recursively.
fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The token class of the first offence in `source`, if any.
fn first_offence(source: &str) -> Option<&'static str> {
    offences(source).first().map(|o| o.token)
}

#[test]
fn the_lint_recognises_each_kind_of_banned_token() {
    let banned = [
        ("// Phase 1 ships this", "Phase N"),
        ("/// see Track 4.1", "Track N"),
        ("//! P4-A5 builtins", "plan item id"),
        ("    // per §3.5", "section sign"),
        ("// Widow's finding", "persona name"),
        ("// We drive 31 requests", "design-note voice"),
        ("/// the cache is ours", "design-note voice"),
        ("// let us check", "design-note voice"),
        (
            "call(); // but we'll execute with args_b",
            "design-note voice",
        ),
        ("/* our reasoning */", "design-note voice"),
        ("/*\n * as we found\n */", "design-note voice"),
        ("// the legacy path", "legacy"),
        ("// kept for a follow-up", "follow-up"),
        ("// left as a followup", "follow-up"),
        ("// previously returned 404", "previously"),
        ("// this used to panic", "used to"),
        ("// the default used to be 30", "used to"),
        ("// TODO(kuba): drop this", "task marker"),
        ("// FIXME", "task marker"),
        ("// added in PR 10", "pull request number"),
        ("// see PR #12", "pull request number"),
        ("// fixes #55", "issue reference"),
        ("// (see #208)", "issue reference"),
        ("// the FL2 exception", "finding label"),
        ("// see docs/pagination.md", "docs pointer"),
        ("/// See `docs/specs/key-identity-v1.md`", "docs pointer"),
        ("// per spec section 4", "spec section"),
        ("// the specification section on keys", "spec section"),
    ];
    for (source, token) in banned {
        assert_eq!(first_offence(source), Some(token), "in {source:?}");
    }

    let allowed = [
        "// The phase of the incident",
        "let phase = \"Phase 1\";",
        "// Track the latest cursor",
        "// Vectors from key-identity-v1 appendix B.",
        "// The verified content of appendix A.4, which names the A.3 attachment.",
        "/// Used to build the `/join/{token}` invite link.",
        "/// Returns a scope whose handle can be used to construct repositories.",
        "/// The en-US locale.",
        "// Business rules for the trust boundary.",
        "/// Rejects `#5` and `PR 3` inside a code span.",
        "/// PKCS#8 with the seed in the last 32 bytes.",
        "// Formats with {:#04x}.",
        "#[derive(Debug)]",
        "#![allow(dead_code)]",
        "let url = \"https://example.com/we\"; // a plain URL",
        "let s = \"// we do this\";",
        "let s = \"escaped \\\" // we\";",
        "let s = r#\"// we \"quoted\" here\"#;",
        "let c = '\"'; let d = '\\''; // fine",
        "fn f<'a>(x: &'a str) -> &'a str { x } // borrows",
        "/// ```\n/// let us = 1;\n/// ```",
        "/// The previous value, if any.",
        "/// Devices that no longer belong to the group.",
    ];
    for source in allowed {
        assert_eq!(first_offence(source), None, "in {source:?}");
    }

    assert_eq!(
        offences("let s = \"a \\\n b\";\n// previously")
            .first()
            .map(|o| o.line),
        Some(3),
        "a string continued with a backslash still counts its line"
    );
    assert_eq!(
        offences("fn f() {}\n\n    x(); // we said TODO"),
        vec![
            Offence {
                line: 3,
                token: "design-note voice",
                found: "we".into(),
            },
            Offence {
                line: 3,
                token: "task marker",
                found: "TODO".into(),
            },
        ]
    );
}
