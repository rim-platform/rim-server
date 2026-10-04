use crate::command::dto::{CommandContextDto, CommandResultDto};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginHostHandshakeRequestDto {
    pub protocol_version: i32,
    pub plugin_id: String,
    pub host_version: String,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginHostHandshakeResponseDto {
    pub accepted: bool,
    pub protocol_version: i32,
    pub plugin_id: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginExecuteCommandRequestDto {
    pub protocol_version: i32,
    pub command_id: String,
    pub args: Option<serde_json::Value>,
    pub context: Option<CommandContextDto>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LocalPluginExecuteCommandResponseDto {
    pub result: CommandResultDto,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_command_request_keeps_context_payload() {
        let request = LocalPluginExecuteCommandRequestDto {
            protocol_version: 1,
            command_id: "plugin.dev.example.k8s-tools.restart_pod".to_string(),
            args: None,
            context: Some(CommandContextDto {
                workspace_id: None,
                conversation_id: None,
                thread_id: None,
                selected_text: Some("pod-1".to_string()),
                selected_message_id: None,
                selected_message_ids: vec![],
                focused_surface: Some("panel".to_string()),
                active_panel: Some("k8s".to_string()),
            }),
        };

        assert_eq!(
            request.command_id,
            "plugin.dev.example.k8s-tools.restart_pod"
        );
        assert_eq!(
            request
                .context
                .as_ref()
                .and_then(|ctx| ctx.active_panel.as_deref()),
            Some("k8s")
        );
    }
}
