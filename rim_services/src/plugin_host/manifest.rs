use crate::command::dto::{CommandArgumentDto, CommandDefinitionDto};
use crate::keybind::dto::KeybindContextDto;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LocalPluginRuntimeKindDto {
    DesktopLocalProcess,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginManifestCommandDto {
    pub id: String,
    pub title: String,
    pub description: String,
    pub keybind_capable: bool,
    pub dangerous: bool,
    pub required_context: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub arguments: Vec<CommandArgumentDto>,
}

impl LocalPluginManifestCommandDto {
    pub fn as_command_definition(&self, plugin_id: &str) -> CommandDefinitionDto {
        CommandDefinitionDto {
            id: format!("plugin.{plugin_id}.{}", self.id),
            title: self.title.clone(),
            description: self.description.clone(),
            category: "Plugin".to_string(),
            source_kind: crate::command::dto::CommandSourceKindDto::Plugin,
            execution_target: crate::command::dto::CommandExecutionTargetDto::DesktopLocal,
            arguments: self.arguments.clone(),
            required_context: self.required_context.clone(),
            required_capabilities: self.required_capabilities.clone(),
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: self.keybind_capable,
            dangerous: self.dangerous,
            user_command_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginKeybindSuggestionDto {
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContextDto,
    pub preset_args: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginContextActionDto {
    pub id: String,
    pub title: String,
    pub command_id: String,
    pub context_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginPanelDto {
    pub id: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginSettingSchemaDto {
    pub key: String,
    pub title: String,
    pub description: String,
    pub value_type: String,
    pub is_secret: bool,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginManifestDto {
    pub manifest_version: i32,
    pub plugin_id: String,
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub runtime_kind: LocalPluginRuntimeKindDto,
    pub entrypoint: String,
    pub capabilities: Vec<String>,
    pub commands: Vec<LocalPluginManifestCommandDto>,
    pub keybind_suggestions: Vec<LocalPluginKeybindSuggestionDto>,
    pub context_actions: Vec<LocalPluginContextActionDto>,
    pub panels: Vec<LocalPluginPanelDto>,
    pub settings_schema: Vec<LocalPluginSettingSchemaDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LocalPluginInstallStatusDto {
    Installed,
    Disabled,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginInstallStateDto {
    pub plugin_id: String,
    pub source_path: String,
    pub manifest: LocalPluginManifestDto,
    pub is_enabled: bool,
    pub install_status: LocalPluginInstallStatusDto,
    pub approved_capabilities: Vec<String>,
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_definition_is_namespaced_under_plugin() {
        let command = LocalPluginManifestCommandDto {
            id: "restart_pod".to_string(),
            title: "Restart Pod".to_string(),
            description: "desc".to_string(),
            keybind_capable: true,
            dangerous: true,
            required_context: vec![],
            required_capabilities: vec!["local.process".to_string()],
            arguments: vec![],
        };

        let definition = command.as_command_definition("dev.example.k8s-tools");
        assert_eq!(definition.id, "plugin.dev.example.k8s-tools.restart_pod");
    }
}
