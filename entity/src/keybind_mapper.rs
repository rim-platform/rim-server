use std::str::FromStr;

use crate::keybind;
use rim_core::keybind::model::{Keybind, KeybindContext, KeybindScope, KeybindSubject};
use rim_core::server::model::WorkspaceId;
use rim_core::user::model::UserId;

impl From<keybind::Model> for Keybind {
    fn from(m: keybind::Model) -> Self {
        let scope = KeybindScope::from_str(&m.scope).expect("invalid keybind scope in DB");
        let context = KeybindContext::from_str(&m.context).expect("invalid keybind context in DB");
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
