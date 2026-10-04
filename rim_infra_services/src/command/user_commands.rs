use serde_json::Value;

use rim_core::command::model::UserCommand;
use rim_services::command::dto::{CommandDefinitionDto, UserCommandDto, UserCommandStepDto};
use rim_services::{ServiceError, ServiceResult};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct PersistedUserCommandStep {
    pub command_id: String,
    pub args: Option<Value>,
}

pub(super) fn user_command_exists_locally(command_id: &str) -> bool {
    command_id.starts_with("user.")
}

pub(super) fn validate_user_command_step_shape(
    command_key: &str,
    steps: &[UserCommandStepDto],
) -> ServiceResult<()> {
    if steps.is_empty() {
        return Err(ServiceError::Validation(
            "user command must have at least one step".to_string(),
        ));
    }

    for step in steps {
        if step.command_id.trim().is_empty() {
            return Err(ServiceError::Validation(
                "user command steps must reference a command id".to_string(),
            ));
        }
        if step.command_id == command_key {
            return Err(ServiceError::Validation(
                "user command cannot reference itself".to_string(),
            ));
        }
    }

    Ok(())
}

pub(super) fn validate_user_command_steps(
    command_key: &str,
    steps: &[UserCommandStepDto],
    builtin_lookup: impl Fn(&str) -> Option<CommandDefinitionDto>,
) -> ServiceResult<()> {
    validate_user_command_step_shape(command_key, steps)?;

    for step in steps {
        if builtin_lookup(&step.command_id).is_none()
            && !user_command_exists_locally(&step.command_id)
        {
            return Err(ServiceError::Validation(format!(
                "unknown command step `{}`",
                step.command_id
            )));
        }
    }

    Ok(())
}

pub(super) fn parse_user_command(user_command: UserCommand) -> ServiceResult<UserCommandDto> {
    let steps: Vec<PersistedUserCommandStep> =
        serde_json::from_value(user_command.steps_json.clone()).map_err(ServiceError::internal)?;
    Ok(UserCommandDto {
        id: user_command.id,
        command_key: user_command.command_key,
        title: user_command.title,
        description: user_command.description,
        steps: steps
            .into_iter()
            .map(|step| UserCommandStepDto {
                command_id: step.command_id,
                args: step.args,
            })
            .collect(),
        is_enabled: user_command.is_enabled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::command::model::UserCommandId;
    use rim_services::command::dto::UserCommandStepDto;

    fn step(id: &str) -> UserCommandStepDto {
        UserCommandStepDto {
            command_id: id.to_string(),
            args: None,
        }
    }

    #[test]
    fn validate_step_shape_rejects_empty_steps() {
        let err = validate_user_command_step_shape("user.test", &[]).unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_step_shape_rejects_empty_command_id() {
        let err = validate_user_command_step_shape("user.test", &[step("  ")]).unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_step_shape_rejects_self_reference() {
        let err = validate_user_command_step_shape("user.test", &[step("user.test")]).unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_step_shape_accepts_valid_steps() {
        validate_user_command_step_shape(
            "user.test",
            &[step("chat.post_message"), step("chat.create_group")],
        )
        .unwrap();
    }

    #[test]
    fn validate_steps_rejects_unknown_command() {
        let err = validate_user_command_steps("user.test", &[step("unknown.command")], |_| None)
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(msg) if msg.contains("unknown")));
    }

    #[test]
    fn validate_steps_accepts_builtin_command() {
        validate_user_command_steps("user.test", &[step("chat.post_message")], |id| {
            if id == "chat.post_message" {
                Some(CommandDefinitionDto {
                    id: id.to_string(),
                    title: "Post".to_string(),
                    description: "desc".to_string(),
                    category: "Chat".to_string(),
                    source_kind: rim_services::command::dto::CommandSourceKindDto::BuiltIn,
                    execution_target:
                        rim_services::command::dto::CommandExecutionTargetDto::Backend,
                    arguments: vec![],
                    required_context: vec![],
                    required_capabilities: vec![],
                    visible_in_palette: true,
                    visible_in_context_menus: false,
                    keybind_capable: true,
                    dangerous: false,
                    user_command_id: None,
                    nl_description: None,
                    intent_samples: vec![],
                    ai_callable: false,
                    sensitivity: rim_services::command::sensitivity::CommandSensitivityDto::Safe,
                    typed_verb: rim_services::command::verb::TypedVerb::Other,
                    rate_limit_per_minute: None,
                    feature: None,
                })
            } else {
                None
            }
        })
        .unwrap();
    }

    #[test]
    fn validate_steps_accepts_user_command_reference() {
        // user.* commands are accepted even without builtin lookup
        validate_user_command_steps("user.test", &[step("user.other_command")], |_| None).unwrap();
    }

    #[test]
    fn user_command_exists_locally_checks_prefix() {
        assert!(user_command_exists_locally("user.my_command"));
        assert!(!user_command_exists_locally("chat.post_message"));
        assert!(!user_command_exists_locally(""));
    }

    #[test]
    fn parse_user_command_deserializes_steps() {
        let user_command = UserCommand {
            id: UserCommandId::new(),
            owner_user_id: rim_core::user::model::UserId::new(),
            command_key: "user.test".to_string(),
            title: "Test".to_string(),
            description: "desc".to_string(),
            steps_json: serde_json::json!([
                {"command_id": "chat.post_message", "args": null},
                {"command_id": "chat.create_group", "args": {"key": "val"}}
            ]),
            is_enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let dto = super::parse_user_command(user_command).unwrap();
        assert_eq!(dto.steps.len(), 2);
        assert_eq!(dto.steps[0].command_id, "chat.post_message");
        assert!(dto.steps[0].args.is_none());
        assert_eq!(dto.steps[1].command_id, "chat.create_group");
        assert!(dto.steps[1].args.is_some());
    }

    #[test]
    fn parse_user_command_invalid_json_returns_error() {
        let user_command = UserCommand {
            id: UserCommandId::new(),
            owner_user_id: rim_core::user::model::UserId::new(),
            command_key: "user.test".to_string(),
            title: "Test".to_string(),
            description: "desc".to_string(),
            steps_json: serde_json::json!("not an array"),
            is_enabled: false,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let err = super::parse_user_command(user_command).unwrap_err();
        assert!(matches!(err, ServiceError::Internal(_)));
    }
}
