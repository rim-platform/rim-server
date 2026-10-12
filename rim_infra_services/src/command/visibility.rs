//! Which commands a caller may run in a context: the rule the command listings filter by.

use std::collections::{BTreeSet, HashMap};

use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_services::command::dto::{
    CommandContextDto, CommandDefinitionDto, CommandExecutionTargetDto,
};
use rim_services::server_member::command::GetServerMemberQuery;
use rim_services::{Actor, ServiceError, ServiceResult};

use super::DefaultCommandService;
use super::registry;

/// The caller's membership in the workspace a listing is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Membership {
    pub role: ServerMemberRole,
    /// The workspace is the direct space.
    pub direct_space: bool,
}

/// The step command ids of each of the caller's user commands, by key.
pub(super) type UserCommandSteps = HashMap<String, Vec<String>>;

/// Whether the server runs `definition` against a workspace. The commands the client runs
/// itself need none and are listed in every context.
pub(super) fn needs_workspace(definition: &CommandDefinitionDto) -> bool {
    definition.execution_target == CommandExecutionTargetDto::Backend
}

/// Whether a caller may run `definition`: one that needs a workspace needs `membership` (`None`
/// when the context names no workspace or the caller is not a member of it) with a role every
/// step admits, and every command needs each of its capabilities to be `true` in `held`.
pub(super) fn may_run(
    definition: &CommandDefinitionDto,
    membership: Option<Membership>,
    held: &HashMap<String, bool>,
    user_steps: &UserCommandSteps,
) -> bool {
    let capabilities_held = definition
        .required_capabilities
        .iter()
        .all(|cap| held.get(cap).copied().unwrap_or(false));
    if !needs_workspace(definition) {
        return capabilities_held;
    }
    membership.is_some_and(|membership| {
        capabilities_held && role_admits(&definition.id, membership, user_steps, 0)
    })
}

/// Whether `membership` meets the role `command_id` needs; for a user command, the role each of
/// its steps needs. A chain deeper than the executor runs admits nobody.
fn role_admits(
    command_id: &str,
    membership: Membership,
    user_steps: &UserCommandSteps,
    depth: usize,
) -> bool {
    match user_steps.get(command_id) {
        Some(steps) => {
            depth < DefaultCommandService::MAX_RECURSION_DEPTH
                && steps
                    .iter()
                    .all(|step| role_admits(step, membership, user_steps, depth + 1))
        }
        None => {
            registry::required_role(command_id).admits(membership.role, membership.direct_space)
        }
    }
}

impl DefaultCommandService {
    /// Keeps the commands `actor` may run in `context`, by [`may_run`]. The membership lookup
    /// makes a local user a guest of the direct space, as other reads there do.
    pub(super) async fn retain_runnable(
        &self,
        actor: &Actor,
        context: Option<&CommandContextDto>,
        commands: Vec<CommandDefinitionDto>,
        user_steps: &UserCommandSteps,
    ) -> ServiceResult<Vec<CommandDefinitionDto>> {
        let workspace_id = context.and_then(|c| c.workspace_id);
        let membership = match workspace_id {
            Some(workspace_id) if commands.iter().any(needs_workspace) => {
                self.membership(actor, workspace_id).await?
            }
            _ => None,
        };
        let candidates: Vec<CommandDefinitionDto> = commands
            .into_iter()
            .filter(|c| membership.is_some() || !needs_workspace(c))
            .collect();

        let capabilities: Vec<String> = candidates
            .iter()
            .flat_map(|c| c.required_capabilities.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let held = if capabilities.is_empty() {
            HashMap::new()
        } else {
            self.services
                .capability_service
                .check_many(actor, workspace_id, &capabilities)
                .await?
        };

        Ok(candidates
            .into_iter()
            .filter(|c| may_run(c, membership, &held, user_steps))
            .collect())
    }

    /// The actor's membership in `workspace_id`, `None` when they are not a member.
    async fn membership(
        &self,
        actor: &Actor,
        workspace_id: ServerId,
    ) -> ServiceResult<Option<Membership>> {
        let member = self
            .services
            .server_member_service
            .get_member(GetServerMemberQuery {
                actor: *actor,
                server_id: workspace_id,
                user_id: actor.user_id,
            })
            .await;
        match member {
            Ok(member) => Ok(Some(Membership {
                role: member.member_role,
                direct_space: self.direct_space_id == Some(workspace_id),
            })),
            Err(ServiceError::PermissionDenied | ServiceError::NotFound) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use rim_services::command::dto::CommandArgumentTypeDto;

    use super::*;
    use crate::command::registry::builtin_commands;

    /// `needs_workspace` reads the execution target; this pins what that relies on: every
    /// builtin the server runs takes a workspace, and none the client runs needs a capability.
    #[test]
    fn server_run_builtins_take_a_workspace_and_client_run_ones_need_no_capability() {
        for command in builtin_commands() {
            if needs_workspace(&command) {
                let takes_workspace = command.required_context.iter().any(|k| k == "workspace")
                    || command.arguments.iter().any(|a| {
                        a.name == "server_id"
                            && a.argument_type == CommandArgumentTypeDto::WorkspaceId
                    });
                assert!(takes_workspace, "{}", command.id);
            } else {
                assert!(command.required_capabilities.is_empty(), "{}", command.id);
            }
        }
    }

    #[test]
    fn a_user_command_chain_deeper_than_the_executor_runs_admits_nobody() {
        let owner = Membership {
            role: ServerMemberRole::Owner,
            direct_space: false,
        };
        let chain = |levels: usize| -> UserCommandSteps {
            (0..levels)
                .map(|level| {
                    let step = if level + 1 == levels {
                        "chat.post_message".to_string()
                    } else {
                        format!("user.level{}", level + 1)
                    };
                    (format!("user.level{level}"), vec![step])
                })
                .collect()
        };
        let mut top = builtin_commands()
            .into_iter()
            .find(|c| c.id == "chat.post_message")
            .expect("chat.post_message");
        top.id = "user.level0".to_string();
        top.required_capabilities.clear();
        let held = HashMap::new();
        let limit = DefaultCommandService::MAX_RECURSION_DEPTH;

        assert!(may_run(&top, Some(owner), &held, &chain(limit)));
        assert!(!may_run(&top, Some(owner), &held, &chain(limit + 1)));
    }
}
