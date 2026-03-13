use crate::keybind::dto::{
    DeviceLocalKeybindDto, EffectiveKeybindDto, KeybindLayersDto, KeybindRuntimeSourceDto,
    RuntimeKeybindLayersDto,
};

pub fn resolve_effective_keybind(
    backend_layers: KeybindLayersDto,
    device_local_keybind: Option<DeviceLocalKeybindDto>,
) -> RuntimeKeybindLayersDto {
    let resolved_effective_keybind = device_local_keybind
        .as_ref()
        .filter(|keybind| keybind.is_enabled)
        .map(|keybind| EffectiveKeybindDto {
            source: KeybindRuntimeSourceDto::DeviceLocal,
            keystroke: keybind.keystroke.clone(),
            command_id: keybind.command_id.clone(),
            context: keybind.context,
            preset_args: keybind.preset_args.clone(),
            is_enabled: true,
        })
        .or_else(|| {
            backend_layers
                .resolved_persisted_keybind
                .as_ref()
                .map(|keybind| EffectiveKeybindDto {
                    source: match keybind.source {
                        crate::keybind::dto::KeybindSourceDto::Workspace => {
                            KeybindRuntimeSourceDto::Workspace
                        }
                        crate::keybind::dto::KeybindSourceDto::User => {
                            KeybindRuntimeSourceDto::User
                        }
                        crate::keybind::dto::KeybindSourceDto::Default => {
                            KeybindRuntimeSourceDto::Default
                        }
                    },
                    keystroke: keybind.keystroke.clone(),
                    command_id: keybind.command_id.clone(),
                    context: keybind.context,
                    preset_args: keybind.preset_args.clone(),
                    is_enabled: keybind.is_enabled,
                })
        });

    RuntimeKeybindLayersDto {
        device_local_keybind,
        backend_layers,
        resolved_effective_keybind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybind::dto::{
        DefaultKeybindDto, KeybindContextDto, KeybindDto, KeybindScopeDto, KeybindSourceDto,
    };

    fn backend_layers() -> KeybindLayersDto {
        KeybindLayersDto {
            default_keybind: Some(DefaultKeybindDto {
                source: KeybindSourceDto::Default,
                keystroke: "Ctrl+R".to_string(),
                command_id: "workspace.cycle_recent_activity".to_string(),
                context: KeybindContextDto::Global,
                preset_args: None,
            }),
            user_keybind: Some(KeybindDto {
                id: rim_core::keybind::model::KeybindId::new(),
                scope: KeybindScopeDto::User,
                source: KeybindSourceDto::User,
                workspace_id: None,
                keystroke: "Ctrl+R".to_string(),
                command_id: "workspace.open_command_palette".to_string(),
                context: KeybindContextDto::Global,
                preset_args: None,
                is_enabled: true,
            }),
            workspace_keybind: Some(KeybindDto {
                id: rim_core::keybind::model::KeybindId::new(),
                scope: KeybindScopeDto::Workspace,
                source: KeybindSourceDto::Workspace,
                workspace_id: Some(rim_core::server::model::WorkspaceId::new()),
                keystroke: "Ctrl+R".to_string(),
                command_id: "chat.post_message".to_string(),
                context: KeybindContextDto::Global,
                preset_args: None,
                is_enabled: true,
            }),
            resolved_persisted_keybind: Some(KeybindDto {
                id: rim_core::keybind::model::KeybindId::new(),
                scope: KeybindScopeDto::Workspace,
                source: KeybindSourceDto::Workspace,
                workspace_id: Some(rim_core::server::model::WorkspaceId::new()),
                keystroke: "Ctrl+R".to_string(),
                command_id: "chat.post_message".to_string(),
                context: KeybindContextDto::Global,
                preset_args: None,
                is_enabled: true,
            }),
        }
    }

    #[test]
    fn device_local_overrides_backend_layers() {
        let resolved = resolve_effective_keybind(
            backend_layers(),
            Some(DeviceLocalKeybindDto {
                keystroke: "Ctrl+R".to_string(),
                command_id: "workspace.close_panel".to_string(),
                context: KeybindContextDto::Global,
                preset_args: None,
                is_enabled: true,
            }),
        );

        assert_eq!(
            resolved
                .resolved_effective_keybind
                .as_ref()
                .map(|keybind| keybind.source),
            Some(KeybindRuntimeSourceDto::DeviceLocal)
        );
    }

    #[test]
    fn backend_resolved_binding_is_used_when_no_device_local_exists() {
        let resolved = resolve_effective_keybind(backend_layers(), None);

        assert_eq!(
            resolved
                .resolved_effective_keybind
                .as_ref()
                .map(|keybind| keybind.source),
            Some(KeybindRuntimeSourceDto::Workspace)
        );
    }
}
