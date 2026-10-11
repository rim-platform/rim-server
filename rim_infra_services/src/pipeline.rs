use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use dashmap::DashMap;

use rim_core::pipeline::model::{
    FilterCondition, Pipeline, PipelineAction, PipelineFilter, PipelineId, PipelineTrigger,
    PipelineTriggerKind,
};
use rim_core::pipeline::repository::PipelineRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_services::command::dto::CommandContextDto;
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::ExecuteCommandCmd;
use rim_services::command::service::CommandService;
use rim_services::events::{Broadcaster, WorkspaceEvent};
use rim_services::pipeline::dto::{PipelineDto, PipelineFiredResult, PipelineTriggerDto};
use rim_services::pipeline::request::{
    CreatePipelineCmd, DeletePipelineCmd, ListPipelinesCmd, TogglePipelineCmd, UpdatePipelineCmd,
};
use rim_services::pipeline::service::PipelineService;
use rim_services::{Actor, ServiceError, ServiceResult};

const MAX_PIPELINES_PER_USER_PER_SERVER: u64 = 50;
const RATE_LIMIT_WINDOW_SECS: u64 = 60;
const RATE_LIMIT_MAX_FIRINGS: u32 = 10;

#[derive(Clone)]
pub struct PipelineRepos {
    pub pipeline_repo: Arc<dyn PipelineRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

#[derive(Clone)]
pub struct DefaultPipelineService {
    repos: PipelineRepos,
    command_service: Arc<dyn CommandService>,
    broadcaster: Arc<dyn Broadcaster>,
    cooldowns: Arc<DashMap<PipelineId, Instant>>,
    rate_limits: Arc<DashMap<PipelineId, (u32, Instant)>>,
}

impl DefaultPipelineService {
    pub fn new(
        repos: PipelineRepos,
        command_service: Arc<dyn CommandService>,
        broadcaster: Arc<dyn Broadcaster>,
    ) -> Self {
        Self {
            repos,
            command_service,
            broadcaster,
            cooldowns: Arc::new(DashMap::new()),
            rate_limits: Arc::new(DashMap::new()),
        }
    }

    async fn ensure_member(&self, server_id: ServerId, user_id: UserId) -> ServiceResult<()> {
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            server_id,
            user_id,
        )
        .await
    }

    /// The pipeline's creator, or an Owner or Moderator of its workspace, may change it; any
    /// other member or a non-member gets `PermissionDenied`.
    async fn ensure_can_manage(&self, pipeline: &Pipeline, user_id: UserId) -> ServiceResult<()> {
        let standing = crate::util::member_standing(
            self.repos.server_member_repo.as_ref(),
            pipeline.server_id,
            user_id,
        )
        .await?;
        if pipeline.created_by == user_id || standing.role.is_moderator() {
            Ok(())
        } else {
            Err(ServiceError::PermissionDenied)
        }
    }

    fn validate_name(name: &str) -> ServiceResult<()> {
        if name.is_empty() {
            return Err(ServiceError::Validation(
                "pipeline name cannot be empty".into(),
            ));
        }
        if name.len() > 256 {
            return Err(ServiceError::Validation(
                "pipeline name exceeds maximum length of 256 characters".into(),
            ));
        }
        Ok(())
    }

    fn validate_cooldown(cooldown_secs: Option<i32>) -> ServiceResult<()> {
        if let Some(secs) = cooldown_secs
            && secs < 0
        {
            return Err(ServiceError::Validation(
                "cooldown_secs cannot be negative".into(),
            ));
        }
        Ok(())
    }

    fn validate_trigger(trigger: &PipelineTriggerDto) -> ServiceResult<()> {
        match trigger.kind {
            rim_services::pipeline::dto::PipelineTriggerKindDto::MemberJoined
            | rim_services::pipeline::dto::PipelineTriggerKindDto::WorkspaceStateChanged
                if trigger.conversation_id.is_some() =>
            {
                return Err(ServiceError::Validation(
                    "MemberJoined and WorkspaceStateChanged triggers cannot have a conversation_id"
                        .into(),
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn matches_trigger(pipeline: &Pipeline, event: &WorkspaceEvent) -> bool {
        let kind_matches = matches!(
            (&pipeline.trigger.kind, event),
            (
                PipelineTriggerKind::NewMessage,
                WorkspaceEvent::NewMessage { .. }
            ) | (
                PipelineTriggerKind::MessageUpdated,
                WorkspaceEvent::MessageUpdated { .. }
            ) | (
                PipelineTriggerKind::MessageDeleted,
                WorkspaceEvent::MessageDeleted { .. }
            ) | (
                PipelineTriggerKind::ReactionsUpdated,
                WorkspaceEvent::ReactionsUpdated { .. }
            ) | (
                PipelineTriggerKind::MemberJoined,
                WorkspaceEvent::MemberJoined { .. }
            ) | (
                PipelineTriggerKind::WorkspaceStateChanged,
                WorkspaceEvent::WorkspaceStateChanged { .. }
            )
        );

        if !kind_matches {
            return false;
        }

        // If trigger has a conversation_id filter, match against event's conversation_id
        if let Some(trigger_conv_id) = pipeline.trigger.conversation_id {
            let event_conv_id = event_conversation_id(event);
            return event_conv_id == Some(trigger_conv_id);
        }

        true
    }

    fn apply_filter(pipeline: &Pipeline, event: &WorkspaceEvent) -> bool {
        let filter = match &pipeline.filter {
            None => return true,
            Some(f) => f,
        };

        for condition in &filter.conditions {
            match condition {
                FilterCondition::MessageContains { pattern } => {
                    // The server cannot read an encrypted message's text.
                    if event_message_is_encrypted(event) {
                        return false;
                    }
                    let text = event_message_text(event);
                    if !text.as_deref().unwrap_or("").contains(pattern.as_str()) {
                        return false;
                    }
                }
                FilterCondition::MessageFromUser { user_id } => {
                    let author = event_author_id(event);
                    if author != Some(*user_id) {
                        return false;
                    }
                }
                FilterCondition::ConversationIs { conversation_id } => {
                    let conv = event_conversation_id(event);
                    if conv != Some(*conversation_id) {
                        return false;
                    }
                }
            }
        }

        true
    }

    fn check_cooldown(&self, pipeline: &Pipeline) -> bool {
        if let Some(cooldown) = pipeline.cooldown_secs
            && let Some(last) = self.cooldowns.get(&pipeline.id)
        {
            let elapsed = last.elapsed().as_secs();
            if elapsed < cooldown as u64 {
                return false;
            }
        }
        true
    }

    /// Returns false and triggers circuit breaker if rate limit exceeded.
    fn check_rate_limit(&self, pipeline: &Pipeline) -> bool {
        let mut entry = self
            .rate_limits
            .entry(pipeline.id)
            .or_insert((0, Instant::now()));
        let (count, window_start) = entry.value_mut();
        let elapsed = window_start.elapsed().as_secs();

        if elapsed > RATE_LIMIT_WINDOW_SECS {
            // Reset window
            *count = 1;
            *window_start = Instant::now();
            true
        } else {
            *count += 1;
            *count <= RATE_LIMIT_MAX_FIRINGS
        }
    }
}

fn event_conversation_id(
    event: &WorkspaceEvent,
) -> Option<rim_core::conversation::model::ConversationId> {
    match event {
        WorkspaceEvent::NewMessage {
            conversation_id, ..
        }
        | WorkspaceEvent::MessageUpdated {
            conversation_id, ..
        }
        | WorkspaceEvent::MessageDeleted {
            conversation_id, ..
        }
        | WorkspaceEvent::ReactionsUpdated {
            conversation_id, ..
        } => Some(*conversation_id),
        _ => None,
    }
}

fn event_author_id(event: &WorkspaceEvent) -> Option<UserId> {
    match event {
        WorkspaceEvent::NewMessage { message, .. }
        | WorkspaceEvent::MessageUpdated { message, .. } => Some(message.author_id),
        _ => None,
    }
}

fn event_message_is_encrypted(event: &WorkspaceEvent) -> bool {
    match event {
        WorkspaceEvent::NewMessage { message, .. }
        | WorkspaceEvent::MessageUpdated { message, .. } => message.encryption.is_some(),
        _ => false,
    }
}

fn event_message_text(event: &WorkspaceEvent) -> Option<String> {
    match event {
        WorkspaceEvent::NewMessage { message, .. }
        | WorkspaceEvent::MessageUpdated { message, .. } => message.text.clone(),
        _ => None,
    }
}

#[async_trait]
impl PipelineService for DefaultPipelineService {
    async fn create(&self, cmd: CreatePipelineCmd) -> ServiceResult<PipelineDto> {
        self.ensure_member(cmd.server_id, cmd.actor.user_id).await?;
        Self::validate_name(&cmd.name)?;
        Self::validate_trigger(&cmd.trigger)?;
        Self::validate_cooldown(cmd.cooldown_secs)?;

        // Quota check
        let count = self
            .repos
            .pipeline_repo
            .count_for_user(cmd.server_id, cmd.actor.user_id)
            .await?;

        if count >= MAX_PIPELINES_PER_USER_PER_SERVER {
            return Err(ServiceError::Validation(format!(
                "quota exceeded: maximum {} pipelines per user per server",
                MAX_PIPELINES_PER_USER_PER_SERVER
            )));
        }

        // Convert DTOs to domain types via From impls
        let trigger: PipelineTrigger = cmd.trigger.into();
        let filter = cmd.filter.map(PipelineFilter::from);
        let actions: Vec<PipelineAction> = cmd.actions.into_iter().map(Into::into).collect();

        let now = chrono::Utc::now();
        let pipeline = Pipeline {
            id: PipelineId::new(),
            server_id: cmd.server_id,
            name: cmd.name,
            description: cmd.description,
            trigger,
            filter,
            actions,
            enabled: true,
            cooldown_secs: cmd.cooldown_secs,
            created_by: cmd.actor.user_id,
            schema_version: 1,
            last_fired_at: None,
            created_at: now,
            updated_at: now,
        };

        let stored = self.repos.pipeline_repo.create(&pipeline).await?;

        Ok(stored.into())
    }

    async fn list(&self, cmd: ListPipelinesCmd) -> ServiceResult<Vec<PipelineDto>> {
        self.ensure_member(cmd.server_id, cmd.actor.user_id).await?;

        let pipelines = match cmd.enabled {
            Some(true) => {
                self.repos
                    .pipeline_repo
                    .list_enabled_for_server(cmd.server_id)
                    .await?
            }
            Some(false) => {
                // List all, then filter disabled
                let all = self
                    .repos
                    .pipeline_repo
                    .list_for_server(cmd.server_id)
                    .await?;
                all.into_iter().filter(|p| !p.enabled).collect()
            }
            None => {
                self.repos
                    .pipeline_repo
                    .list_for_server(cmd.server_id)
                    .await?
            }
        };

        Ok(pipelines.into_iter().map(Into::into).collect())
    }

    async fn update(&self, cmd: UpdatePipelineCmd) -> ServiceResult<PipelineDto> {
        let existing = self
            .repos
            .pipeline_repo
            .get_by_id(cmd.pipeline_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        self.ensure_can_manage(&existing, cmd.actor.user_id).await?;
        Self::validate_name(&cmd.name)?;
        Self::validate_trigger(&cmd.trigger)?;
        Self::validate_cooldown(cmd.cooldown_secs)?;

        let trigger: PipelineTrigger = cmd.trigger.into();
        let filter = cmd.filter.map(PipelineFilter::from);
        let actions: Vec<PipelineAction> = cmd.actions.into_iter().map(Into::into).collect();

        let updated = Pipeline {
            id: existing.id,
            server_id: existing.server_id,
            name: cmd.name,
            description: cmd.description,
            trigger,
            filter,
            actions,
            enabled: existing.enabled,
            cooldown_secs: cmd.cooldown_secs,
            created_by: existing.created_by,
            schema_version: existing.schema_version,
            last_fired_at: existing.last_fired_at,
            created_at: existing.created_at,
            updated_at: chrono::Utc::now(),
        };

        let stored = self.repos.pipeline_repo.update(&updated).await?;

        Ok(stored.into())
    }

    async fn delete(&self, cmd: DeletePipelineCmd) -> ServiceResult<()> {
        let existing = self
            .repos
            .pipeline_repo
            .get_by_id(cmd.pipeline_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        self.ensure_can_manage(&existing, cmd.actor.user_id).await?;

        self.repos.pipeline_repo.delete(cmd.pipeline_id).await?;

        // Evict in-memory tracking entries to prevent memory leaks
        self.cooldowns.remove(&cmd.pipeline_id);
        self.rate_limits.remove(&cmd.pipeline_id);

        Ok(())
    }

    async fn toggle(&self, cmd: TogglePipelineCmd) -> ServiceResult<PipelineDto> {
        let existing = self
            .repos
            .pipeline_repo
            .get_by_id(cmd.pipeline_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        self.ensure_can_manage(&existing, cmd.actor.user_id).await?;

        let stored = self
            .repos
            .pipeline_repo
            .set_enabled(cmd.pipeline_id, cmd.enabled)
            .await?;

        Ok(stored.into())
    }

    async fn evaluate_event(
        &self,
        event: WorkspaceEvent,
    ) -> ServiceResult<Vec<PipelineFiredResult>> {
        // Extract server_id from event
        let server_id_str = event.workspace_id();
        let server_id: ServerId = server_id_str
            .parse::<uuid::Uuid>()
            .map(Into::into)
            .map_err(|e| ServiceError::internal(anyhow::anyhow!("invalid server_id: {e}")))?;

        let pipelines = self
            .repos
            .pipeline_repo
            .list_enabled_for_server(server_id)
            .await?;

        let mut results = Vec::new();

        for pipeline in &pipelines {
            if !Self::matches_trigger(pipeline, &event) {
                continue;
            }

            if !Self::apply_filter(pipeline, &event) {
                results.push(PipelineFiredResult {
                    pipeline_id: pipeline.id.to_string(),
                    pipeline_name: pipeline.name.clone(),
                    actions_executed: 0,
                    skipped_reason: Some("filter did not match".into()),
                });
                continue;
            }

            if !self.check_cooldown(pipeline) {
                results.push(PipelineFiredResult {
                    pipeline_id: pipeline.id.to_string(),
                    pipeline_name: pipeline.name.clone(),
                    actions_executed: 0,
                    skipped_reason: Some("cooldown active".into()),
                });
                continue;
            }

            // Check rate limit (circuit breaker)
            if !self.check_rate_limit(pipeline) {
                tracing::warn!(
                    pipeline_id = %pipeline.id,
                    pipeline_name = %pipeline.name,
                    "pipeline exceeded rate limit, suspending"
                );

                // Suspend the pipeline — log failure but don't abort evaluation
                if let Err(e) = self
                    .repos
                    .pipeline_repo
                    .set_enabled(pipeline.id, false)
                    .await
                {
                    tracing::error!(
                        pipeline_id = %pipeline.id,
                        "CRITICAL: failed to suspend rate-limited pipeline: {e}"
                    );
                }

                self.broadcaster
                    .broadcast(WorkspaceEvent::PipelineSuspended {
                        workspace_id: server_id,
                        pipeline_id: pipeline.id,
                        pipeline_name: pipeline.name.clone(),
                        reason: format!(
                            "exceeded {} firings per {} seconds",
                            RATE_LIMIT_MAX_FIRINGS, RATE_LIMIT_WINDOW_SECS
                        ),
                    });

                results.push(PipelineFiredResult {
                    pipeline_id: pipeline.id.to_string(),
                    pipeline_name: pipeline.name.clone(),
                    actions_executed: 0,
                    skipped_reason: Some("rate limit exceeded — pipeline suspended".into()),
                });
                continue;
            }

            // Record cooldown
            self.cooldowns.insert(pipeline.id, Instant::now());

            // Execute actions
            let actor = Actor::new(pipeline.created_by);
            let conv_id = event_conversation_id(&event);
            let context = Some(CommandContextDto {
                workspace_id: Some(server_id),
                conversation_id: conv_id,
                thread_id: None,
                selected_text: None,
                selected_message_id: None,
                selected_message_ids: vec![],
                focused_surface: None,
                active_panel: None,
            });

            let mut actions_executed = 0;
            for action in &pipeline.actions {
                match action {
                    PipelineAction::ExecuteCommand { command_id, args } => {
                        let result = self
                            .command_service
                            .execute_command(ExecuteCommandCmd {
                                actor,
                                command_id: command_id.clone(),
                                args: args.clone(),
                                context: context.clone(),
                                // Pipelines never auto-confirm. An action
                                // that needs confirmation gets
                                // `ConfirmationRequired` and is skipped with
                                // a warning below: pipelines run server-side
                                // without a user present to confirm.
                                confirmation: None,
                                is_pipe: false,
                                // Pipelines fire in response to backend
                                // events the user did not explicitly
                                // initiate (e.g. webhook trigger). Tagged
                                // `Plugin` because pipelines are config-
                                // driven non-human automation: `is_agentic()`
                                // holds, so a Mutating command needs a
                                // confirmation proof just as for an agent.
                                initiator: CommandInitiator::Plugin,
                            })
                            .await;

                        match result {
                            Ok(res) => {
                                use rim_services::command::dto::CommandResultStatusDto;
                                match res.status {
                                    CommandResultStatusDto::Success => {
                                        actions_executed += 1;
                                    }
                                    CommandResultStatusDto::ConfirmationRequired => {
                                        // The action needs confirmation.
                                        // Server-side pipelines have no user
                                        // present to confirm, so the issued
                                        // challenge is unusable; the unspent
                                        // token expires on its TTL.
                                        tracing::warn!(
                                            pipeline_id = %pipeline.id,
                                            command_id = %command_id,
                                            "pipeline action targets a dangerous command but server-side pipelines cannot satisfy the confirmation gate; action skipped",
                                        );
                                    }
                                    CommandResultStatusDto::Error => {
                                        tracing::warn!(
                                            pipeline_id = %pipeline.id,
                                            command_id = %command_id,
                                            message = %res.message,
                                            "pipeline action returned error status",
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    pipeline_id = %pipeline.id,
                                    command_id = %command_id,
                                    "pipeline action failed: {e}"
                                );
                            }
                        }
                    }
                }
            }

            self.broadcaster.broadcast(WorkspaceEvent::PipelineFired {
                workspace_id: server_id,
                pipeline_id: pipeline.id,
                pipeline_name: pipeline.name.clone(),
                trigger_event: format!("{:?}", pipeline.trigger.kind),
                actions_executed,
            });

            // Persist last_fired_at so it survives restarts and is visible in queries
            if let Err(e) = self
                .repos
                .pipeline_repo
                .set_last_fired_at(pipeline.id, chrono::Utc::now())
                .await
            {
                tracing::error!(
                    pipeline_id = %pipeline.id,
                    "failed to update last_fired_at: {e}"
                );
            }

            results.push(PipelineFiredResult {
                pipeline_id: pipeline.id.to_string(),
                pipeline_name: pipeline.name.clone(),
                actions_executed,
                skipped_reason: None,
            });
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::pipeline::model::{PipelineAction, PipelineTrigger, PipelineTriggerKind};
    use rim_core::repository::error::RepoResult;
    use rim_core::server_member::model::{MemberStanding, ServerMember, ServerMemberRole};
    use rim_core::server_member::repository::ServerMemberRepository;
    use rim_services::Actor;
    use rim_services::events::MessageEventData;
    use rim_services::pipeline::dto::{
        PipelineActionDto, PipelineTriggerDto, PipelineTriggerKindDto,
    };

    mockall::mock! {
        PipelineRepository {}
        #[async_trait::async_trait]
        impl PipelineRepository for PipelineRepository {
            async fn create(&self, pipeline: &Pipeline) -> RepoResult<Pipeline>;
            async fn get_by_id(&self, id: PipelineId) -> RepoResult<Option<Pipeline>>;
            async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<Pipeline>>;
            async fn list_enabled_for_server(&self, server_id: ServerId) -> RepoResult<Vec<Pipeline>>;
            async fn update(&self, pipeline: &Pipeline) -> RepoResult<Pipeline>;
            async fn delete(&self, id: PipelineId) -> RepoResult<()>;
            async fn count_for_user(&self, server_id: ServerId, user_id: UserId) -> RepoResult<u64>;
            async fn set_enabled(&self, id: PipelineId, enabled: bool) -> RepoResult<Pipeline>;
            async fn set_last_fired_at(&self, id: PipelineId, fired_at: chrono::DateTime<chrono::Utc>) -> RepoResult<()>;
        }
    }

    mockall::mock! {
        ServerMemberRepository {}
        #[async_trait::async_trait]
        impl ServerMemberRepository for ServerMemberRepository {
            async fn get_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<Option<ServerMember>>;
            async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<ServerMember>;
            async fn change_member_role(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<()>;
            async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;
            async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>>;
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn join_direct_space(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn get_member_standing(&self, server_id: rim_core::server::model::ServerId, user_id: rim_core::user::model::UserId) -> RepoResult<Option<rim_core::server_member::model::MemberStanding>>;
            async fn get_owner_id(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<Option<rim_core::user::model::UserId>>;
        }
    }

    fn make_member_repo() -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        repo.expect_is_member().returning(|_, _| Ok(true));
        repo
    }

    fn make_non_member_repo() -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        repo.expect_is_member().returning(|_, _| Ok(false));
        repo.expect_join_direct_space().returning(|_, _| Ok(false));
        repo
    }

    fn make_service(
        pipeline_repo: MockPipelineRepository,
        member_repo: MockServerMemberRepository,
    ) -> DefaultPipelineService {
        use rim_services::command::service::MockCommandService;
        DefaultPipelineService::new(
            PipelineRepos {
                pipeline_repo: Arc::new(pipeline_repo),
                server_member_repo: Arc::new(member_repo),
            },
            Arc::new(MockCommandService::new()),
            Arc::new(rim_services::events::NoopBroadcaster),
        )
    }

    fn make_pipeline(server_id: ServerId, user_id: UserId) -> Pipeline {
        Pipeline {
            id: PipelineId::new(),
            server_id,
            name: "Test Pipeline".to_string(),
            description: None,
            trigger: PipelineTrigger {
                kind: PipelineTriggerKind::NewMessage,
                conversation_id: None,
            },
            filter: None,
            actions: vec![PipelineAction::ExecuteCommand {
                command_id: "test.cmd".to_string(),
                args: None,
            }],
            enabled: true,
            cooldown_secs: None,
            created_by: user_id,
            schema_version: 1,
            last_fired_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn make_new_message_event(
        server_id: ServerId,
        conv_id: rim_core::conversation::model::ConversationId,
        author_id: UserId,
    ) -> WorkspaceEvent {
        WorkspaceEvent::NewMessage {
            workspace_id: server_id,
            conversation_id: conv_id,
            conversation_type: "Channel".to_string(),
            member_user_ids: vec![],
            is_private: false,
            message: MessageEventData {
                id: rim_core::message::model::MessageId::new(),
                server_id,
                conversation_id: conv_id,
                thread_id: None,
                root_thread_id: None,
                author_id,
                text: Some("hello world".to_string()),
                has_attachments: false,
                parent_message_id: None,
                forwarded_from_message_id: None,
                is_deleted: false,
                sent_at: chrono::Utc::now(),
                encryption: None,
                reactions: vec![],
            },
        }
    }

    #[tokio::test]
    async fn create_pipeline_ok() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockPipelineRepository::new();
        repo.expect_count_for_user().returning(|_, _| Ok(0));
        repo.expect_create().returning(|p| Ok(p.clone()));

        let svc = make_service(repo, make_member_repo());
        let result = svc
            .create(CreatePipelineCmd {
                actor,
                server_id,
                name: "My Pipeline".to_string(),
                description: None,
                trigger: PipelineTriggerDto {
                    kind: PipelineTriggerKindDto::NewMessage,
                    conversation_id: None,
                },
                filter: None,
                actions: vec![PipelineActionDto::ExecuteCommand {
                    command_id: "test.cmd".to_string(),
                    args: None,
                }],
                cooldown_secs: None,
            })
            .await
            .unwrap();

        assert_eq!(result.name, "My Pipeline");
        assert!(result.enabled);
    }

    #[tokio::test]
    async fn create_rejects_non_member() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let repo = MockPipelineRepository::new();
        let svc = make_service(repo, make_non_member_repo());

        let err = svc
            .create(CreatePipelineCmd {
                actor,
                server_id,
                name: "My Pipeline".to_string(),
                description: None,
                trigger: PipelineTriggerDto {
                    kind: PipelineTriggerKindDto::NewMessage,
                    conversation_id: None,
                },
                filter: None,
                actions: vec![],
                cooldown_secs: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn create_rejects_over_quota() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockPipelineRepository::new();
        repo.expect_count_for_user()
            .returning(|_, _| Ok(MAX_PIPELINES_PER_USER_PER_SERVER));

        let svc = make_service(repo, make_member_repo());

        let err = svc
            .create(CreatePipelineCmd {
                actor,
                server_id,
                name: "My Pipeline".to_string(),
                description: None,
                trigger: PipelineTriggerDto {
                    kind: PipelineTriggerKindDto::NewMessage,
                    conversation_id: None,
                },
                filter: None,
                actions: vec![],
                cooldown_secs: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_rejects_invalid_conversation_scope() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let repo = MockPipelineRepository::new();
        let svc = make_service(repo, make_member_repo());

        let conv_id = rim_core::conversation::model::ConversationId::new();
        let err = svc
            .create(CreatePipelineCmd {
                actor,
                server_id,
                name: "My Pipeline".to_string(),
                description: None,
                trigger: PipelineTriggerDto {
                    kind: PipelineTriggerKindDto::MemberJoined,
                    conversation_id: Some(conv_id),
                },
                filter: None,
                actions: vec![],
                cooldown_secs: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn evaluate_matches_new_message() {
        let server_id = ServerId::new();
        let user_id = UserId::new();
        let conv_id = rim_core::conversation::model::ConversationId::new();

        let pipeline = make_pipeline(server_id, user_id);
        let pipeline_clone = pipeline.clone();

        let mut repo = MockPipelineRepository::new();
        repo.expect_list_enabled_for_server()
            .returning(move |_| Ok(vec![pipeline_clone.clone()]));
        repo.expect_set_last_fired_at().returning(|_, _| Ok(()));

        let mut cmd_svc = rim_services::command::service::MockCommandService::new();
        cmd_svc.expect_execute_command().returning(|_| {
            Box::pin(async {
                Ok(rim_services::command::dto::CommandResultDto {
                    status: rim_services::command::dto::CommandResultStatusDto::Success,
                    message: "ok".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

        let svc = DefaultPipelineService::new(
            PipelineRepos {
                pipeline_repo: Arc::new(repo),
                server_member_repo: Arc::new(make_member_repo()),
            },
            Arc::new(cmd_svc),
            Arc::new(rim_services::events::NoopBroadcaster),
        );

        let event = make_new_message_event(server_id, conv_id, user_id);
        let results = svc.evaluate_event(event).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].actions_executed, 1);
        assert!(results[0].skipped_reason.is_none());
    }

    #[tokio::test]
    async fn evaluate_respects_cooldown() {
        let server_id = ServerId::new();
        let user_id = UserId::new();
        let conv_id = rim_core::conversation::model::ConversationId::new();

        let mut pipeline = make_pipeline(server_id, user_id);
        pipeline.cooldown_secs = Some(60);
        let pipeline_clone = pipeline.clone();

        let mut repo = MockPipelineRepository::new();
        repo.expect_list_enabled_for_server()
            .returning(move |_| Ok(vec![pipeline_clone.clone()]));
        repo.expect_set_last_fired_at().returning(|_, _| Ok(()));

        let mut cmd_svc = rim_services::command::service::MockCommandService::new();
        cmd_svc.expect_execute_command().returning(|_| {
            Box::pin(async {
                Ok(rim_services::command::dto::CommandResultDto {
                    status: rim_services::command::dto::CommandResultStatusDto::Success,
                    message: "ok".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

        let svc = DefaultPipelineService::new(
            PipelineRepos {
                pipeline_repo: Arc::new(repo),
                server_member_repo: Arc::new(make_member_repo()),
            },
            Arc::new(cmd_svc),
            Arc::new(rim_services::events::NoopBroadcaster),
        );

        let event = make_new_message_event(server_id, conv_id, user_id);

        // First firing should succeed
        let results = svc.evaluate_event(event.clone()).await.unwrap();
        assert_eq!(results[0].actions_executed, 1);

        // Second firing should be skipped due to cooldown
        let results = svc.evaluate_event(event).await.unwrap();
        assert_eq!(results[0].actions_executed, 0);
        assert!(results[0].skipped_reason.as_deref() == Some("cooldown active"));
    }

    /// One workspace, conversation and author to build events in.
    struct Scene {
        server_id: ServerId,
        conv_id: rim_core::conversation::model::ConversationId,
        author_id: UserId,
    }

    impl Scene {
        fn new() -> Self {
            Self {
                server_id: ServerId::new(),
                conv_id: rim_core::conversation::model::ConversationId::new(),
                author_id: UserId::new(),
            }
        }

        /// A new message reading "hello world".
        fn message(&self) -> WorkspaceEvent {
            make_new_message_event(self.server_id, self.conv_id, self.author_id)
        }

        /// The same message, end-to-end encrypted.
        fn encrypted_message(&self) -> WorkspaceEvent {
            let mut event = self.message();
            if let WorkspaceEvent::NewMessage { message, .. } = &mut event {
                message.encryption = Some(rim_services::events::MessageEncryptionData {
                    incarnation: 0,
                    epoch: 1,
                    sender_device_id: uuid::Uuid::now_v7(),
                    ciphertext: Some("opaque".to_string()),
                    franking_receipt: String::new(),
                    franking_key_id: 1,
                    sent_at_us: String::new(),
                });
            }
            event
        }

        fn deleted(&self) -> WorkspaceEvent {
            WorkspaceEvent::MessageDeleted {
                workspace_id: self.server_id,
                conversation_id: self.conv_id,
                conversation_type: "Channel".to_string(),
                member_user_ids: vec![],
                is_private: false,
                message_id: rim_core::message::model::MessageId::new(),
                thread_id: None,
            }
        }

        fn reactions(&self) -> WorkspaceEvent {
            WorkspaceEvent::ReactionsUpdated {
                workspace_id: self.server_id,
                conversation_id: self.conv_id,
                conversation_type: "Channel".to_string(),
                member_user_ids: vec![],
                is_private: false,
                message_id: rim_core::message::model::MessageId::new(),
                reactions: vec![],
            }
        }

        /// The author joining the workspace.
        fn member_joined(&self) -> WorkspaceEvent {
            WorkspaceEvent::MemberJoined {
                workspace_id: self.server_id,
                user_id: self.author_id,
                username: "hello world".to_string(),
            }
        }

        /// The author changing a workspace state key.
        fn state_changed(&self) -> WorkspaceEvent {
            WorkspaceEvent::WorkspaceStateChanged {
                workspace_id: self.server_id,
                key: "hello world".to_string(),
                value: serde_json::json!("hello world"),
                scope: rim_services::workspace_state::dto::WorkspaceStateScopeDto::Server,
                user_id: Some(self.author_id),
                version: 1,
            }
        }

        fn contains(pattern: &str) -> FilterCondition {
            FilterCondition::MessageContains {
                pattern: pattern.to_string(),
            }
        }

        fn by_author(&self) -> FilterCondition {
            FilterCondition::MessageFromUser {
                user_id: self.author_id,
            }
        }

        fn in_conversation(&self) -> FilterCondition {
            FilterCondition::ConversationIs {
                conversation_id: self.conv_id,
            }
        }

        /// A pipeline of this workspace triggered by `kind`, scoped to `conversation`.
        fn pipeline(
            &self,
            kind: PipelineTriggerKind,
            conversation: Option<rim_core::conversation::model::ConversationId>,
        ) -> Pipeline {
            let mut pipeline = make_pipeline(self.server_id, UserId::new());
            pipeline.trigger = PipelineTrigger {
                kind,
                conversation_id: conversation,
            };
            pipeline
        }
    }

    /// Whether a filter of `conditions` lets `event` through.
    fn passes(conditions: Vec<FilterCondition>, event: &WorkspaceEvent) -> bool {
        let mut pipeline = make_pipeline(ServerId::new(), UserId::new());
        pipeline.filter = Some(PipelineFilter { conditions });
        DefaultPipelineService::apply_filter(&pipeline, event)
    }

    #[test]
    fn message_contains_matches_plain_text_and_never_an_encrypted_message() {
        let scene = Scene::new();
        assert!(passes(vec![Scene::contains("lo wo")], &scene.message()));
        assert!(!passes(vec![Scene::contains("bye")], &scene.message()));
        assert!(!passes(
            vec![Scene::contains("lo wo")],
            &scene.encrypted_message()
        ));
    }

    #[test]
    fn message_from_user_and_conversation_is_match_only_their_own_value() {
        let scene = Scene::new();
        let other = Scene::new();
        assert!(passes(vec![scene.by_author()], &scene.message()));
        assert!(!passes(vec![other.by_author()], &scene.message()));
        assert!(passes(vec![scene.in_conversation()], &scene.message()));
        assert!(!passes(vec![other.in_conversation()], &scene.message()));
    }

    #[test]
    fn every_condition_must_hold() {
        let scene = Scene::new();
        let other = Scene::new();
        let event = scene.message();
        assert!(passes(
            vec![
                Scene::contains("hello"),
                scene.by_author(),
                scene.in_conversation()
            ],
            &event
        ));
        for failing in [
            Scene::contains("bye"),
            other.by_author(),
            other.in_conversation(),
        ] {
            let conditions = vec![
                Scene::contains("hello"),
                scene.by_author(),
                scene.in_conversation(),
                failing.clone(),
            ];
            assert!(!passes(conditions, &event), "{failing:?}");
        }
    }

    #[test]
    fn no_filter_or_an_empty_condition_list_passes_every_event() {
        let scene = Scene::new();
        let unfiltered = make_pipeline(scene.server_id, scene.author_id);
        for event in [
            scene.message(),
            scene.member_joined(),
            scene.state_changed(),
        ] {
            assert!(DefaultPipelineService::apply_filter(&unfiltered, &event));
            assert!(passes(vec![], &event));
        }
    }

    /// Text and author conditions hold only for a message's own text and author, so a member
    /// joining, a state change, a deletion or a reaction never passes them; a conversation
    /// condition also holds for a deletion or reaction in that conversation.
    #[test]
    fn message_conditions_hold_only_for_events_that_carry_their_value() {
        let scene = Scene::new();
        let without_message = [
            scene.member_joined(),
            scene.state_changed(),
            scene.deleted(),
            scene.reactions(),
        ];
        for event in &without_message {
            assert!(!passes(vec![Scene::contains("hello")], event), "{event:?}");
            assert!(!passes(vec![scene.by_author()], event), "{event:?}");
        }
        assert!(!passes(
            vec![scene.in_conversation()],
            &scene.member_joined()
        ));
        assert!(!passes(
            vec![scene.in_conversation()],
            &scene.state_changed()
        ));
        assert!(passes(vec![scene.in_conversation()], &scene.deleted()));
        assert!(passes(vec![scene.in_conversation()], &scene.reactions()));
    }

    /// A trigger fires only for its own event kind and, when scoped, only for an event in that
    /// conversation; workspace-wide events carry no conversation and never match a scope.
    #[test]
    fn a_trigger_matches_its_kind_and_its_conversation_scope() {
        let scene = Scene::new();
        let elsewhere = Some(rim_core::conversation::model::ConversationId::new());
        let here = Some(scene.conv_id);
        let cases = [
            (PipelineTriggerKind::NewMessage, scene.message()),
            (PipelineTriggerKind::MessageDeleted, scene.deleted()),
            (PipelineTriggerKind::ReactionsUpdated, scene.reactions()),
        ];
        for (kind, event) in cases {
            let matches = |scope| {
                DefaultPipelineService::matches_trigger(&scene.pipeline(kind, scope), &event)
            };
            assert!(matches(None), "{kind:?}");
            assert!(matches(here), "{kind:?}");
            assert!(!matches(elsewhere), "{kind:?}");
        }
        assert!(!DefaultPipelineService::matches_trigger(
            &scene.pipeline(PipelineTriggerKind::MessageUpdated, None),
            &scene.message()
        ));
        let joined = scene.pipeline(PipelineTriggerKind::MemberJoined, here);
        assert!(!DefaultPipelineService::matches_trigger(
            &joined,
            &scene.member_joined()
        ));
        let joined = scene.pipeline(PipelineTriggerKind::MemberJoined, None);
        assert!(DefaultPipelineService::matches_trigger(
            &joined,
            &scene.member_joined()
        ));
    }

    /// A triggered pipeline whose filter fails is reported as skipped and runs no action.
    #[tokio::test]
    async fn evaluate_reports_a_failed_filter_and_runs_no_action() {
        let scene = Scene::new();
        let mut pipeline = scene.pipeline(PipelineTriggerKind::NewMessage, None);
        pipeline.filter = Some(PipelineFilter {
            conditions: vec![Scene::contains("bye")],
        });
        let mut repo = MockPipelineRepository::new();
        repo.expect_list_enabled_for_server()
            .returning(move |_| Ok(vec![pipeline.clone()]));
        let svc = make_service(repo, make_member_repo());

        let results = svc.evaluate_event(scene.message()).await.expect("evaluate");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].actions_executed, 0);
        assert_eq!(
            results[0].skipped_reason.as_deref(),
            Some("filter did not match")
        );
    }

    /// What an actor holding `role` in the workspace (`None`: not a member) gets back from
    /// update, toggle and delete, in that order, on a pipeline they did or did not create.
    async fn manage_outcomes(
        role: Option<ServerMemberRole>,
        actor_is_creator: bool,
    ) -> Vec<ServiceResult<()>> {
        let actor = UserId::new();
        let creator = if actor_is_creator {
            actor
        } else {
            UserId::new()
        };
        let pipeline = make_pipeline(ServerId::new(), creator);

        let mut repo = MockPipelineRepository::new();
        let stored = pipeline.clone();
        repo.expect_get_by_id()
            .returning(move |_| Ok(Some(stored.clone())));
        repo.expect_update().returning(|p| Ok(p.clone()));
        let stored = pipeline.clone();
        repo.expect_set_enabled()
            .returning(move |_, _| Ok(stored.clone()));
        repo.expect_delete().returning(|_| Ok(()));
        let mut members = MockServerMemberRepository::new();
        members
            .expect_get_member_standing()
            .returning(move |_, _| Ok(role.map(MemberStanding::new)));
        members
            .expect_join_direct_space()
            .returning(|_, _| Ok(false));

        let svc = make_service(repo, members);
        let actor = Actor::new(actor);
        vec![
            svc.update(UpdatePipelineCmd {
                actor,
                pipeline_id: pipeline.id,
                name: "Renamed".to_string(),
                description: None,
                trigger: PipelineTriggerDto {
                    kind: PipelineTriggerKindDto::NewMessage,
                    conversation_id: None,
                },
                filter: None,
                actions: vec![],
                cooldown_secs: None,
            })
            .await
            .map(drop),
            svc.toggle(TogglePipelineCmd {
                actor,
                pipeline_id: pipeline.id,
                enabled: false,
            })
            .await
            .map(drop),
            svc.delete(DeletePipelineCmd {
                actor,
                pipeline_id: pipeline.id,
            })
            .await,
        ]
    }

    fn all_denied(outcomes: &[ServiceResult<()>]) -> bool {
        outcomes
            .iter()
            .all(|o| matches!(o, Err(ServiceError::PermissionDenied)))
    }

    #[tokio::test]
    async fn the_creator_may_update_toggle_and_delete_their_pipeline() {
        let outcomes = manage_outcomes(Some(ServerMemberRole::Member), true).await;
        assert!(outcomes.iter().all(Result::is_ok), "{outcomes:?}");
    }

    #[tokio::test]
    async fn an_owner_or_moderator_may_update_toggle_and_delete_any_pipeline() {
        for role in [ServerMemberRole::Owner, ServerMemberRole::Moderator] {
            let outcomes = manage_outcomes(Some(role), false).await;
            assert!(outcomes.iter().all(Result::is_ok), "{role}: {outcomes:?}");
        }
    }

    #[tokio::test]
    async fn another_member_or_guest_may_not_change_a_pipeline() {
        for role in [ServerMemberRole::Member, ServerMemberRole::Guest] {
            let outcomes = manage_outcomes(Some(role), false).await;
            assert!(all_denied(&outcomes), "{role}: {outcomes:?}");
        }
    }

    #[tokio::test]
    async fn a_creator_who_left_the_workspace_may_not_change_their_pipeline() {
        let outcomes = manage_outcomes(None, true).await;
        assert!(all_denied(&outcomes), "{outcomes:?}");
    }
}
