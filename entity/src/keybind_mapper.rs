use crate::keybind;
use rim_core::keybind::model::{Keybind, KeybindContext, KeybindScope, KeybindSubject};
use rim_core::server::model::WorkspaceId;
use rim_core::user::model::UserId;

impl From<keybind::Model> for Keybind {
    fn from(m: keybind::Model) -> Self {
        let scope = match m.scope.as_str() {
            "user" => KeybindScope::User,
            "workspace" => KeybindScope::Workspace,
            other => {
                // DB column is free-text; fall back to User if an unexpected value appears.
                tracing::warn!(
                    "warning: unknown keybind scope in database: {other:?}, defaulting to User"
                );
                KeybindScope::User
            }
        };
        let context = match m.context.as_str() {
            "global" => KeybindContext::Global,
            "composer" => KeybindContext::Composer,
            "message_list" => KeybindContext::MessageList,
            "thread_view" => KeybindContext::ThreadView,
            "conversation_view" => KeybindContext::ConversationView,
            "panel" => KeybindContext::Panel,
            "command_palette" => KeybindContext::CommandPalette,
            other => {
                // DB column is free-text; fall back to Global if an unexpected value appears.
                tracing::warn!(
                    "warning: unknown keybind context in database: {other:?}, defaulting to Global"
                );
                KeybindContext::Global
            }
        };
        let subject = match scope {
            KeybindScope::User => KeybindSubject::User(UserId::from(m.subject_id)),
            KeybindScope::Workspace => KeybindSubject::Workspace(WorkspaceId::from(m.subject_id)),
        };

        Self {
            id: m.id.into(),
            scope,
            subject,
            keystroke: m.keystroke,
            command_id: m.command_id,
            context,
            preset_args: m.preset_args,
            is_enabled: m.is_enabled,
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
