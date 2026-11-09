use rim_services::command::dto::{CommandContextDto, CommandDefinitionDto};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::{Actor, ServiceError, ServiceResult};

use super::DefaultCommandService;
use super::dispatch::Dispatch;

impl Dispatch {
    /// Every command the dispatch reaches: the single command, or each pipe stage that names
    /// one.
    pub(super) fn definitions(&self) -> Vec<&CommandDefinitionDto> {
        match self {
            Self::Single(resolved) => resolved.iter().map(|r| &r.definition).collect(),
            Self::Pipe(stages) => stages
                .iter()
                .filter_map(|(_, r)| r.as_ref().map(|r| &r.definition))
                .collect(),
        }
    }

    /// The strictest sensitivity of the commands the dispatch reaches, and whether any of them
    /// is dangerous: what its audit row records. `Safe` and `false` when none resolved.
    pub(super) fn audit_profile(&self) -> (CommandSensitivityDto, bool) {
        let definitions = self.definitions();
        (
            CommandSensitivityDto::effective(
                CommandSensitivityDto::Safe,
                &definitions
                    .iter()
                    .map(|d| d.sensitivity)
                    .collect::<Vec<_>>(),
            ),
            definitions.iter().any(|d| d.dangerous),
        )
    }

    /// Returns the de-duplicated union of `required_capabilities` declared
    /// by every command the dispatch reaches (the single command, or every
    /// pipe stage).
    pub(super) fn required_capabilities(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let definitions = self.definitions();
        for def in definitions {
            for cap in &def.required_capabilities {
                if seen.insert(cap) {
                    out.push(cap.clone());
                }
            }
        }
        out
    }

    /// Returns `true` if dispatching for `initiator` requires a `ConfirmationProof`. A pipe
    /// is judged stage by stage up to its first unknown stage.
    pub(super) fn requires_confirmation(&self, initiator: CommandInitiator) -> bool {
        match self {
            Self::Single(resolved) => resolved
                .as_ref()
                .is_some_and(|r| r.definition.requires_confirmation_for_initiator(initiator)),
            Self::Pipe(stages) => {
                for (_, resolved) in stages {
                    let Some(resolved) = resolved else {
                        return false;
                    };
                    if resolved
                        .definition
                        .requires_confirmation_for_initiator(initiator)
                    {
                        return true;
                    }
                }
                false
            }
        }
    }
}

impl DefaultCommandService {
    /// Timeout gate: `Err(TimedOut { until })` when `definition` is Mutating (or
    /// stricter) and the actor is timed out in the context's workspace. Without a workspace
    /// context there is nothing to check here; the services behind the command still apply
    /// their own write gates.
    pub(super) async fn ensure_command_not_timed_out(
        &self,
        definition: &CommandDefinitionDto,
        actor: &Actor,
        context: Option<&CommandContextDto>,
    ) -> ServiceResult<()> {
        if definition.sensitivity < CommandSensitivityDto::Mutating {
            return Ok(());
        }
        let Some(server_id) = context.and_then(|c| c.workspace_id) else {
            return Ok(());
        };
        match self
            .services
            .moderation_service
            .active_timeout(server_id, actor.user_id)
            .await?
        {
            Some(until) => Err(ServiceError::TimedOut { until }),
            None => Ok(()),
        }
    }
}

/// `err` as the caller of a dispatch sees it: a technical failure becomes
/// [`ServiceError::CommandFailed`], which is safe to retry only when every command reached reads
/// and none is dangerous. Any other error is returned unchanged.
pub(super) fn with_retry_safety(
    err: ServiceError,
    sensitivity: CommandSensitivityDto,
    dangerous: bool,
) -> ServiceError {
    match err {
        failure @ (ServiceError::Unavailable
        | ServiceError::Timeout
        | ServiceError::Internal(_)
        | ServiceError::UpstreamUnavailable { .. }) => ServiceError::CommandFailed {
            failure: Box::new(failure),
            retry_safe: sensitivity < CommandSensitivityDto::Mutating && !dangerous,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_safe(err: ServiceError) -> bool {
        match err {
            ServiceError::CommandFailed { retry_safe, .. } => retry_safe,
            other => panic!("not wrapped: {other:?}"),
        }
    }

    #[test]
    fn a_failed_read_is_safe_to_retry() {
        assert!(retry_safe(with_retry_safety(
            ServiceError::Timeout,
            CommandSensitivityDto::Safe,
            false
        )));
        assert!(retry_safe(with_retry_safety(
            ServiceError::Timeout,
            CommandSensitivityDto::Sensitive,
            false
        )));
    }

    #[test]
    fn a_failed_change_is_not_safe_to_retry() {
        assert!(!retry_safe(with_retry_safety(
            ServiceError::Unavailable,
            CommandSensitivityDto::Mutating,
            false
        )));
        assert!(!retry_safe(with_retry_safety(
            ServiceError::Unavailable,
            CommandSensitivityDto::Restricted,
            false
        )));
    }

    #[test]
    fn a_failed_dangerous_read_is_not_safe_to_retry() {
        assert!(!retry_safe(with_retry_safety(
            ServiceError::Internal(None),
            CommandSensitivityDto::Safe,
            true
        )));
    }

    #[test]
    fn a_policy_refusal_is_not_wrapped() {
        assert!(matches!(
            with_retry_safety(
                ServiceError::PermissionDenied,
                CommandSensitivityDto::Safe,
                false
            ),
            ServiceError::PermissionDenied
        ));
    }
}
