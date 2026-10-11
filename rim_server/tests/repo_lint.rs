//! Comment hygiene across the workspace: no comment line in any crate's `src` or `tests` names a
//! planning phase, a track, a plan item id, a section sign or a persona name. Comments say
//! what the code is and does. Reads source files only, so it needs no database.

use std::fs;
use std::path::{Path, PathBuf};

/// Literal substrings a comment line must not contain.
const BANNED_WORDS: &[&str] = &[
    "§",
    "Fury",
    "Widow",
    "Hulk",
    "Iron Man",
    "Iron-Man",
    "IronMan",
    "Hawkeye",
    "Strange",
    "Falcon",
    "Tony",
    "Spider-Man",
    "Ant-Man",
];

/// Words that must not be followed by a space and a digit.
const BANNED_NUMBERED: &[&str] = &["Phase", "Track"];

#[test]
fn no_comment_names_a_plan_phase_or_persona() {
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
    assert!(
        !files.is_empty(),
        "no Rust sources found under {}",
        root.display()
    );

    let mut offences = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).expect("read Rust source");
        for (index, line) in text.lines().enumerate() {
            if let Some(word) = comment_offence(line) {
                let shown = file.strip_prefix(root).unwrap_or(file).display();
                offences.push(format!(
                    "{shown}:{}: `{word}` in: {}",
                    index + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        offences.is_empty(),
        "comments must say what the code does, without plan phases, tracks, plan ids, \
         section signs or persona names:\n{}",
        offences.join("\n")
    );
}

/// The comment text of `line` when the line is a `//`, `///` or `//!` comment.
fn comment_text(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix("//")
}

/// The first banned token in `line`'s comment, if any.
fn comment_offence(line: &str) -> Option<String> {
    let comment = comment_text(line)?;
    if let Some(word) = BANNED_WORDS.iter().find(|w| comment.contains(*w)) {
        return Some((*word).to_string());
    }
    for word in BANNED_NUMBERED {
        for (at, _) in comment.match_indices(word) {
            let rest = &comment[at + word.len()..];
            if rest.starts_with(' ') && rest[1..].starts_with(|c: char| c.is_ascii_digit()) {
                return Some(format!("{word} N"));
            }
        }
    }
    plan_item_id(comment)
}

/// A plan item id: `P`, a digit, `-`, an uppercase letter, a digit.
fn plan_item_id(comment: &str) -> Option<String> {
    let bytes = comment.as_bytes();
    bytes.windows(5).find_map(|w| {
        let is_id = w[0] == b'P'
            && w[1].is_ascii_digit()
            && w[2] == b'-'
            && w[3].is_ascii_uppercase()
            && w[4].is_ascii_digit();
        is_id.then(|| String::from_utf8_lossy(w).into_owned())
    })
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

#[test]
fn the_lint_recognises_each_kind_of_banned_token() {
    assert_eq!(
        comment_offence("// Phase 1 ships this"),
        Some("Phase N".into())
    );
    assert_eq!(comment_offence("/// see Track 4.1"), Some("Track N".into()));
    assert_eq!(comment_offence("//! P4-A5 builtins"), Some("P4-A5".into()));
    assert_eq!(comment_offence("    // per §3.5"), Some("§".into()));
    assert_eq!(comment_offence("// Widow's finding"), Some("Widow".into()));
    assert_eq!(comment_offence("// The phase of the incident"), None);
    assert_eq!(comment_offence("let phase = \"Phase 1\";"), None);
    assert_eq!(comment_offence("// Track the latest cursor"), None);
}
