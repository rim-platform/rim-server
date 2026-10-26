//! The definition file of the default workspace: its name, DM policy and channels, read from
//! JSON and checked against the rules the API applies to the same values.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use rim_core::conversation::model::PostingPolicy;
use rim_core::dm_policy::model::{MemberDm, OutsideFloor};

/// Longest workspace name, in characters, as `createServer` takes it.
const MAX_NAME_CHARS: usize = 128;
/// Longest channel title, in characters, as `createConversation` takes it.
const MAX_TITLE_CHARS: usize = 100;
/// Longest channel topic, in characters, as `updateConversationTopic` takes it.
const MAX_TOPIC_CHARS: usize = 512;

/// The workspace a definition file describes, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDefinition {
    /// The workspace's name, trimmed.
    pub name: String,
    /// Whether the account that claims the workspace also becomes an instance operator.
    pub claim_grants_operator: bool,
    /// The workspace's DM policy; `None` keeps the policy a new workspace gets.
    pub dm: Option<DmDefinition>,
    /// The channels, in the order the file lists them.
    pub channels: Vec<ChannelDefinition>,
}

/// The DM policy a definition sets, as `setWorkspaceDmPolicy` takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmDefinition {
    pub outside_floor: OutsideFloor,
    pub member_default: MemberDm,
}

/// One channel of a definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelDefinition {
    /// The channel's title, trimmed; unique within the definition.
    pub title: String,
    /// The topic, trimmed and without control characters other than newlines; `None` leaves
    /// the channel's topic as it is.
    pub topic: Option<String>,
    /// Whether the channel is private; fixed when the channel is created.
    pub private: bool,
    /// Who may post in the channel and, under `PostingPolicy::Moderators`, whether everyone
    /// else may still react and reply in threads; `None` leaves the channel's setting as it is.
    pub posting: Option<(PostingPolicy, bool)>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawDefinition {
    name: String,
    #[serde(default)]
    claim_grants_operator: bool,
    #[serde(default)]
    dm: Option<RawDm>,
    #[serde(default)]
    channels: Vec<RawChannel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawDm {
    outside_floor: String,
    member_default: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChannel {
    name: String,
    #[serde(default)]
    topic: Option<String>,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    posting: Option<String>,
    #[serde(default, rename = "postingAllowsThreads")]
    posting_allows_threads: Option<bool>,
}

impl WorkspaceDefinition {
    /// Read and check the definition file at `path`.
    ///
    /// # Errors
    /// A message naming the file when it cannot be read, else as [`WorkspaceDefinition::parse`].
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    /// Parse and check a definition from its JSON `text`.
    ///
    /// # Errors
    /// A message naming the field that is missing, unknown or out of range, such as
    /// `channels[1].topic: at most 512 characters`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: RawDefinition = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let name = raw.name.trim();
        if name.is_empty() {
            return Err("name: must not be empty".to_string());
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(format!("name: at most {MAX_NAME_CHARS} characters"));
        }
        let dm = raw.dm.map(dm_definition).transpose()?;
        let mut titles = HashSet::new();
        let channels = raw
            .channels
            .into_iter()
            .enumerate()
            .map(|(index, channel)| {
                let channel = channel_definition(index, channel)?;
                if !titles.insert(channel.title.clone()) {
                    return Err(format!(
                        "channels[{index}].name: {} is listed twice",
                        channel.title
                    ));
                }
                Ok(channel)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            name: name.to_string(),
            claim_grants_operator: raw.claim_grants_operator,
            dm,
            channels,
        })
    }
}

fn dm_definition(raw: RawDm) -> Result<DmDefinition, String> {
    let outside_floor = match raw.outside_floor.as_str() {
        "NONE" => OutsideFloor::None,
        "REQUESTS" => OutsideFloor::Requests,
        "CLOSED" => OutsideFloor::Closed,
        other => {
            return Err(format!(
                "dm.outsideFloor: {other} is not NONE, REQUESTS or CLOSED"
            ));
        }
    };
    let member_default = match raw.member_default.as_str() {
        "DIRECT" => MemberDm::Direct,
        "REQUESTS" => MemberDm::Requests,
        other => {
            return Err(format!(
                "dm.memberDefault: {other} is not DIRECT or REQUESTS"
            ));
        }
    };
    Ok(DmDefinition {
        outside_floor,
        member_default,
    })
}

fn channel_definition(index: usize, raw: RawChannel) -> Result<ChannelDefinition, String> {
    let title = raw.name.trim();
    if title.is_empty() {
        return Err(format!("channels[{index}].name: must not be empty"));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!(
            "channels[{index}].name: at most {MAX_TITLE_CHARS} characters"
        ));
    }
    let topic = raw
        .topic
        .as_deref()
        .map(clean_topic)
        .filter(|t| !t.is_empty());
    if topic
        .as_ref()
        .is_some_and(|t| t.chars().count() > MAX_TOPIC_CHARS)
    {
        return Err(format!(
            "channels[{index}].topic: at most {MAX_TOPIC_CHARS} characters"
        ));
    }
    let posting = match (raw.posting.as_deref(), raw.posting_allows_threads) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(format!(
                "channels[{index}].postingAllowsThreads: needs posting"
            ));
        }
        (Some(policy), allows_threads) => {
            let policy = policy.parse::<PostingPolicy>().map_err(|_| {
                format!(r#"channels[{index}].posting: must be "everyone" or "moderators""#)
            })?;
            Some((policy, allows_threads.unwrap_or(true)))
        }
    };
    Ok(ChannelDefinition {
        title: title.to_string(),
        topic,
        private: raw.private,
        posting,
    })
}

/// A topic as `updateConversationTopic` stores it: control characters other than newlines
/// removed, then trimmed.
fn clean_topic(topic: &str) -> String {
    topic
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = include_str!("../../../deploy/default-workspace.json");

    #[test]
    fn the_committed_file_is_a_valid_definition() {
        let definition = WorkspaceDefinition::parse(PROJECT).expect("the committed definition");
        assert_eq!(definition.name, "RIM");
        assert!(definition.claim_grants_operator);
        assert_eq!(
            definition.dm,
            Some(DmDefinition {
                outside_floor: OutsideFloor::Requests,
                member_default: MemberDm::Requests,
            })
        );
        let titles: Vec<&str> = definition
            .channels
            .iter()
            .map(|c| c.title.as_str())
            .collect();
        assert_eq!(titles, ["general", "announcements", "help", "plugins"]);
        assert!(definition.channels.iter().all(|c| !c.private));
        assert_eq!(
            definition.channels[0].topic.as_deref(),
            Some("Anything about RIM")
        );
        let posting: Vec<_> = definition.channels.iter().map(|c| c.posting).collect();
        assert_eq!(
            posting,
            [None, Some((PostingPolicy::Moderators, true)), None, None]
        );
    }

    #[test]
    fn a_name_alone_is_enough_and_the_rest_defaults() {
        let definition = WorkspaceDefinition::parse(r#"{ "name": "  Lobby " }"#).expect("parse");
        assert_eq!(
            definition,
            WorkspaceDefinition {
                name: "Lobby".to_string(),
                claim_grants_operator: false,
                dm: None,
                channels: vec![],
            }
        );
    }

    #[test]
    fn a_channel_keeps_its_privacy_and_a_cleaned_topic() {
        let definition = WorkspaceDefinition::parse(
            r#"{ "name": "Co", "channels": [
                { "name": " staff ", "topic": " Only\u0007 us\n ", "private": true },
                { "name": "blank", "topic": "  " }
            ] }"#,
        )
        .expect("parse");
        assert_eq!(
            definition.channels,
            vec![
                ChannelDefinition {
                    title: "staff".to_string(),
                    topic: Some("Only us".to_string()),
                    private: true,
                    posting: None,
                },
                ChannelDefinition {
                    title: "blank".to_string(),
                    topic: None,
                    private: false,
                    posting: None,
                },
            ]
        );
    }

    #[test]
    fn a_channel_takes_a_posting_policy_with_threads_open_by_default() {
        let definition = WorkspaceDefinition::parse(
            r#"{ "name": "Co", "channels": [
                { "name": "news", "posting": "moderators" },
                { "name": "board", "posting": "moderators", "postingAllowsThreads": false },
                { "name": "chat", "posting": "everyone" },
                { "name": "misc" }
            ] }"#,
        )
        .expect("parse");
        let posting: Vec<_> = definition.channels.iter().map(|c| c.posting).collect();
        assert_eq!(
            posting,
            [
                Some((PostingPolicy::Moderators, true)),
                Some((PostingPolicy::Moderators, false)),
                Some((PostingPolicy::Everyone, true)),
                None,
            ]
        );
    }

    fn error(text: &str) -> String {
        WorkspaceDefinition::parse(text).expect_err("refused")
    }

    #[test]
    fn every_refusal_names_its_field() {
        let long = "x".repeat(129);
        let cases = [
            (r#"{ "name": " " }"#.to_string(), "name: must not be empty"),
            (format!(r#"{{ "name": "{long}" }}"#), "name: at most 128"),
            (r#"{ "channels": [] }"#.to_string(), "missing field `name`"),
            (r#"{ "name": "a", "owner": "kuba" }"#.to_string(), "unknown field `owner`"),
            (
                r#"{ "name": "a", "dm": { "outsideFloor": "REQUEST", "memberDefault": "DIRECT" } }"#
                    .to_string(),
                "dm.outsideFloor: REQUEST is not",
            ),
            (
                r#"{ "name": "a", "dm": { "outsideFloor": "NONE", "memberDefault": "direct" } }"#
                    .to_string(),
                "dm.memberDefault: direct is not",
            ),
            (
                r#"{ "name": "a", "dm": { "outsideFloor": "NONE" } }"#.to_string(),
                "missing field `memberDefault`",
            ),
            (
                r#"{ "name": "a", "channels": [{ "name": "ok" }, { "name": "" }] }"#.to_string(),
                "channels[1].name: must not be empty",
            ),
            (
                format!(r#"{{ "name": "a", "channels": [{{ "name": "{}" }}] }}"#, "c".repeat(101)),
                "channels[0].name: at most 100",
            ),
            (
                format!(
                    r#"{{ "name": "a", "channels": [{{ "name": "c", "topic": "{}" }}] }}"#,
                    "t".repeat(513)
                ),
                "channels[0].topic: at most 512",
            ),
            (
                r#"{ "name": "a", "channels": [{ "name": "c" }, { "name": " c" }] }"#.to_string(),
                "channels[1].name: c is listed twice",
            ),
            (
                r#"{ "name": "a", "channels": [{ "name": "c", "title": "c" }] }"#.to_string(),
                "unknown field `title`",
            ),
            (
                r#"{ "name": "a", "channels": [{ "name": "c" }, { "name": "d", "posting": "staff" }] }"#
                    .to_string(),
                r#"channels[1].posting: must be "everyone" or "moderators""#,
            ),
            (
                r#"{ "name": "a", "channels": [{ "name": "c", "postingAllowsThreads": false }] }"#
                    .to_string(),
                "channels[0].postingAllowsThreads: needs posting",
            ),
            ("not json".to_string(), "expected"),
        ];
        for (text, expected) in cases {
            let err = error(&text);
            assert!(err.contains(expected), "{text}: {err}");
        }
    }

    #[test]
    fn load_names_a_file_it_cannot_read() {
        let err = WorkspaceDefinition::load(Path::new("/nonexistent/default-workspace.json"))
            .expect_err("missing file");
        assert!(
            err.starts_with("cannot read /nonexistent/default-workspace.json"),
            "{err}"
        );
    }
}
