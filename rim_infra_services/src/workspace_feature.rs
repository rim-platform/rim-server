use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::workspace_feature::model::WorkspaceFeature;
use rim_core::workspace_feature::repository::WorkspaceFeatureRepository;
use rim_services::audit::dto::{AuditLogEntryDto, AuditOutcome};
use rim_services::audit::service::AuditLogService;
use rim_services::command::verb::TypedVerb;
use rim_services::workspace_feature::command::{
    ListWorkspaceFeaturesQuery, SetWorkspaceFeatureCmd,
};
use rim_services::workspace_feature::dto::WorkspaceFeatureStateDto;
use rim_services::workspace_feature::service::WorkspaceFeatureService;
use rim_services::{ServiceError, ServiceResult};

#[derive(Clone)]
pub struct WorkspaceFeatureRepos {
    pub workspace_feature_repo: Arc<dyn WorkspaceFeatureRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

/// Pseudo command id under which a feature toggle is recorded in `command_audit_log`. Not a
/// registered command: the toggle is a settings mutation, but it reuses the command audit
/// table so workspace-configuration changes show up next to command executions.
pub(crate) const SET_FEATURE_AUDIT_ID: &str = "workspace.set_feature";

pub struct DefaultWorkspaceFeatureService {
    repos: WorkspaceFeatureRepos,
    /// `None` in unit tests; the composition root always wires it.
    audit_log_service: Option<Arc<dyn AuditLogService>>,
}

impl DefaultWorkspaceFeatureService {
    pub fn new(repos: WorkspaceFeatureRepos) -> Self {
        Self {
            repos,
            audit_log_service: None,
        }
    }

    pub fn with_audit_log_service(mut self, audit_log_service: Arc<dyn AuditLogService>) -> Self {
        self.audit_log_service = Some(audit_log_service);
        self
    }

    /// Records a successful toggle on both audit sinks used by command execution: the
    /// `target: "audit"` tracing line and, when wired, a `command_audit_log` row. A failed DB
    /// write is logged and swallowed; the toggle itself already succeeded.
    async fn audit_toggle(&self, cmd: &SetWorkspaceFeatureCmd) {
        tracing::info!(
            target: "audit",
            event = "workspace_feature_set",
            actor_id = %cmd.actor.user_id,
            server_id = %cmd.server_id,
            feature = %cmd.feature,
            enabled = cmd.enabled,
        );
        let Some(audit_svc) = &self.audit_log_service else {
            return;
        };
        let entry = AuditLogEntryDto {
            id: uuid::Uuid::now_v7(),
            actor_user_id: cmd.actor.user_id.into(),
            server_id: cmd.server_id.into(),
            command_id: SET_FEATURE_AUDIT_ID.to_string(),
            initiator: "settings".to_string(),
            args_redacted: serde_json::json!({
                "feature": cmd.feature.as_str(),
                "enabled": cmd.enabled,
            }),
            sensitivity: "restricted".to_string(),
            dangerous: false,
            confirmation_kind: "none".to_string(),
            typed_verb: Some(TypedVerb::Configure),
            outcome: AuditOutcome::Success,
            error_kind: None,
            capability_check_outcome: None,
            confirmation_outcome: None,
            parent_audit_id: None,
            created_at: chrono::Utc::now(),
        };
        if let Err(e) = audit_svc.write_entry(entry).await {
            tracing::warn!(
                event = "audit_db_write_failed",
                error = %e,
                "audit DB write failed for workspace feature toggle (tracing line already emitted)"
            );
        }
    }

    /// Every known feature for `server_id`, persisted rows first, defaults for the rest.
    async fn resolve_all(
        &self,
        server_id: ServerId,
    ) -> ServiceResult<Vec<WorkspaceFeatureStateDto>> {
        let mut persisted: HashMap<WorkspaceFeature, WorkspaceFeatureStateDto> = self
            .repos
            .workspace_feature_repo
            .list_for_server(server_id)
            .await?
            .into_iter()
            .map(|state| (state.feature, WorkspaceFeatureStateDto::from(state)))
            .collect();

        Ok(WorkspaceFeature::ALL
            .into_iter()
            .map(|feature| {
                persisted
                    .remove(&feature)
                    .unwrap_or_else(|| WorkspaceFeatureStateDto::default_for(server_id, feature))
            })
            .collect())
    }
}

#[async_trait]
impl WorkspaceFeatureService for DefaultWorkspaceFeatureService {
    async fn list(
        &self,
        query: ListWorkspaceFeaturesQuery,
    ) -> ServiceResult<Vec<WorkspaceFeatureStateDto>> {
        let is_member = self
            .repos
            .server_member_repo
            .is_member(query.server_id, query.actor.user_id)
            .await?;
        if !is_member {
            return Err(ServiceError::NotFound);
        }

        self.resolve_all(query.server_id).await
    }

    async fn set(&self, cmd: SetWorkspaceFeatureCmd) -> ServiceResult<WorkspaceFeatureStateDto> {
        let member = self
            .repos
            .server_member_repo
            .get_member(cmd.server_id, cmd.actor.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if member.member_role != ServerMemberRole::Owner {
            return Err(ServiceError::PermissionDenied);
        }

        let state = self
            .repos
            .workspace_feature_repo
            .set(
                cmd.server_id,
                cmd.feature,
                cmd.enabled,
                Some(cmd.actor.user_id),
            )
            .await?;
        self.audit_toggle(&cmd).await;
        Ok(WorkspaceFeatureStateDto::from(state))
    }

    async fn is_enabled(
        &self,
        server_id: ServerId,
        feature: WorkspaceFeature,
    ) -> ServiceResult<bool> {
        Ok(self
            .repos
            .workspace_feature_repo
            .is_enabled(server_id, feature)
            .await?)
    }

    async fn enabled_features(&self, server_id: ServerId) -> ServiceResult<Vec<WorkspaceFeature>> {
        Ok(self
            .resolve_all(server_id)
            .await?
            .into_iter()
            .filter(|state| state.enabled)
            .map(|state| state.feature)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rim_core::server_member::model::{ServerMember, ServerMemberId};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_core::workspace_feature::model::WorkspaceFeatureState;
    use rim_core::workspace_feature::repository::MockWorkspaceFeatureRepository;
    use rim_services::Actor;

    fn member(server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> ServerMember {
        ServerMember {
            id: ServerMemberId::new(),
            server_id,
            user_id,
            member_role: role,
            joined_at: Utc::now(),
        }
    }

    fn svc(
        feature_repo: MockWorkspaceFeatureRepository,
        member_repo: MockServerMemberRepository,
    ) -> DefaultWorkspaceFeatureService {
        DefaultWorkspaceFeatureService::new(WorkspaceFeatureRepos {
            workspace_feature_repo: Arc::new(feature_repo),
            server_member_repo: Arc::new(member_repo),
        })
    }

    fn set_cmd(actor: UserId, server_id: ServerId) -> SetWorkspaceFeatureCmd {
        SetWorkspaceFeatureCmd {
            actor: Actor::new(actor),
            server_id,
            feature: WorkspaceFeature::Incidents,
            enabled: true,
        }
    }

    #[tokio::test]
    async fn set_allows_owner_and_records_actor() {
        let server_id = ServerId::new();
        let actor = UserId::new();

        let mut members = MockServerMemberRepository::new();
        members
            .expect_get_member()
            .returning(move |_, _| Ok(Some(member(server_id, actor, ServerMemberRole::Owner))));

        let mut features = MockWorkspaceFeatureRepository::new();
        features
            .expect_set()
            .withf(move |s, f, enabled, by| {
                *s == server_id
                    && *f == WorkspaceFeature::Incidents
                    && *enabled
                    && *by == Some(actor)
            })
            .times(1)
            .returning(|server_id, feature, enabled, updated_by| {
                Box::pin(async move {
                    Ok(WorkspaceFeatureState {
                        server_id,
                        feature,
                        enabled,
                        updated_by,
                        updated_at: Utc::now(),
                    })
                })
            });

        let dto = svc(features, members)
            .set(set_cmd(actor, server_id))
            .await
            .unwrap();
        assert!(dto.enabled);
        assert_eq!(dto.updated_by, Some(actor));
        assert!(dto.updated_at.is_some());
    }

    #[tokio::test]
    async fn set_writes_audit_entry_with_feature_and_value() {
        use rim_services::audit::service::MockAuditLogService;

        let server_id = ServerId::new();
        let actor = UserId::new();

        let mut members = MockServerMemberRepository::new();
        members
            .expect_get_member()
            .returning(move |_, _| Ok(Some(member(server_id, actor, ServerMemberRole::Owner))));
        let mut features = MockWorkspaceFeatureRepository::new();
        features
            .expect_set()
            .returning(|server_id, feature, enabled, updated_by| {
                Box::pin(async move {
                    Ok(WorkspaceFeatureState {
                        server_id,
                        feature,
                        enabled,
                        updated_by,
                        updated_at: Utc::now(),
                    })
                })
            });
        let mut audit = MockAuditLogService::new();
        audit
            .expect_write_entry()
            .withf(move |e| {
                e.command_id == SET_FEATURE_AUDIT_ID
                    && e.actor_user_id == uuid::Uuid::from(actor)
                    && e.server_id == uuid::Uuid::from(server_id)
                    && e.args_redacted
                        == serde_json::json!({"feature": "incidents", "enabled": true})
                    && e.outcome == AuditOutcome::Success
            })
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));

        svc(features, members)
            .with_audit_log_service(Arc::new(audit))
            .set(set_cmd(actor, server_id))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn set_forbids_moderator() {
        let server_id = ServerId::new();
        let actor = UserId::new();

        let mut members = MockServerMemberRepository::new();
        members
            .expect_get_member()
            .returning(move |_, _| Ok(Some(member(server_id, actor, ServerMemberRole::Moderator))));
        let mut features = MockWorkspaceFeatureRepository::new();
        features.expect_set().never();

        let err = svc(features, members)
            .set(set_cmd(actor, server_id))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied), "got {err:?}");
    }

    #[tokio::test]
    async fn set_returns_not_found_for_non_member() {
        let mut members = MockServerMemberRepository::new();
        members.expect_get_member().returning(|_, _| Ok(None));
        let mut features = MockWorkspaceFeatureRepository::new();
        features.expect_set().never();

        let err = svc(features, members)
            .set(set_cmd(UserId::new(), ServerId::new()))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound), "got {err:?}");
    }

    #[tokio::test]
    async fn list_returns_not_found_for_non_member() {
        let mut members = MockServerMemberRepository::new();
        members.expect_is_member().returning(|_, _| Ok(false));

        let err = svc(MockWorkspaceFeatureRepository::new(), members)
            .list(ListWorkspaceFeaturesQuery {
                actor: Actor::new(UserId::new()),
                server_id: ServerId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound), "got {err:?}");
    }

    #[tokio::test]
    async fn list_fills_default_when_no_row_exists() {
        let server_id = ServerId::new();
        let mut members = MockServerMemberRepository::new();
        members.expect_is_member().returning(|_, _| Ok(true));
        let mut features = MockWorkspaceFeatureRepository::new();
        features
            .expect_list_for_server()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let states = svc(features, members)
            .list(ListWorkspaceFeaturesQuery {
                actor: Actor::new(UserId::new()),
                server_id,
            })
            .await
            .unwrap();

        assert_eq!(states.len(), WorkspaceFeature::ALL.len());
        let incidents = &states[0];
        assert_eq!(incidents.feature, WorkspaceFeature::Incidents);
        assert_eq!(
            incidents.enabled,
            WorkspaceFeature::Incidents.default_enabled()
        );
        assert!(!incidents.enabled, "incidents default to off");
        assert_eq!(incidents.updated_at, None);
        assert_eq!(incidents.updated_by, None);
    }

    #[tokio::test]
    async fn list_uses_persisted_row_over_default() {
        let server_id = ServerId::new();
        let mut members = MockServerMemberRepository::new();
        members.expect_is_member().returning(|_, _| Ok(true));
        let mut features = MockWorkspaceFeatureRepository::new();
        features.expect_list_for_server().returning(move |_| {
            Box::pin(async move {
                Ok(vec![WorkspaceFeatureState {
                    server_id,
                    feature: WorkspaceFeature::Incidents,
                    enabled: true,
                    updated_by: None,
                    updated_at: Utc::now(),
                }])
            })
        });

        let states = svc(features, members)
            .list(ListWorkspaceFeaturesQuery {
                actor: Actor::new(UserId::new()),
                server_id,
            })
            .await
            .unwrap();
        assert!(states[0].enabled);
        assert!(states[0].updated_at.is_some());
    }

    #[tokio::test]
    async fn enabled_features_resolves_defaults_in_one_call() {
        let mut features = MockWorkspaceFeatureRepository::new();
        features
            .expect_list_for_server()
            .times(1)
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let enabled = svc(features, MockServerMemberRepository::new())
            .enabled_features(ServerId::new())
            .await
            .unwrap();
        assert!(
            enabled.is_empty(),
            "no row and default off → nothing enabled"
        );
    }
}
