//! The DM rules of workspaces and each person's choice of who may start a DM with them.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;

use rim_core::dm_policy::model::{MemberDm, OutsideDm, OutsideFloor, WorkspaceDmPolicy};
use rim_core::dm_policy::repository::DmPolicyRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_services::dm_policy::dto::{
    DmFloorSourceDto, DmOperatorPolicy, MyDmSettingsDto, WorkspaceDmPolicyDto,
    WorkspaceDmSettingDto,
};
use rim_services::dm_policy::service::DmPolicyService;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::relay::policy::closing_floor;

pub struct DefaultDmPolicyService {
    dm_policy_repo: Arc<dyn DmPolicyRepository>,
    server_member_repo: Arc<dyn ServerMemberRepository>,
    operator: DmOperatorPolicy,
}

impl DefaultDmPolicyService {
    pub fn new(
        dm_policy_repo: Arc<dyn DmPolicyRepository>,
        server_member_repo: Arc<dyn ServerMemberRepository>,
        operator: DmOperatorPolicy,
    ) -> Self {
        Self {
            dm_policy_repo,
            server_member_repo,
            operator,
        }
    }
}

fn policy_dto(policy: WorkspaceDmPolicy) -> WorkspaceDmPolicyDto {
    WorkspaceDmPolicyDto {
        workspace_id: policy.server_id,
        outside_floor: policy.outside_floor,
        member_default: policy.member_default,
    }
}

/// What a floor refusing DMs from outside is called in a refusal.
fn floor_name(source: &DmFloorSourceDto) -> String {
    match source {
        DmFloorSourceDto::Operator => "this server".to_string(),
        DmFloorSourceDto::Workspace { name, .. } => format!("the workspace {name}"),
    }
}

#[async_trait]
impl DmPolicyService for DefaultDmPolicyService {
    async fn workspace_dm_policy(
        &self,
        actor: Actor,
        workspace_id: ServerId,
    ) -> ServiceResult<WorkspaceDmPolicyDto> {
        if !self
            .server_member_repo
            .is_member(workspace_id, actor.user_id)
            .await?
        {
            return Err(ServiceError::NotFound);
        }
        self.dm_policy_repo
            .workspace_policy(workspace_id)
            .await?
            .map(policy_dto)
            .ok_or(ServiceError::NotFound)
    }

    async fn set_workspace_dm_policy(
        &self,
        actor: Actor,
        workspace_id: ServerId,
        outside_floor: OutsideFloor,
        member_default: MemberDm,
    ) -> ServiceResult<WorkspaceDmPolicyDto> {
        match self
            .server_member_repo
            .get_member(workspace_id, actor.user_id)
            .await?
        {
            Some(member) if member.member_role == ServerMemberRole::Owner => {}
            _ => return Err(ServiceError::PermissionDenied),
        }
        let policy = self
            .dm_policy_repo
            .set_workspace_policy(workspace_id, outside_floor, member_default)
            .await?
            .ok_or(ServiceError::NotFound)?;
        info!(
            target: "audit",
            event = "workspace.dm_policy_set",
            actor_id = %actor.user_id,
            workspace_id = %workspace_id,
            outside_floor = outside_floor.as_str(),
            member_default = member_default.as_str(),
            "a workspace owner set the workspace's DM rules"
        );
        Ok(policy_dto(policy))
    }

    async fn my_dm_settings(&self, actor: Actor) -> ServiceResult<MyDmSettingsDto> {
        let outside = self.dm_policy_repo.outside(actor.user_id).await?;
        let workspaces = self.dm_policy_repo.user_workspaces(actor.user_id).await?;
        let floor = closing_floor(&self.operator, &workspaces);
        Ok(MyDmSettingsDto {
            outside,
            workspaces: workspaces
                .iter()
                .map(|w| WorkspaceDmSettingDto {
                    workspace_id: w.server_id,
                    members: w.members(),
                    member_default: w.member_default,
                })
                .collect(),
            effective_outside: if floor.is_some() {
                OutsideDm::None
            } else {
                outside
            },
            outside_floor_from: floor,
        })
    }

    async fn set_my_dm_settings(
        &self,
        actor: Actor,
        outside: OutsideDm,
    ) -> ServiceResult<MyDmSettingsDto> {
        if outside == OutsideDm::Requests {
            let workspaces = self.dm_policy_repo.user_workspaces(actor.user_id).await?;
            if let Some(floor) = closing_floor(&self.operator, &workspaces) {
                return Err(ServiceError::Validation(format!(
                    "{} takes no DM requests from outside your workspaces",
                    floor_name(&floor)
                )));
            }
        }
        self.dm_policy_repo
            .set_outside(actor.user_id, outside)
            .await?;
        self.my_dm_settings(actor).await
    }

    async fn set_my_workspace_dm_setting(
        &self,
        actor: Actor,
        workspace_id: ServerId,
        members: MemberDm,
    ) -> ServiceResult<MyDmSettingsDto> {
        let workspaces = self.dm_policy_repo.user_workspaces(actor.user_id).await?;
        if !workspaces.iter().any(|w| w.server_id == workspace_id) {
            return Err(ServiceError::NotFound);
        }
        self.dm_policy_repo
            .set_member_choice(actor.user_id, workspace_id, members)
            .await?;
        self.my_dm_settings(actor).await
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use rim_core::dm_policy::model::{DmInbound, UserWorkspaceDm};
    use rim_core::dm_policy::repository::MockDmPolicyRepository;
    use rim_core::server_member::model::{ServerMember, ServerMemberId};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;

    use super::*;

    fn workspace(id: ServerId, floor: OutsideFloor, choice: Option<MemberDm>) -> UserWorkspaceDm {
        UserWorkspaceDm {
            server_id: id,
            name: "Acme".to_string(),
            outside_floor: floor,
            member_default: MemberDm::Direct,
            choice,
        }
    }

    /// A repository where the user chose `outside` and is a member of `workspaces`.
    fn repo(outside: OutsideDm, workspaces: Vec<UserWorkspaceDm>) -> MockDmPolicyRepository {
        let mut repo = MockDmPolicyRepository::new();
        repo.expect_outside()
            .returning(move |_| Box::pin(async move { Ok(outside) }));
        repo.expect_user_workspaces().returning(move |_| {
            let workspaces = workspaces.clone();
            Box::pin(async move { Ok(workspaces) })
        });
        repo
    }

    fn member_with(role: Option<ServerMemberRole>) -> MockServerMemberRepository {
        let mut members = MockServerMemberRepository::new();
        members
            .expect_get_member()
            .returning(move |server_id, user_id| {
                Ok(role.map(|member_role| ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: Utc::now(),
                }))
            });
        members
    }

    fn service(
        repo: MockDmPolicyRepository,
        members: MockServerMemberRepository,
        inbound: DmInbound,
    ) -> DefaultDmPolicyService {
        DefaultDmPolicyService::new(
            Arc::new(repo),
            Arc::new(members),
            DmOperatorPolicy {
                inbound,
                ..Default::default()
            },
        )
    }

    fn actor() -> Actor {
        Actor::new(UserId::new())
    }

    #[tokio::test]
    async fn settings_show_the_choice_the_default_and_the_floor_that_closes() {
        let open = ServerId::new();
        let closed = ServerId::new();
        let svc = service(
            repo(
                OutsideDm::Requests,
                vec![
                    workspace(open, OutsideFloor::Requests, Some(MemberDm::Requests)),
                    workspace(closed, OutsideFloor::Closed, None),
                ],
            ),
            MockServerMemberRepository::new(),
            DmInbound::Requests,
        );
        let settings = svc.my_dm_settings(actor()).await.expect("settings");
        assert_eq!(settings.outside, OutsideDm::Requests);
        assert_eq!(settings.effective_outside, OutsideDm::None);
        assert_eq!(
            settings.outside_floor_from,
            Some(DmFloorSourceDto::Workspace {
                workspace_id: closed,
                name: "Acme".to_string(),
            })
        );
        assert_eq!(settings.workspaces[0].members, MemberDm::Requests);
        assert_eq!(settings.workspaces[0].member_default, MemberDm::Direct);
        assert_eq!(settings.workspaces[1].members, MemberDm::Direct);
    }

    #[tokio::test]
    async fn requests_below_a_closing_floor_are_refused_naming_it() {
        let svc = service(
            repo(OutsideDm::None, Vec::new()),
            MockServerMemberRepository::new(),
            DmInbound::None,
        );
        let err = svc
            .set_my_dm_settings(actor(), OutsideDm::Requests)
            .await
            .expect_err("below the floor");
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.starts_with("this server")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn none_is_always_allowed_and_stored() {
        let mut repo = repo(OutsideDm::None, Vec::new());
        repo.expect_set_outside()
            .withf(|_, outside| *outside == OutsideDm::None)
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));
        let svc = service(repo, MockServerMemberRepository::new(), DmInbound::None);
        let settings = svc
            .set_my_dm_settings(actor(), OutsideDm::None)
            .await
            .expect("stored");
        assert_eq!(
            settings.outside_floor_from,
            Some(DmFloorSourceDto::Operator)
        );
    }

    #[tokio::test]
    async fn a_workspace_choice_needs_membership() {
        let mine = ServerId::new();
        let mut repo = repo(
            OutsideDm::Requests,
            vec![workspace(mine, OutsideFloor::None, None)],
        );
        repo.expect_set_member_choice()
            .withf(move |_, server_id, members| {
                *server_id == mine && *members == MemberDm::Requests
            })
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(()) }));
        let svc = service(repo, MockServerMemberRepository::new(), DmInbound::Requests);
        svc.set_my_workspace_dm_setting(actor(), mine, MemberDm::Requests)
            .await
            .expect("member");
        let err = svc
            .set_my_workspace_dm_setting(actor(), ServerId::new(), MemberDm::Requests)
            .await
            .expect_err("not a member");
        assert!(matches!(err, ServiceError::NotFound), "{err:?}");
    }

    #[tokio::test]
    async fn only_the_owner_sets_a_workspaces_rules() {
        for role in [
            None,
            Some(ServerMemberRole::Moderator),
            Some(ServerMemberRole::Member),
        ] {
            let svc = service(
                MockDmPolicyRepository::new(),
                member_with(role),
                DmInbound::Requests,
            );
            let err = svc
                .set_workspace_dm_policy(
                    actor(),
                    ServerId::new(),
                    OutsideFloor::Closed,
                    MemberDm::Requests,
                )
                .await
                .expect_err("not the owner");
            assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");
        }
        let mut repo = MockDmPolicyRepository::new();
        repo.expect_set_workspace_policy()
            .returning(|server_id, outside_floor, member_default| {
                Box::pin(async move {
                    Ok(Some(WorkspaceDmPolicy {
                        server_id,
                        outside_floor,
                        member_default,
                    }))
                })
            });
        let svc = service(
            repo,
            member_with(Some(ServerMemberRole::Owner)),
            DmInbound::Requests,
        );
        let set = svc
            .set_workspace_dm_policy(
                actor(),
                ServerId::new(),
                OutsideFloor::Closed,
                MemberDm::Requests,
            )
            .await
            .expect("owner");
        assert_eq!(set.outside_floor, OutsideFloor::Closed);
        assert_eq!(set.member_default, MemberDm::Requests);
    }

    #[tokio::test]
    async fn a_workspaces_rules_are_shown_to_its_members_only() {
        let mut members = MockServerMemberRepository::new();
        members
            .expect_is_member()
            .returning(|_, user_id| Ok(user_id == UserId::from(uuid::Uuid::nil())));
        let mut repo = MockDmPolicyRepository::new();
        repo.expect_workspace_policy().returning(|server_id| {
            Box::pin(async move {
                Ok(Some(WorkspaceDmPolicy {
                    server_id,
                    outside_floor: OutsideFloor::Requests,
                    member_default: MemberDm::Direct,
                }))
            })
        });
        let svc = service(repo, members, DmInbound::Requests);
        let shown = svc
            .workspace_dm_policy(Actor::new(UserId::from(uuid::Uuid::nil())), ServerId::new())
            .await
            .expect("member");
        assert_eq!(shown.outside_floor, OutsideFloor::Requests);
        let err = svc
            .workspace_dm_policy(actor(), ServerId::new())
            .await
            .expect_err("not a member");
        assert!(matches!(err, ServiceError::NotFound), "{err:?}");
    }
}
