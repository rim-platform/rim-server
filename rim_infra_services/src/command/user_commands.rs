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

pub(super) fn validate_user_command_steps(
    command_key: &str,
    steps: &[UserCommandStepDto],
    builtin_lookup: impl Fn(&str) -> Option<CommandDefinitionDto>,
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
