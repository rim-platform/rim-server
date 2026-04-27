use rim_services::command::dto::{CommandContextDto, CommandDefinitionDto, CommandSurfaceDto};
use rim_services::{Actor, ServiceResult};

use super::DefaultCommandService;

impl DefaultCommandService {
    /// Filters `commands` to only those that are `ai_callable` and for which
    /// the actor passes the capability check on every `required_capabilities`
    /// entry.
    pub(super) async fn filter_ai_surface(
        &self,
        actor: &Actor,
        context: Option<&CommandContextDto>,
        commands: Vec<CommandDefinitionDto>,
    ) -> ServiceResult<Vec<CommandDefinitionDto>> {
        let mut ai_commands: Vec<CommandDefinitionDto> =
            commands.into_iter().filter(|c| c.ai_callable).collect();
        if ai_commands.is_empty() {
            return Ok(ai_commands);
        }

        let cap_union: Vec<String> = ai_commands
            .iter()
            .flat_map(|c| c.required_capabilities.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        let workspace_id = context.and_then(|c| c.workspace_id);
        let cap_results = self
            .services
            .capability_service
            .check_many(actor, workspace_id, &cap_union)
            .await?;

        ai_commands.retain(|c| {
            c.required_capabilities
                .iter()
                .all(|cap| cap_results.get(cap).copied().unwrap_or(false))
        });
        Ok(ai_commands)
    }

    /// Test-only seam exposing [`Self::filter_ai_surface`] to integration tests.
    #[doc(hidden)]
    pub async fn filter_ai_surface_test_only(
        &self,
        actor: &Actor,
        context: Option<&CommandContextDto>,
        commands: Vec<CommandDefinitionDto>,
    ) -> ServiceResult<Vec<CommandDefinitionDto>> {
        self.filter_ai_surface(actor, context, commands).await
    }
}

pub(super) fn context_has_key(context: Option<&CommandContextDto>, key: &str) -> bool {
    let Some(context) = context else {
        return false;
    };

    match key {
        "workspace" => context.workspace_id.is_some(),
        "conversation" => context.conversation_id.is_some(),
        "thread" => context.thread_id.is_some(),
        "selected_text" => context
            .selected_text
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty()),
        "selected_message" => context.selected_message_id.is_some(),
        "selected_messages" => !context.selected_message_ids.is_empty(),
        "focused_surface" => context.focused_surface.is_some(),
        "active_panel" => context.active_panel.is_some(),
        _ => false,
    }
}

pub(super) fn command_matches_surface(
    definition: &CommandDefinitionDto,
    surface: CommandSurfaceDto,
    context: Option<&CommandContextDto>,
) -> bool {
    match surface {
        CommandSurfaceDto::Palette => definition.visible_in_palette,
        CommandSurfaceDto::ContextMenu => {
            definition.visible_in_context_menus
                && definition
                    .required_context
                    .iter()
                    .all(|key| context_has_key(context, key))
        }
        CommandSurfaceDto::KeybindPicker => definition.keybind_capable,
        // P2-A2: the Ai surface is filtered async in
        // `DefaultCommandService::filter_ai_surface` (needs
        // `CapabilityService::check_many`); this sync predicate is bypassed
        // for that variant. False is a safe default if a caller ever reaches here.
        CommandSurfaceDto::Ai => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::registry::builtin_commands;

    #[test]
    fn context_has_key_workspace() {
        let ctx = CommandContextDto {
            workspace_id: Some(rim_core::server::model::ServerId::new()),
            conversation_id: None,
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        };
        assert!(context_has_key(Some(&ctx), "workspace"));
        assert!(!context_has_key(Some(&ctx), "conversation"));
    }

    #[test]
    fn context_has_key_none_context_returns_false() {
        assert!(!context_has_key(None, "workspace"));
    }

    #[test]
    fn context_has_key_selected_text_empty_is_false() {
        let ctx = CommandContextDto {
            workspace_id: None,
            conversation_id: None,
            thread_id: None,
            selected_text: Some("   ".to_string()),
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        };
        assert!(!context_has_key(Some(&ctx), "selected_text"));
    }

    #[test]
    fn context_has_key_selected_text_non_empty_is_true() {
        let ctx = CommandContextDto {
            workspace_id: None,
            conversation_id: None,
            thread_id: None,
            selected_text: Some("hello".to_string()),
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        };
        assert!(context_has_key(Some(&ctx), "selected_text"));
    }

    #[test]
    fn context_has_key_selected_messages() {
        let ctx = CommandContextDto {
            workspace_id: None,
            conversation_id: None,
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![rim_core::message::model::MessageId::new()],
            focused_surface: None,
            active_panel: None,
        };
        assert!(context_has_key(Some(&ctx), "selected_messages"));
        assert!(!context_has_key(Some(&ctx), "selected_message"));
    }

    #[test]
    fn context_has_key_unknown_key_returns_false() {
        let ctx = CommandContextDto {
            workspace_id: Some(rim_core::server::model::ServerId::new()),
            conversation_id: None,
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        };
        assert!(!context_has_key(Some(&ctx), "unknown_key"));
    }

    #[test]
    fn command_matches_palette_requires_visible_in_palette() {
        let mut def = builtin_commands()
            .into_iter()
            .find(|c| c.visible_in_palette)
            .unwrap();
        assert!(command_matches_surface(
            &def,
            CommandSurfaceDto::Palette,
            None
        ));
        def.visible_in_palette = false;
        assert!(!command_matches_surface(
            &def,
            CommandSurfaceDto::Palette,
            None
        ));
    }

    #[test]
    fn command_matches_keybind_picker_requires_keybind_capable() {
        let mut def = builtin_commands()
            .into_iter()
            .find(|c| c.keybind_capable)
            .unwrap();
        assert!(command_matches_surface(
            &def,
            CommandSurfaceDto::KeybindPicker,
            None
        ));
        def.keybind_capable = false;
        assert!(!command_matches_surface(
            &def,
            CommandSurfaceDto::KeybindPicker,
            None
        ));
    }

    #[test]
    fn command_matches_context_menu_requires_context() {
        let def = builtin_commands()
            .into_iter()
            .find(|c| c.visible_in_context_menus && !c.required_context.is_empty())
            .unwrap();
        // Without context, should not match
        assert!(!command_matches_surface(
            &def,
            CommandSurfaceDto::ContextMenu,
            None
        ));
    }
}
