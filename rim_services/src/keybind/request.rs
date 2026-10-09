use crate::Actor;
use crate::keybind::dto::{KeybindContextDto, KeybindScopeDto};

#[derive(Debug, Clone)]
pub struct ListKeybindsQuery {
    pub actor: Actor,
    pub scope: KeybindScopeDto,
    pub workspace_id: Option<rim_core::server::model::WorkspaceId>,
}

#[derive(Debug, Clone)]
pub struct ListDefaultKeybindsQuery {
    pub actor: Actor,
}

#[derive(Debug, Clone)]
pub struct ResolveKeybindsQuery {
    pub actor: Actor,
    pub workspace_id: Option<rim_core::server::model::WorkspaceId>,
}

#[derive(Debug, Clone)]
pub struct ResolveKeybindQuery {
    pub actor: Actor,
    pub workspace_id: Option<rim_core::server::model::WorkspaceId>,
    pub keystroke: String,
    pub context: KeybindContextDto,
}

#[derive(Debug, Clone)]
pub struct InspectKeybindLayersQuery {
    pub actor: Actor,
    pub workspace_id: Option<rim_core::server::model::WorkspaceId>,
    pub keystroke: String,
    pub context: KeybindContextDto,
}

#[derive(Debug, Clone)]
pub struct CreateKeybindCmd {
    pub actor: Actor,
    pub scope: KeybindScopeDto,
    pub workspace_id: Option<rim_core::server::model::WorkspaceId>,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContextDto,
    pub preset_args: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct UpdateKeybindCmd {
    pub actor: Actor,
    pub keybind_id: rim_core::keybind::model::KeybindId,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContextDto,
    pub preset_args: Option<serde_json::Value>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct DeleteKeybindCmd {
    pub actor: Actor,
    pub keybind_id: rim_core::keybind::model::KeybindId,
}
