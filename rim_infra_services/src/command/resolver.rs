//! Resolution of command ids to definitions for one caller within one request.

use std::collections::{BTreeSet, HashMap, HashSet};

use async_recursion::async_recursion;

use rim_services::command::dto::{
    CommandDefinitionDto, CommandExecutionTargetDto, CommandSourceKindDto,
    UserCommandCompositionDto, UserCommandDto, UserCommandStepDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;
use rim_services::{Actor, ServiceError, ServiceResult};

use super::DefaultCommandService;
use super::registry::app_command;
use super::user_commands::is_plugin_command_shape;

const RECURSION_CYCLE_DETECTED: &str = "user command recursion cycle detected";

/// The refusal of `subject` ("a user command", "a pipe") for performing more than
/// [`DefaultCommandService::MAX_EXPANDED_STEPS`] builtin steps.
pub(super) fn expanded_steps_exceeded(subject: &str) -> ServiceError {
    ServiceError::Validation(format!(
        "{subject} expands to more than {} steps",
        DefaultCommandService::MAX_EXPANDED_STEPS
    ))
}

/// A command resolved for a caller.
#[derive(Debug, Clone)]
pub(super) struct Resolved {
    pub definition: CommandDefinitionDto,
    /// Builtin steps one run of the command performs: one for a builtin, the sum over its
    /// steps for a user command.
    pub expanded_steps: usize,
    /// User commands on the longest chain of nested user commands it starts: zero for a
    /// builtin, one for a user command whose steps are all builtins.
    pub depth: usize,
}

/// What the steps of one user command add up to.
struct Composition {
    required_context: BTreeSet<String>,
    required_capabilities: BTreeSet<String>,
    keybind_capable: bool,
    dangerous: bool,
    step_sensitivities: Vec<CommandSensitivityDto>,
    expanded_steps: usize,
    depth: usize,
    /// Every step, nested ones included, is a command the server runs, and none asks for an
    /// argument.
    server_runs: bool,
    /// Every step is an AI-callable builtin the server runs, with every argument fixed.
    agent_runnable: bool,
}

/// The definition a macro step naming the plugin command `command_id` counts with: run by the
/// client, keybind-capable and `Safe`, since the client applies the plugin's own rules.
fn plugin_step_definition(command_id: &str) -> CommandDefinitionDto {
    CommandDefinitionDto {
        id: command_id.to_string(),
        title: command_id.to_string(),
        description: String::new(),
        category: String::new(),
        source_kind: CommandSourceKindDto::Plugin,
        execution_target: CommandExecutionTargetDto::DesktopLocal,
        arguments: vec![],
        required_context: vec![],
        required_capabilities: vec![],
        visible_in_palette: false,
        visible_in_context_menus: false,
        keybind_capable: true,
        dangerous: false,
        user_command_id: None,
        nl_description: None,
        intent_samples: vec![],
        ai_callable: false,
        sensitivity: CommandSensitivityDto::Safe,
        typed_verb: TypedVerb::Other,
        rate_limit_per_minute: None,
    }
}

/// Resolves command ids to definitions for one actor. Each of the actor's user commands is
/// read at most once and derived at most once, so a command named by many steps costs one
/// derivation. A user command reached again while its own derivation is open is a cycle; one
/// reached again after its derivation finished (a callee shared by two steps) is not.
pub(super) struct CommandResolver<'a> {
    service: &'a DefaultCommandService,
    actor: Actor,
    /// The actor's user commands read so far, by key; `None` for a key with no stored command.
    stored: HashMap<String, Option<UserCommandDto>>,
    /// Whether `stored` holds every user command of the actor, so a key missing from it is
    /// known to have none.
    complete: bool,
    /// Finished resolutions by command id; a refused one keeps its validation message.
    resolved: HashMap<String, Result<Resolved, String>>,
    /// User commands whose derivation is in progress.
    open: HashSet<String>,
}

impl<'a> CommandResolver<'a> {
    /// A resolver that reads the actor's user commands one key at a time, as steps name them.
    pub(super) fn new(service: &'a DefaultCommandService, actor: Actor) -> Self {
        Self {
            service,
            actor,
            stored: HashMap::new(),
            complete: false,
            resolved: HashMap::new(),
            open: HashSet::new(),
        }
    }

    /// A resolver over `user_commands`, all of the actor's user commands, that reads nothing.
    pub(super) fn with_user_commands(
        service: &'a DefaultCommandService,
        actor: Actor,
        user_commands: &[UserCommandDto],
    ) -> Self {
        Self {
            stored: user_commands
                .iter()
                .map(|command| (command.command_key.clone(), Some(command.clone())))
                .collect(),
            complete: true,
            ..Self::new(service, actor)
        }
    }

    /// The user commands read so far, by key: every user command a resolution reached.
    pub(super) fn into_user_commands(self) -> HashMap<String, UserCommandDto> {
        self.stored
            .into_iter()
            .filter_map(|(key, command)| command.map(|command| (key, command)))
            .collect()
    }

    /// Resolves `command_id` to a builtin, or to one of the actor's enabled user commands.
    /// Returns `None` when it names neither.
    ///
    /// # Errors
    ///
    /// `Validation` when a user command's steps name an unknown or client-run command, form a
    /// cycle, or expand to more than [`DefaultCommandService::MAX_EXPANDED_STEPS`] builtin
    /// steps; a repository error from reading a user command.
    #[async_recursion]
    pub(super) async fn resolve(&mut self, command_id: &str) -> ServiceResult<Option<Resolved>> {
        if let Some(done) = self.resolved.get(command_id) {
            return done.clone().map(Some).map_err(ServiceError::Validation);
        }
        if let Some(definition) = self.service.builtin_or_test_definition(command_id) {
            let resolved = Resolved {
                definition,
                expanded_steps: 1,
                depth: 0,
            };
            self.resolved
                .insert(command_id.to_string(), Ok(resolved.clone()));
            return Ok(Some(resolved));
        }
        match self.user_command(command_id).await? {
            Some(user_command) if user_command.is_enabled => {
                self.resolve_user(&user_command).await.map(Some)
            }
            _ => Ok(None),
        }
    }

    /// Resolves `user_command`, enabled or not, to its definition.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve`].
    pub(super) async fn resolve_user(
        &mut self,
        user_command: &UserCommandDto,
    ) -> ServiceResult<Resolved> {
        let key = user_command.command_key.as_str();
        if let Some(done) = self.resolved.get(key) {
            return done.clone().map_err(ServiceError::Validation);
        }
        let result = self
            .compose(key, &user_command.steps)
            .await
            .map(|composition| Resolved {
                definition: user_command.as_command_definition_with_metadata(
                    UserCommandCompositionDto {
                        execution_target: if composition.server_runs {
                            CommandExecutionTargetDto::Backend
                        } else {
                            CommandExecutionTargetDto::DesktopLocal
                        },
                        required_context: composition.required_context.into_iter().collect(),
                        required_capabilities: composition
                            .required_capabilities
                            .into_iter()
                            .collect(),
                        keybind_capable: composition.keybind_capable,
                        dangerous: composition.dangerous,
                        step_sensitivities: composition.step_sensitivities,
                        agent_runnable: composition.agent_runnable,
                    },
                ),
                expanded_steps: composition.expanded_steps,
                depth: composition.depth + 1,
            });
        match &result {
            Ok(resolved) => {
                self.resolved.insert(key.to_string(), Ok(resolved.clone()));
            }
            Err(ServiceError::Validation(message)) => {
                self.resolved.insert(key.to_string(), Err(message.clone()));
            }
            Err(_) => {}
        }
        result
    }

    /// Checks that `steps`, as the steps of the user command `command_key`, each resolve to a
    /// command without reaching `command_key` again. Returns whether the AI agent may run a
    /// macro with these steps: each an AI-callable builtin the server runs, with every argument
    /// fixed.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve`].
    pub(super) async fn check_steps(
        &mut self,
        command_key: &str,
        steps: &[UserCommandStepDto],
    ) -> ServiceResult<bool> {
        self.compose(command_key, steps)
            .await
            .map(|composition| composition.agent_runnable)
    }

    /// Resolves the steps of the user command `command_key` while it is open.
    async fn compose(
        &mut self,
        command_key: &str,
        steps: &[UserCommandStepDto],
    ) -> ServiceResult<Composition> {
        if !self.open.insert(command_key.to_string()) {
            return Err(ServiceError::Validation(
                RECURSION_CYCLE_DETECTED.to_string(),
            ));
        }
        let composition = self.compose_open(steps).await;
        self.open.remove(command_key);
        composition
    }

    async fn compose_open(&mut self, steps: &[UserCommandStepDto]) -> ServiceResult<Composition> {
        let mut composition = Composition {
            required_context: BTreeSet::new(),
            required_capabilities: BTreeSet::new(),
            keybind_capable: true,
            dangerous: false,
            step_sensitivities: Vec::with_capacity(steps.len()),
            expanded_steps: 0,
            depth: 0,
            server_runs: true,
            agent_runnable: true,
        };
        for step in steps {
            let resolved = self.resolve_step(&step.command_id).await?.ok_or_else(|| {
                ServiceError::Validation(format!("unknown command step `{}`", step.command_id))
            })?;
            let definition = resolved.definition;
            let server_runs = definition.execution_target == CommandExecutionTargetDto::Backend
                && step.ask.is_empty();
            composition.server_runs &= server_runs;
            composition.agent_runnable &= server_runs
                && definition.source_kind == CommandSourceKindDto::BuiltIn
                && definition.ai_callable;
            composition.expanded_steps = composition
                .expanded_steps
                .saturating_add(resolved.expanded_steps);
            if composition.expanded_steps > DefaultCommandService::MAX_EXPANDED_STEPS {
                return Err(expanded_steps_exceeded("a user command"));
            }
            composition.depth = composition.depth.max(resolved.depth);
            composition
                .required_context
                .extend(definition.required_context);
            composition
                .required_capabilities
                .extend(definition.required_capabilities);
            composition.keybind_capable &= definition.keybind_capable;
            composition.dangerous |= definition.dangerous;
            composition.step_sensitivities.push(definition.sensitivity);
        }
        Ok(composition)
    }

    /// Resolves the macro step `command_id`: as [`Self::resolve`], else to an app command, else,
    /// for an id shaped like a plugin's, to [`plugin_step_definition`]. Each counts one step.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve`].
    async fn resolve_step(&mut self, command_id: &str) -> ServiceResult<Option<Resolved>> {
        if let Some(resolved) = self.resolve(command_id).await? {
            return Ok(Some(resolved));
        }
        let definition = app_command(command_id).or_else(|| {
            is_plugin_command_shape(command_id).then(|| plugin_step_definition(command_id))
        });
        Ok(definition.map(|definition| Resolved {
            definition,
            expanded_steps: 1,
            depth: 0,
        }))
    }

    /// The actor's user command `key`, read once.
    async fn user_command(&mut self, key: &str) -> ServiceResult<Option<UserCommandDto>> {
        if let Some(command) = self.stored.get(key) {
            return Ok(command.clone());
        }
        if self.complete {
            return Ok(None);
        }
        let command = self
            .service
            .repos
            .user_command_repo
            .get_by_key_for_user(self.actor.user_id, key)
            .await?
            .map(DefaultCommandService::parse_user_command)
            .transpose()?;
        self.stored.insert(key.to_string(), command.clone());
        Ok(command)
    }
}
