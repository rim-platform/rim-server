use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::conversation::model::ConversationId;
use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(PipelineId);

#[derive(Debug, Clone, PartialEq)]
pub struct Pipeline {
    pub id: PipelineId,
    pub server_id: ServerId,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTrigger,
    pub filter: Option<PipelineFilter>,
    pub actions: Vec<PipelineAction>,
    pub enabled: bool,
    pub cooldown_secs: Option<i32>,
    pub created_by: UserId,
    pub schema_version: i32,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineTrigger {
    pub kind: PipelineTriggerKind,
    pub conversation_id: Option<ConversationId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PipelineTriggerKind {
    NewMessage,
    MessageUpdated,
    MessageDeleted,
    ReactionsUpdated,
    MemberJoined,
    WorkspaceStateChanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineFilter {
    pub conditions: Vec<FilterCondition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FilterCondition {
    MessageContains { pattern: String },
    MessageFromUser { user_id: UserId },
    ConversationIs { conversation_id: ConversationId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PipelineAction {
    ExecuteCommand {
        command_id: String,
        args: Option<serde_json::Value>,
    },
    /// A message posted by a bot of the workspace in a channel it is in, when someone joins.
    /// `text` may hold the placeholders `{username}` and `{name}`.
    PostAsBot {
        bot_user_id: UserId,
        conversation_id: ConversationId,
        text: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_action_stored_before_post_as_bot_still_reads() {
        let stored = serde_json::json!([
            { "type": "ExecuteCommand", "command_id": "state.get", "args": null },
            { "type": "ExecuteCommand", "command_id": "state.list", "args": { "prefix": "" } },
        ]);
        let actions: Vec<PipelineAction> = serde_json::from_value(stored).expect("reads");
        assert_eq!(actions.len(), 2);
        assert!(matches!(actions[0], PipelineAction::ExecuteCommand { .. }));
    }

    #[test]
    fn a_post_as_bot_action_round_trips_under_its_own_tag() {
        let action = PipelineAction::PostAsBot {
            bot_user_id: UserId::new(),
            conversation_id: ConversationId::new(),
            text: "Hello {name}".to_string(),
        };
        let stored = serde_json::to_value(&action).expect("writes");
        assert_eq!(stored["type"], "PostAsBot");
        assert_eq!(
            serde_json::from_value::<PipelineAction>(stored).expect("reads"),
            action
        );
    }
}
