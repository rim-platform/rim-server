use std::collections::{HashMap, HashSet};

use serde_json::Value;

use rim_core::command::model::UserCommand;
use rim_services::command::dto::{CommandDefinitionDto, UserCommandDto, UserCommandStepDto};
use rim_services::{ServiceError, ServiceResult};

use super::DefaultCommandService;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct PersistedUserCommandStep {
    pub command_id: String,
    pub args: Option<Value>,
}

pub(super) fn user_command_exists_locally(command_id: &str) -> bool {
    command_id.starts_with("user.")
}

/// Refuses `steps` for the user command `command_key` when there are none, more than
/// [`DefaultCommandService::MAX_USER_COMMAND_STEPS`], one without a command id, or one naming
/// `command_key` itself.
pub(super) fn validate_user_command_step_shape(
    command_key: &str,
    steps: &[UserCommandStepDto],
) -> ServiceResult<()> {
    if steps.is_empty() {
        return Err(ServiceError::Validation(
            "user command must have at least one step".to_string(),
        ));
    }
    if steps.len() > DefaultCommandService::MAX_USER_COMMAND_STEPS {
        return Err(ServiceError::Validation(format!(
            "a user command may have at most {} steps",
            DefaultCommandService::MAX_USER_COMMAND_STEPS
        )));
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

/// Number of user commands in the longest chain through `command_key` once it has `steps`:
/// the commands calling it, itself, and the commands it calls.
pub(super) fn longest_chain_through(
    stored: &[UserCommandDto],
    command_key: &str,
    steps: &[UserCommandStepDto],
) -> usize {
    let mut calls: HashMap<&str, Vec<&str>> = stored
        .iter()
        .filter(|command| command.command_key != command_key)
        .map(|command| {
            (
                command.command_key.as_str(),
                user_command_steps(&command.steps),
            )
        })
        .collect();
    calls.insert(command_key, user_command_steps(steps));

    let mut callers: HashMap<&str, Vec<&str>> = HashMap::new();
    for (&caller, callees) in &calls {
        for &callee in callees {
            callers.entry(callee).or_default().push(caller);
        }
    }

    longest_path(&calls, command_key, &mut HashMap::new())
        + longest_path(&callers, command_key, &mut HashMap::new())
        - 1
}

/// Most builtin steps one run performs among `command_key`, once it has `steps`, and the stored
/// commands that call it directly or through others. A step naming one of the stored commands
/// counts that command's steps; any other step counts one.
pub(super) fn most_expanded_steps_through(
    stored: &[UserCommandDto],
    command_key: &str,
    steps: &[UserCommandStepDto],
) -> usize {
    let mut steps_by_key: HashMap<&str, &[UserCommandStepDto]> = stored
        .iter()
        .filter(|command| command.command_key != command_key)
        .map(|command| (command.command_key.as_str(), command.steps.as_slice()))
        .collect();
    steps_by_key.insert(command_key, steps);

    let mut callers: HashMap<&str, Vec<&str>> = HashMap::new();
    for (&caller, steps) in &steps_by_key {
        for step in steps.iter() {
            if steps_by_key.contains_key(step.command_id.as_str()) {
                callers
                    .entry(step.command_id.as_str())
                    .or_default()
                    .push(caller);
            }
        }
    }
    let mut affected = vec![command_key];
    let mut reached: HashSet<&str> = HashSet::from([command_key]);
    let mut next = 0;
    while let Some(&callee) = affected.get(next) {
        for &caller in callers.get(callee).into_iter().flatten() {
            if reached.insert(caller) {
                affected.push(caller);
            }
        }
        next += 1;
    }

    let mut memo = HashMap::new();
    affected
        .into_iter()
        .map(|key| expanded_steps(&steps_by_key, key, &mut memo))
        .max()
        .unwrap_or(0)
}

/// Builtin steps one run of `key` performs, with `steps_by_key` holding the steps of every user
/// command. A command reached again while its own count is open counts zero, so a cycle in
/// stored data ends the walk.
fn expanded_steps<'a>(
    steps_by_key: &HashMap<&'a str, &'a [UserCommandStepDto]>,
    key: &'a str,
    memo: &mut HashMap<&'a str, Option<usize>>,
) -> usize {
    match memo.get(key) {
        Some(Some(count)) => return *count,
        Some(None) => return 0,
        None => {}
    }
    memo.insert(key, None);
    let mut count = 0usize;
    for step in steps_by_key.get(key).copied().unwrap_or_default() {
        let step_count = match steps_by_key.get_key_value(step.command_id.as_str()) {
            Some((&callee, _)) => expanded_steps(steps_by_key, callee, memo),
            None => 1,
        };
        count = count.saturating_add(step_count);
    }
    memo.insert(key, Some(count));
    count
}

fn user_command_steps(steps: &[UserCommandStepDto]) -> Vec<&str> {
    steps
        .iter()
        .map(|step| step.command_id.as_str())
        .filter(|command_id| user_command_exists_locally(command_id))
        .collect()
}

/// Nodes on the longest path from `node` along `edges`, `node` included. A node reached again
/// while its own path is open counts as zero, so a cycle in stored data ends the walk.
fn longest_path<'a>(
    edges: &HashMap<&'a str, Vec<&'a str>>,
    node: &'a str,
    memo: &mut HashMap<&'a str, Option<usize>>,
) -> usize {
    match memo.get(node) {
        Some(Some(length)) => return *length,
        Some(None) => return 0,
        None => {}
    }
    memo.insert(node, None);
    let longest = edges
        .get(node)
        .into_iter()
        .flatten()
        .map(|&next| longest_path(edges, next, memo))
        .max()
        .unwrap_or(0);
    memo.insert(node, Some(longest + 1));
    longest + 1
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
