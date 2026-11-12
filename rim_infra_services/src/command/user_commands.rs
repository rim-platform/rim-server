use std::collections::{HashMap, HashSet};

use serde_json::Value;

use rim_core::command::model::UserCommand;
use rim_services::command::bounds::{
    MAX_STEP_ARGS_BYTES, ensure_args_size, ensure_command_id_size,
};
use rim_services::command::dto::{
    CommandDefinitionDto, UpdateUserCommandStepDto, UserCommandDto, UserCommandStepDto,
};
use rim_services::{ServiceError, ServiceResult};

use super::DefaultCommandService;
use super::registry::{app_command, is_agent_management_command};

/// Argument names one step may ask for.
const MAX_ASKED_ARGUMENTS: usize = 32;
/// Bytes of one asked argument name.
const MAX_ASKED_ARGUMENT_NAME_BYTES: usize = 64;

/// One stored step of a macro, as `steps_json` keeps it. A step stored before steps could ask
/// for arguments reads with none.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct PersistedUserCommandStep {
    pub command_id: String,
    pub args: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ask: Vec<String>,
}

impl From<&UserCommandStepDto> for PersistedUserCommandStep {
    fn from(step: &UserCommandStepDto) -> Self {
        Self {
            command_id: step.command_id.clone(),
            args: step.args.clone(),
            ask: step.ask.clone(),
        }
    }
}

/// `steps` as `steps_json` stores them.
///
/// # Errors
///
/// `Internal` when they do not serialize.
pub(super) fn persisted_steps(steps: &[UserCommandStepDto]) -> ServiceResult<Value> {
    serde_json::to_value(
        steps
            .iter()
            .map(PersistedUserCommandStep::from)
            .collect::<Vec<_>>(),
    )
    .map_err(ServiceError::internal)
}

/// The steps of an update, each without `ask` taking what the stored step at the same position
/// asked for when that step names the same command.
pub(super) fn steps_with_stored_asks(
    steps: Vec<UpdateUserCommandStepDto>,
    stored: &[UserCommandStepDto],
) -> Vec<UserCommandStepDto> {
    steps
        .into_iter()
        .enumerate()
        .map(|(index, step)| {
            let ask = step.ask.unwrap_or_else(|| {
                stored
                    .get(index)
                    .filter(|stored| stored.command_id == step.command_id)
                    .map(|stored| stored.ask.clone())
                    .unwrap_or_default()
            });
            UserCommandStepDto {
                command_id: step.command_id,
                args: step.args,
                ask,
            }
        })
        .collect()
}

/// Whether `command_id` has the shape of a plugin command, `plugin.<pluginId>.<command>`: at
/// least four segments after `plugin.`, each one or more ASCII letters, digits, `_` or `-`, the
/// first three, the plugin id, without `_`.
pub(super) fn is_plugin_command_shape(command_id: &str) -> bool {
    let Some(rest) = command_id.strip_prefix("plugin.") else {
        return false;
    };
    let segments: Vec<&str> = rest.split('.').collect();
    segments.len() >= 4
        && segments.iter().enumerate().all(|(index, segment)| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || (b == b'_' && index >= 3))
        })
}

pub(super) fn user_command_exists_locally(command_id: &str) -> bool {
    command_id.starts_with("user.")
}

/// Whether `command_key` is `user.` followed by one or more lowercase letters, digits, `.`,
/// `_` or `-`.
pub(super) fn is_valid_user_command_key(command_key: &str) -> bool {
    command_key.strip_prefix("user.").is_some_and(|key| {
        !key.is_empty()
            && key.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            })
    })
}

/// Refuses `steps` for the user command `command_key` when there are none, more than
/// [`DefaultCommandService::MAX_USER_COMMAND_STEPS`], one without a command id or with one
/// longer than 256 bytes, one whose arguments serialize to more than [`MAX_STEP_ARGS_BYTES`],
/// or one naming `command_key` itself.
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
        ensure_command_id_size(&step.command_id, "a user command step's command id")?;
        ensure_args_size(
            step.args.as_ref(),
            MAX_STEP_ARGS_BYTES,
            "a user command step's args",
        )?;
        if step.command_id == command_key {
            return Err(ServiceError::Validation(
                "user command cannot reference itself".to_string(),
            ));
        }
        validate_asked_arguments(step)?;
    }

    Ok(())
}

/// Refuses a step that asks for more than [`MAX_ASKED_ARGUMENTS`] arguments, for one whose name
/// is empty or longer than [`MAX_ASKED_ARGUMENT_NAME_BYTES`] bytes, for one name twice, or for a
/// name its `args` also fix.
///
/// # Errors
///
/// `Validation` naming the rule.
fn validate_asked_arguments(step: &UserCommandStepDto) -> ServiceResult<()> {
    if step.ask.len() > MAX_ASKED_ARGUMENTS {
        return Err(ServiceError::Validation(format!(
            "a step may ask for at most {MAX_ASKED_ARGUMENTS} arguments"
        )));
    }
    let fixed = step.args.as_ref().and_then(Value::as_object);
    let mut seen = HashSet::new();
    for name in &step.ask {
        if name.is_empty() || name.len() > MAX_ASKED_ARGUMENT_NAME_BYTES {
            return Err(ServiceError::Validation(format!(
                "an asked argument name may be 1 to {MAX_ASKED_ARGUMENT_NAME_BYTES} bytes"
            )));
        }
        if !seen.insert(name.as_str()) {
            return Err(ServiceError::Validation(format!(
                "a step asks for `{name}` twice"
            )));
        }
        if fixed.is_some_and(|args| args.contains_key(name)) {
            return Err(ServiceError::Validation(format!(
                "a step both fixes and asks for `{name}`"
            )));
        }
    }
    Ok(())
}

/// Refuses `steps` for the macro `command_key` unless each names a builtin, an app command other
/// than an agent-management one, a command shaped like a plugin's, or, only where `stored`, the
/// macro as saved before, already names it, another macro; and unless each passes
/// [`validate_user_command_step_shape`].
///
/// # Errors
///
/// `Validation` naming the step refused.
pub(super) fn validate_user_command_steps(
    command_key: &str,
    steps: &[UserCommandStepDto],
    stored: Option<&UserCommandDto>,
    builtin_lookup: impl Fn(&str) -> Option<CommandDefinitionDto>,
) -> ServiceResult<()> {
    validate_user_command_step_shape(command_key, steps)?;

    for step in steps {
        let command_id = step.command_id.as_str();
        if user_command_exists_locally(command_id) {
            let kept = stored.is_some_and(|stored| {
                stored
                    .steps
                    .iter()
                    .any(|stored_step| stored_step.command_id == command_id)
            });
            if !kept {
                return Err(ServiceError::Validation(
                    "macro steps must not reference other macros".to_string(),
                ));
            }
        } else if is_agent_management_command(command_id) {
            return Err(ServiceError::Validation(format!(
                "macro steps must not manage agents (`{command_id}`)"
            )));
        } else if builtin_lookup(command_id).is_none()
            && app_command(command_id).is_none()
            && !is_plugin_command_shape(command_id)
        {
            return Err(ServiceError::Validation(format!(
                "unknown command step `{command_id}`"
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
                ask: step.ask,
            })
            .collect(),
        is_enabled: user_command.is_enabled,
        ai_callable: user_command.ai_callable,
        nl_description: user_command.nl_description,
        intent_samples: user_command.intent_samples,
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
            ask: vec![],
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
        let err =
            validate_user_command_steps("user.test", &[step("unknown.command")], None, |_| None)
                .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(msg) if msg.contains("unknown")));
    }

    #[test]
    fn validate_steps_accepts_builtin_command() {
        validate_user_command_steps("user.test", &[step("chat.post_message")], None, |id| {
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

    /// A new macro's step may not name another macro; an update keeps a step naming one that
    /// the stored macro already names.
    #[test]
    fn a_macro_step_names_another_macro_only_where_it_did_before() {
        let err = validate_user_command_steps("user.test", &[step("user.other")], None, |_| None)
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "macro steps must not reference other macros"),
            "{err:?}"
        );

        let stored = UserCommandDto {
            id: UserCommandId::new(),
            command_key: "user.test".to_string(),
            title: "Test".to_string(),
            description: String::new(),
            steps: vec![step("chat.mark_as_read"), step("user.other")],
            is_enabled: true,
            ai_callable: false,
            nl_description: String::new(),
            intent_samples: vec![],
        };
        validate_user_command_steps(
            "user.test",
            &[step("user.other"), step("chat.mark_as_read")],
            Some(&stored),
            DefaultCommandService::builtin_definition,
        )
        .expect("a nested macro the stored macro names is kept");
        let err =
            validate_user_command_steps("user.test", &[step("user.third")], Some(&stored), |_| {
                None
            })
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }

    /// App commands and plugin-shaped ids are steps; agent-management ids are not.
    #[test]
    fn macro_steps_take_app_and_plugin_commands_but_not_agent_management() {
        for id in [
            "workspace.toggle_zen_mode",
            "settings.open.keybinds",
            "plugin.dev.example.poll.poll",
            "plugin.dev.example.k8s.restart_pod",
            "agents.allow_in_encrypted_conversation",
        ] {
            validate_user_command_steps("user.test", &[step(id)], None, |_| None)
                .unwrap_or_else(|err| panic!("{id}: {err:?}"));
        }
        for id in [
            "agents.approve_request",
            "agents.disconnect",
            "agents.open_runs",
        ] {
            let err =
                validate_user_command_steps("user.test", &[step(id)], None, |_| None).unwrap_err();
            assert!(
                matches!(&err, ServiceError::Validation(m) if m == &format!("macro steps must not manage agents (`{id}`)")),
                "{id}: {err:?}"
            );
        }
        for id in [
            "plugin.k8s.restart",
            "plugin.dev.ex_ample.k8s.restart",
            "workspace.nope",
        ] {
            let err =
                validate_user_command_steps("user.test", &[step(id)], None, |_| None).unwrap_err();
            assert!(
                matches!(&err, ServiceError::Validation(m) if m == &format!("unknown command step `{id}`")),
                "{id}: {err:?}"
            );
        }
    }

    #[test]
    fn plugin_command_shape_takes_a_three_segment_plugin_id_and_a_command() {
        assert!(is_plugin_command_shape(
            "plugin.dev.example.hello-world.say_hi"
        ));
        assert!(is_plugin_command_shape(
            "plugin.dev.example.poll.poll.results"
        ));
        assert!(is_plugin_command_shape("plugin.Dev.Example.Tool.Run"));
        assert!(!is_plugin_command_shape("plugin.dev.example.poll"));
        assert!(!is_plugin_command_shape("plugin.dev..poll.x"));
        assert!(!is_plugin_command_shape("plugin.dev.example.poll.bad id"));
        assert!(!is_plugin_command_shape("plugin.dev_x.example.poll.run"));
        assert!(!is_plugin_command_shape("plugins.dev.example.poll.run"));
    }

    /// An asked argument list is bounded, unique and apart from the fixed arguments.
    #[test]
    fn asked_arguments_are_bounded_unique_and_not_fixed() {
        let asking = |ask: Vec<String>, args: Option<Value>| UserCommandStepDto {
            command_id: "chat.post_message".to_string(),
            args,
            ask,
        };
        validate_user_command_step_shape(
            "user.test",
            &[asking(
                vec!["text".to_string()],
                Some(serde_json::json!({ "server_id": "s" })),
            )],
        )
        .expect("a distinct asked argument is accepted");

        let cases = [
            (
                asking((0..33).map(|i| format!("a{i}")).collect(), None),
                "a step may ask for at most 32 arguments".to_string(),
            ),
            (
                asking(vec![String::new()], None),
                "an asked argument name may be 1 to 64 bytes".to_string(),
            ),
            (
                asking(vec!["x".repeat(65)], None),
                "an asked argument name may be 1 to 64 bytes".to_string(),
            ),
            (
                asking(vec!["text".to_string(), "text".to_string()], None),
                "a step asks for `text` twice".to_string(),
            ),
            (
                asking(
                    vec!["text".to_string()],
                    Some(serde_json::json!({ "text": "hi" })),
                ),
                "a step both fixes and asks for `text`".to_string(),
            ),
        ];
        for (step, refusal) in cases {
            let err = validate_user_command_step_shape("user.test", &[step]).unwrap_err();
            assert!(
                matches!(&err, ServiceError::Validation(m) if m == &refusal),
                "{refusal}: {err:?}"
            );
        }
    }

    /// A step stored before steps could ask reads with none, and one that asks for nothing is
    /// stored without the field.
    #[test]
    fn persisted_steps_read_old_rows_and_omit_an_empty_ask() {
        let old: PersistedUserCommandStep = serde_json::from_value(
            serde_json::json!({ "command_id": "chat.mark_as_read", "args": null }),
        )
        .unwrap();
        assert!(old.ask.is_empty());

        let stored = persisted_steps(&[
            step("chat.mark_as_read"),
            UserCommandStepDto {
                command_id: "chat.post_message".to_string(),
                args: None,
                ask: vec!["text".to_string()],
            },
        ])
        .unwrap();
        assert_eq!(
            stored,
            serde_json::json!([
                { "command_id": "chat.mark_as_read", "args": null },
                { "command_id": "chat.post_message", "args": null, "ask": ["text"] }
            ])
        );
    }

    /// An update's step without `ask` keeps the stored step's when both name the same command.
    #[test]
    fn update_steps_keep_the_stored_ask_of_the_same_command() {
        let stored = vec![
            UserCommandStepDto {
                command_id: "chat.post_message".to_string(),
                args: None,
                ask: vec!["text".to_string()],
            },
            UserCommandStepDto {
                command_id: "chat.set_topic".to_string(),
                args: None,
                ask: vec!["topic".to_string()],
            },
        ];
        let update = |command_id: &str, ask: Option<Vec<String>>| UpdateUserCommandStepDto {
            command_id: command_id.to_string(),
            args: None,
            ask,
        };

        let steps = steps_with_stored_asks(
            vec![
                update("chat.post_message", None),
                update("chat.mark_as_read", None),
                update("chat.set_topic", Some(vec![])),
            ],
            &stored,
        );

        assert_eq!(steps[0].ask, vec!["text".to_string()]);
        assert!(steps[1].ask.is_empty());
        assert!(steps[2].ask.is_empty());
    }

    #[test]
    fn user_command_exists_locally_checks_prefix() {
        assert!(user_command_exists_locally("user.my_command"));
        assert!(!user_command_exists_locally("chat.post_message"));
        assert!(!user_command_exists_locally(""));
    }

    #[test]
    fn user_command_keys_take_lowercase_letters_digits_dots_dashes_and_underscores() {
        assert!(is_valid_user_command_key("user.greet"));
        assert!(is_valid_user_command_key("user.a-b_c.1"));
        assert!(!is_valid_user_command_key("user."));
        assert!(!is_valid_user_command_key("user.Bad"));
        assert!(!is_valid_user_command_key("user.bad id"));
        assert!(!is_valid_user_command_key("user.bad!"));
        assert!(!is_valid_user_command_key("user.é"));
        assert!(!is_valid_user_command_key("greet"));
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
            ai_callable: false,
            nl_description: String::new(),
            intent_samples: vec![],
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
            ai_callable: false,
            nl_description: String::new(),
            intent_samples: vec![],
        };
        let err = super::parse_user_command(user_command).unwrap_err();
        assert!(matches!(err, ServiceError::Internal(_)));
    }
}
