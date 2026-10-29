//! The text art check's client: tells whether a message looks like text art, and has the image
//! check service score how sexual the drawn text looks, from 0 to 1.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;

use crate::avatar_check::read_scores;
use crate::s2s::net::http_client;

/// How long one check may take, connection included.
const TIMEOUT: Duration = Duration::from_secs(10);
/// The fewest non-whitespace characters a text art line has.
const MIN_LINE_CHARS: usize = 8;
/// The fewest non-blank lines a text art block has.
const MIN_LINES: usize = 6;
/// The fewest non-whitespace characters a text art block has.
const MIN_CHARS: usize = 120;
/// The category of the service's answer the check reads.
const SEXUAL: &str = "sexual";

/// Why a text could not be scored.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextArtCheckError {
    /// The service did not answer in time, answered another status than 200, or answered
    /// something that is not a set of scores with a sexual one.
    #[error("the text art check is unavailable: {0}")]
    Unavailable(String),
}

/// Something that scores how sexual the drawing in a text looks, from 0 to 1.
#[async_trait]
pub trait TextScorer: Send + Sync {
    /// The sexual score of `text`.
    ///
    /// # Errors
    /// `Unavailable` when the text cannot be scored.
    async fn score(&self, text: &str) -> Result<f64, TextArtCheckError>;
}

/// A client of the image check service's text check.
pub struct TextArtChecker {
    client: reqwest::Client,
    url: String,
}

impl TextArtChecker {
    /// A checker of the service at `base_url`. Requests may reach private addresses, where the
    /// service runs, and time out after 10 seconds.
    ///
    /// # Errors
    /// The message of a client that cannot be built.
    pub fn new(base_url: &str) -> Result<Self, String> {
        Self::with_timeout(base_url, TIMEOUT)
    }

    fn with_timeout(base_url: &str, timeout: Duration) -> Result<Self, String> {
        let client = http_client(true, timeout).map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            url: format!("{}/v1/check-text", base_url.trim_end_matches('/')),
        })
    }
}

#[async_trait]
impl TextScorer for TextArtChecker {
    /// The sexual score of `text`, posted to `<base>/v1/check-text` as UTF-8 plain text.
    ///
    /// # Errors
    /// `Unavailable` when the request fails or times out, the status is not 200, the answer is
    /// over 16 KiB, or it holds no valid scores or no sexual score.
    async fn score(&self, text: &str) -> Result<f64, TextArtCheckError> {
        let unavailable = TextArtCheckError::Unavailable;
        let response = self
            .client
            .post(&self.url)
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(text.to_owned())
            .send()
            .await
            .map_err(|e| unavailable(e.to_string()))?;
        let scores = read_scores(response).await.map_err(unavailable)?;
        scores
            .get(SEXUAL)
            .copied()
            .ok_or_else(|| unavailable("bad answer: no sexual score".to_string()))
    }
}

/// The longest run of text art lines in `text`, or `None` when the message holds no picture big
/// enough to check. A line is text art when it has at least 8 non-whitespace characters and
/// fewer than half of them are letters or digits; braille, block and box-drawing characters are
/// neither, while code, logs and prose are mostly letters. Blank lines stay inside a run, any
/// other line ends it. The run needs at least 6 non-blank lines and 120 non-whitespace
/// characters; the first of equally long runs wins.
pub fn art_block(text: &str) -> Option<String> {
    let mut best: Vec<&str> = Vec::new();
    let mut run: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !run.is_empty() {
                run.push(line);
            }
        } else if is_art_line(line) {
            run.push(line);
        } else {
            keep_longest(&mut best, &mut run);
        }
    }
    keep_longest(&mut best, &mut run);
    let lines = non_blank(&best);
    let chars = best
        .iter()
        .flat_map(|l| l.chars())
        .filter(|c| !c.is_whitespace())
        .count();
    (lines >= MIN_LINES && chars >= MIN_CHARS).then(|| best.join("\n"))
}

/// Whether `line` is a text art line.
fn is_art_line(line: &str) -> bool {
    let (chars, alphanumeric) = line
        .chars()
        .filter(|c| !c.is_whitespace())
        .fold((0usize, 0usize), |(n, a), c| {
            (n + 1, a + usize::from(c.is_alphanumeric()))
        });
    chars >= MIN_LINE_CHARS && alphanumeric * 2 < chars
}

/// Ends `run`, without its trailing blank lines: it replaces `best` when it has more lines.
fn keep_longest<'a>(best: &mut Vec<&'a str>, run: &mut Vec<&'a str>) {
    while run.last().is_some_and(|l| l.trim().is_empty()) {
        run.pop();
    }
    if run.len() > best.len() {
        *best = std::mem::take(run);
    }
    run.clear();
}

fn non_blank(lines: &[&str]) -> usize {
    lines.iter().filter(|l| !l.trim().is_empty()).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbound::fixture::serve;

    /// An HTTP/1.1 head with `status` and a JSON body of `len` bytes.
    fn head(status: &str, len: usize) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
        )
    }

    async fn answering(status: &str, body: &str) -> TextArtChecker {
        let server = serve(
            head(status, body.len()),
            body.as_bytes().to_vec(),
            Duration::ZERO,
            Duration::ZERO,
        )
        .await;
        TextArtChecker::with_timeout(
            &format!("http://{}/", server.addr),
            Duration::from_millis(500),
        )
        .expect("client")
    }

    fn braille(lines: usize) -> String {
        (0..lines)
            .map(|i| {
                if i % 2 == 0 {
                    "⠀⠀⣀⣤⣶⣿⣿⣿⣿⣶⣤⣀⠀⠀⢀⣴⣿⣿⣿⡿⠿⠿⢿⣿⣿⣷⣄⠀"
                } else {
                    "⢠⣿⣿⡿⠋⠁⠀⠀⠈⠙⢿⣿⣿⡄⣾⣿⠟⠁⠀⠀⠀⠀⠈⠻⣿⣿⣷⠀"
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_braille_drawing_is_its_own_block() {
        assert_eq!(art_block(&braille(40)), Some(braille(40)));
    }

    #[test]
    fn an_ascii_figure_is_a_block() {
        let figure = r"
            /\_/\    /\_/\    /\_/\
           ( o.o )  ( -.- )  ( ^.^ )
            > ^ <    > ^ <    > ^ <
           /|   |\  /|   |\  /|   |\
          (_|   |_)(_|   |_)(_|   |_)
            |___|    |___|    |___|
           /\   /\  /\   /\  /\   /\
          /__\_/__\/__\_/__\/__\_/__\
          \__/ \__/\__/ \__/\__/ \__/
           ||   ||  ||   ||  ||   ||
        ";
        assert!(art_block(figure).is_some_and(|b| b.lines().count() == 10));
    }

    #[test]
    fn code_is_not_text_art() {
        let code = r#"
            fn parse_settings(input: &str) -> Result<Settings, String> {
                let mut settings = Settings::default();
                for line in input.lines() {
                    let (key, value) = line.split_once('=').ok_or("missing equals sign")?;
                    match key.trim() {
                        "timeout" => settings.timeout = value.trim().parse().map_err(|e| format!("{e}"))?,
                        "retries" => settings.retries = value.trim().parse().map_err(|e| format!("{e}"))?,
                        "verbose" => settings.verbose = value.trim() == "true",
                        other => return Err(format!("unknown setting {other}")),
                    }
                }
                Ok(settings)
            }
        "#;
        assert!(code.lines().filter(|l| !l.trim().is_empty()).count() >= 12);
        assert!(art_block(code).is_none());
    }

    #[test]
    fn log_lines_are_not_text_art() {
        let logs = (0..30)
            .map(|i| {
                format!(
                    "2026-10-29T08:{i:02}:13.512Z INFO rim_server::app: request served status=200 path=/api/graphql ms={i}"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(art_block(&logs).is_none());
    }

    #[test]
    fn prose_is_not_text_art() {
        let prose = "The meeting moved to Thursday because half of the team is travelling this \
            week. Please read the notes from last time before you come, and bring the numbers \
            for the third quarter if you have them. We will go through the budget first, then \
            the hiring plan, and finish with questions. Coffee is on the second floor."
            .repeat(3);
        assert!(art_block(&prose).is_none());
        let paragraphs = [prose.as_str(); 6].join("\n\n");
        assert!(art_block(&paragraphs).is_none());
    }

    #[test]
    fn a_short_braille_snippet_is_not_text_art() {
        assert!(art_block(&braille(3)).is_none());
    }

    const FIGURE: &str = r"/\_/\    /\_/\    /\_/\
( o.o )  ( -.- )  ( ^.^ )
 > ^ <    > ^ <    > ^ <
/|   |\  /|   |\  /|   |\
(_|   |_)(_|   |_)(_|   |_)
  |___|    |___|    |___|
 /\   /\  /\   /\  /\   /\
/__\_/__\/__\_/__\/__\_/__\
\__/ \__/\__/ \__/\__/ \__/
 ||   ||  ||   ||  ||   ||";

    fn paragraph() -> String {
        "The meeting moved to Thursday because half of the team is travelling this week, so \
         please read the notes from last time before you come and bring the numbers for the \
         third quarter if you have them. We will go through the budget first, then the hiring \
         plan, and finish with questions. Coffee is on the second floor of the main building, \
         next to the printers, and there is tea in the kitchen as well as plenty of biscuits \
         for everybody who arrives early enough to get one before they are gone."
            .repeat(2)
    }

    #[test]
    fn an_intro_line_is_left_out_of_the_block() {
        let text = format!("hey here is my new picture:\n{}", braille(40));
        assert_eq!(art_block(&text), Some(braille(40)));
    }

    #[test]
    fn a_figure_next_to_a_paragraph_is_found() {
        let paragraph = paragraph();
        assert!(paragraph.len() >= 600);
        let before = format!("{paragraph}\n{FIGURE}");
        assert_eq!(art_block(&before).as_deref(), Some(FIGURE));
        let after = format!("{FIGURE}\n{paragraph}");
        assert_eq!(art_block(&after).as_deref(), Some(FIGURE));
    }

    #[test]
    fn the_longer_of_two_blocks_wins() {
        let text = format!("{}\nand another one\n{}", braille(7), braille(9));
        assert_eq!(art_block(&text), Some(braille(9)));
        let text = format!("{}\nand another one\n{}", braille(9), braille(7));
        assert_eq!(art_block(&text), Some(braille(9)));
    }

    #[test]
    fn blank_lines_stay_inside_a_block_and_trailing_ones_go() {
        let drawing = braille(8);
        let lines: Vec<&str> = drawing.lines().collect();
        let text = format!(
            "{}\n\n  \n{}\n\n\n",
            lines[..4].join("\n"),
            lines[4..].join("\n")
        );
        let expected = format!("{}\n\n  \n{}", lines[..4].join("\n"), lines[4..].join("\n"));
        assert_eq!(art_block(&text), Some(expected));
    }

    #[test]
    fn plain_chat_has_no_block() {
        assert_eq!(art_block("hey whats up?"), None);
    }

    #[tokio::test]
    async fn the_sexual_score_is_read() {
        let checker = answering("200 OK", r#"{"scores": {"sexual": 0.72}, "model": "m"}"#).await;
        assert_eq!(checker.score("art").await, Ok(0.72));
    }

    #[tokio::test]
    async fn a_score_out_of_range_is_unavailable() {
        for body in [
            r#"{"scores": {"sexual": 1.2}}"#,
            r#"{"scores": {"sexual": -0.1}}"#,
        ] {
            let checker = answering("200 OK", body).await;
            assert!(
                matches!(
                    checker.score("art").await,
                    Err(TextArtCheckError::Unavailable(_))
                ),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn another_status_is_unavailable() {
        let checker = answering("503 Service Unavailable", r#"{"error": "busy"}"#).await;
        let err = checker.score("art").await.unwrap_err();
        assert!(err.to_string().contains("503"), "{err}");
    }

    #[tokio::test]
    async fn a_missing_sexual_score_is_unavailable() {
        for body in ["{}", r#"{"scores": {}}"#, r#"{"scores": {"gore": 0.1}}"#] {
            let checker = answering("200 OK", body).await;
            assert!(
                matches!(
                    checker.score("art").await,
                    Err(TextArtCheckError::Unavailable(_))
                ),
                "{body}"
            );
        }
    }

    #[test]
    fn the_check_path_follows_the_base_url() {
        for base in ["http://imagecheck:8090", "http://imagecheck:8090/"] {
            let checker = TextArtChecker::new(base).expect("client");
            assert_eq!(checker.url, "http://imagecheck:8090/v1/check-text");
        }
    }
}
