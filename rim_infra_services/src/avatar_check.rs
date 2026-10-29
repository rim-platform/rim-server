//! The profile picture check: sends a picture to the image check service, which scores it per
//! category from 0 to 1, and turns the highest score into a verdict.

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;

use crate::outbound::read_capped;
use crate::s2s::net::http_client;

/// How long one check may take, connection included.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest answer read from the service.
const MAX_ANSWER_BYTES: usize = 16 * 1024;
/// The longest category name taken from an answer.
const MAX_CATEGORY_CHARS: usize = 32;

/// What becomes of a checked picture.
#[derive(Debug, Clone, PartialEq)]
pub enum AvatarVerdict {
    /// Every score is below the report threshold.
    Allow,
    /// The highest score is at or above the report threshold and below the block one: the
    /// picture is stored and reported.
    Report { category: String, score: f64 },
    /// The highest score is at or above the block threshold: the picture is refused.
    Block { category: String, score: f64 },
}

/// Why a picture could not be checked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AvatarCheckError {
    /// The service did not answer in time, answered another status than 200, or answered
    /// something that is not a set of scores.
    #[error("the image check is unavailable: {0}")]
    Unavailable(String),
}

/// The service's answer to a check.
#[derive(Deserialize)]
struct CheckAnswer {
    scores: BTreeMap<String, f64>,
}

/// The scores of the service's `response` to a check, shared by every check of the service.
///
/// # Errors
/// What went wrong when the status is not 200, the answer is over 16 KiB, or it holds no
/// scores, a score outside 0 to 1 or a category that is not a short lowercase name.
pub(crate) async fn read_scores(
    response: reqwest::Response,
) -> Result<BTreeMap<String, f64>, String> {
    if response.status() != StatusCode::OK {
        return Err(format!("answered {}", response.status()));
    }
    let body = read_capped(response, MAX_ANSWER_BYTES)
        .await
        .map_err(|e| e.to_string())?;
    let answer: CheckAnswer =
        serde_json::from_slice(&body).map_err(|e| format!("bad answer: {e}"))?;
    let valid = |category: &str, score: f64| {
        (0.0..=1.0).contains(&score)
            && !category.is_empty()
            && category.len() <= MAX_CATEGORY_CHARS
            && category
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_')
    };
    if answer.scores.is_empty() || answer.scores.iter().any(|(c, s)| !valid(c, *s)) {
        return Err("bad answer: no valid scores".to_string());
    }
    Ok(answer.scores)
}

/// A client of the image check service with the thresholds its scores are held against.
pub struct AvatarChecker {
    client: reqwest::Client,
    url: String,
    block: f64,
    report: f64,
}

impl AvatarChecker {
    /// A checker of the service at `base_url` that blocks at `block` and reports at `report`.
    /// Requests may reach private addresses, where the service runs, and time out after 5
    /// seconds.
    ///
    /// # Errors
    /// The message of a client that cannot be built.
    pub fn new(base_url: &str, block: f64, report: f64) -> Result<Self, String> {
        Self::with_timeout(base_url, block, report, CHECK_TIMEOUT)
    }

    fn with_timeout(
        base_url: &str,
        block: f64,
        report: f64,
        timeout: Duration,
    ) -> Result<Self, String> {
        let client = http_client(true, timeout).map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            url: format!("{}/v1/check", base_url.trim_end_matches('/')),
            block,
            report,
        })
    }

    /// The verdict on `png`, posted to `<base>/v1/check` as `image/png`.
    ///
    /// # Errors
    /// `Unavailable` when the request fails or times out, the status is not 200, the answer is
    /// over 16 KiB, or it holds no scores, a score outside 0 to 1 or a malformed category.
    pub async fn check(&self, png: &[u8]) -> Result<AvatarVerdict, AvatarCheckError> {
        let unavailable = AvatarCheckError::Unavailable;
        let response = self
            .client
            .post(&self.url)
            .header(CONTENT_TYPE, "image/png")
            .body(png.to_vec())
            .send()
            .await
            .map_err(|e| unavailable(e.to_string()))?;
        let scores = read_scores(response).await.map_err(unavailable)?;
        self.verdict(&scores)
            .ok_or_else(|| unavailable("bad answer: no valid scores".to_string()))
    }

    /// The verdict from the highest of `scores`; `None` when there is none.
    fn verdict(&self, scores: &BTreeMap<String, f64>) -> Option<AvatarVerdict> {
        let (category, score) =
            scores
                .iter()
                .fold(None, |top: Option<(&String, f64)>, (c, s)| match top {
                    Some((_, best)) if best >= *s => top,
                    _ => Some((c, *s)),
                })?;
        let category = category.clone();
        Some(if score >= self.block {
            AvatarVerdict::Block { category, score }
        } else if score >= self.report {
            AvatarVerdict::Report { category, score }
        } else {
            AvatarVerdict::Allow
        })
    }
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

    async fn answering(status: &str, body: &str, delay: Duration) -> AvatarChecker {
        let server = serve(
            head(status, body.len()),
            body.as_bytes().to_vec(),
            delay,
            Duration::ZERO,
        )
        .await;
        AvatarChecker::with_timeout(
            &format!("http://{}/", server.addr),
            0.85,
            0.60,
            Duration::from_millis(500),
        )
        .expect("client")
    }

    fn scores(sexual: f64, gore: f64, violence: f64) -> String {
        format!(
            r#"{{"scores": {{"sexual": {sexual}, "gore": {gore}, "violence": {violence}}}, "model": "m"}}"#
        )
    }

    #[tokio::test]
    async fn the_highest_score_decides() {
        let cases = [
            (scores(0.01, 0.02, 0.03), AvatarVerdict::Allow),
            (scores(0.59, 0.0, 0.0), AvatarVerdict::Allow),
            (
                scores(0.1, 0.6, 0.2),
                AvatarVerdict::Report {
                    category: "gore".to_string(),
                    score: 0.6,
                },
            ),
            (
                scores(0.7, 0.9, 0.85),
                AvatarVerdict::Block {
                    category: "gore".to_string(),
                    score: 0.9,
                },
            ),
            (
                scores(0.85, 0.0, 0.0),
                AvatarVerdict::Block {
                    category: "sexual".to_string(),
                    score: 0.85,
                },
            ),
        ];
        for (body, expected) in cases {
            let checker = answering("200 OK", &body, Duration::ZERO).await;
            assert_eq!(checker.check(b"png").await, Ok(expected), "{body}");
        }
    }

    #[tokio::test]
    async fn another_status_is_unavailable() {
        let checker = answering("503 Service Unavailable", "{}", Duration::ZERO).await;
        let err = checker.check(b"png").await.unwrap_err();
        assert!(err.to_string().contains("503"), "{err}");
    }

    #[tokio::test]
    async fn a_bad_answer_is_unavailable() {
        for body in [
            "not json",
            "{}",
            r#"{"scores": {}}"#,
            r#"{"scores": {"gore": 1.5}}"#,
            r#"{"scores": {"gore": -0.1}}"#,
            r#"{"scores": {"Gore!": 0.1}}"#,
            r#"{"scores": {"gore": "high"}}"#,
        ] {
            let checker = answering("200 OK", body, Duration::ZERO).await;
            assert!(
                matches!(
                    checker.check(b"png").await,
                    Err(AvatarCheckError::Unavailable(_))
                ),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn an_answer_over_16_kib_is_unavailable() {
        let body = format!(
            r#"{{"scores": {{"gore": 0.1}}, "pad": "{}"}}"#,
            "x".repeat(17_000)
        );
        let checker = answering("200 OK", &body, Duration::ZERO).await;
        let err = checker.check(b"png").await.unwrap_err();
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[tokio::test]
    async fn a_slow_answer_times_out() {
        let checker = answering("200 OK", &scores(0.0, 0.0, 0.0), Duration::from_secs(3)).await;
        assert!(matches!(
            checker.check(b"png").await,
            Err(AvatarCheckError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn a_closed_port_is_unavailable() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        let checker = AvatarChecker::new(&format!("http://{addr}"), 0.85, 0.60).expect("client");
        assert!(matches!(
            checker.check(b"png").await,
            Err(AvatarCheckError::Unavailable(_))
        ));
    }

    #[test]
    fn the_check_path_follows_the_base_url() {
        for base in ["http://imagecheck:8090", "http://imagecheck:8090/"] {
            let checker = AvatarChecker::new(base, 0.85, 0.60).expect("client");
            assert_eq!(checker.url, "http://imagecheck:8090/v1/check");
        }
    }
}
