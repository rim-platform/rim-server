use std::collections::HashMap;

use async_trait::async_trait;

use rim_core::message::model::MessageId;
use rim_core::user::model::UserId;
use rim_services::mention::service::MentionService;
use rim_services::{ServiceError, ServiceResult};

use crate::repos::MentionRepos;

/// Regex-free mention parser: extracts user IDs from `<@uuid>` tokens.
pub fn parse_mention_ids(text: &str) -> Vec<UserId> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let prefix = b"<@";
    let mut i = 0;
    while i + 2 < len {
        if bytes[i] == prefix[0] && bytes[i + 1] == prefix[1] {
            // Found "<@", look for closing ">"
            let start = i + 2;
            if let Some(end_offset) = text[start..].find('>') {
                let candidate = &text[start..start + end_offset];
                if let Ok(uuid) = uuid::Uuid::parse_str(candidate) {
                    let id = UserId::from(uuid);
                    if seen.insert(id) {
                        result.push(id);
                    }
                }
                i = start + end_offset + 1;
            } else {
                i += 2;
            }
        } else {
            i += 1;
        }
    }
    result
}

#[derive(Clone)]
pub struct DefaultMentionService {
    repos: MentionRepos,
}

impl DefaultMentionService {
    pub fn new(repos: MentionRepos) -> Self {
        Self { repos }
    }
}

#[async_trait]
impl MentionService for DefaultMentionService {
    async fn get_mentions_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> ServiceResult<HashMap<MessageId, Vec<UserId>>> {
        self.repos
            .message_mention_repo
            .get_mentions_for_messages(message_ids)
            .await
            .map_err(ServiceError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_mention() {
        let text = "hello <@550e8400-e29b-41d4-a716-446655440000> world";
        let ids = parse_mention_ids(text);
        assert_eq!(ids.len(), 1);
        assert_eq!(
            uuid::Uuid::from(ids[0]),
            uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );
    }

    #[test]
    fn parse_multiple_mentions() {
        let text =
            "<@550e8400-e29b-41d4-a716-446655440000> and <@660e8400-e29b-41d4-a716-446655440000>";
        let ids = parse_mention_ids(text);
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn parse_no_mentions() {
        let text = "no mentions here";
        let ids = parse_mention_ids(text);
        assert!(ids.is_empty());
    }

    #[test]
    fn parse_invalid_uuid_ignored() {
        let text = "<@not-a-uuid> and <@550e8400-e29b-41d4-a716-446655440000>";
        let ids = parse_mention_ids(text);
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn parse_unclosed_mention_ignored() {
        let text = "<@550e8400-e29b-41d4-a716-446655440000";
        let ids = parse_mention_ids(text);
        assert!(ids.is_empty());
    }

    #[test]
    fn parse_duplicate_mentions_deduped() {
        let text =
            "<@550e8400-e29b-41d4-a716-446655440000> and <@550e8400-e29b-41d4-a716-446655440000>";
        let ids = parse_mention_ids(text);
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn parse_empty_string() {
        assert!(parse_mention_ids("").is_empty());
    }

    #[test]
    fn parse_nested_angle_brackets_not_confused() {
        let text = "<<@550e8400-e29b-41d4-a716-446655440000>>";
        let ids = parse_mention_ids(text);
        // Should still find the UUID
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn parse_mention_adjacent_to_text() {
        let text = "hi<@550e8400-e29b-41d4-a716-446655440000>bye";
        let ids = parse_mention_ids(text);
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn parse_mention_preserves_order() {
        let text =
            "<@660e8400-e29b-41d4-a716-446655440000> then <@550e8400-e29b-41d4-a716-446655440000>";
        let ids = parse_mention_ids(text);
        assert_eq!(ids.len(), 2);
        assert_eq!(
            uuid::Uuid::from(ids[0]),
            uuid::Uuid::parse_str("660e8400-e29b-41d4-a716-446655440000").unwrap()
        );
        assert_eq!(
            uuid::Uuid::from(ids[1]),
            uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );
    }

    #[test]
    fn parse_mention_with_whitespace_in_uuid_ignored() {
        let text = "<@ 550e8400-e29b-41d4-a716-446655440000>";
        let ids = parse_mention_ids(text);
        assert!(ids.is_empty());
    }
}
