use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use dashmap::DashMap;

use rim_core::conversation::model::{Conversation, ConversationId};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::pipeline::model::{
    FilterCondition, Pipeline, PipelineAction, PipelineFilter, PipelineId, PipelineTrigger,
    PipelineTriggerKind,
};
use rim_core::pipeline::repository::PipelineRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_services::command::bounds::{
    MAX_DESCRIPTION_CHARS, MAX_STEP_ARGS_BYTES, ensure_args_size, ensure_chars,
    ensure_command_id_size,
};
use rim_services::command::dto::CommandContextDto;
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::ExecuteCommandCmd;
use rim_services::command::service::CommandService;
use rim_services::events::visibility::user_can_see_event_as;
use rim_services::events::{Broadcaster, WorkspaceEvent};
use rim_services::limits::{InstanceLimits, ensure_below, member_share};
use rim_services::pipeline::dto::{
    FilterConditionDto, PipelineActionDto, PipelineDto, PipelineFilterDto, PipelineFiredResult,
    PipelineTriggerDto,
};
use rim_services::pipeline::request::{
    CreatePipelineCmd, DeletePipelineCmd, ListPipelinesCmd, TogglePipelineCmd, UpdatePipelineCmd,
};
use rim_services::pipeline::service::PipelineService;
use rim_services::{Actor, ServiceError, ServiceResult};

const MAX_PIPELINES_PER_USER_PER_SERVER: u64 = 50;
const RATE_LIMIT_WINDOW_SECS: u64 = 60;
const RATE_LIMIT_MAX_FIRINGS: u32 = 10;
/// Actions one pipeline may list; a firing runs at most this many.
const MAX_ACTIONS: usize = 10;
/// Conditions one pipeline's filter may list.
const MAX_FILTER_CONDITIONS: usize = 5;
/// Characters of a `MessageContains` filter's pattern.
const MAX_FILTER_PATTERN_CHARS: usize = 256;
/// The refusal of a pipeline action naming a user command or a macro.
const COMPOSED_ACTION: &str = "pipeline actions must not reference user commands or macros";

#[derive(Clone)]
pub struct PipelineRepos {
    pub pipeline_repo: Arc<dyn PipelineRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    /// Loads the conversation an event comes from, to decide whether a creator may read it.
    pub conversation_repo: Arc<dyn ConversationRepository>,
    /// Answers whether a creator is a member of that conversation.
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
}

/// One workspace event and what has been decided about who may see it, so pipelines that share
/// a creator cost one lookup per event.
struct EventAudience<'a> {
    event: &'a WorkspaceEvent,
    server_id: ServerId,
    /// The event's conversation once loaded: `Some(None)` when it is not in the workspace.
    conversation: Option<Option<Conversation>>,
    /// Whether each creator asked about so far may see the event.
    verdicts: HashMap<UserId, bool>,
}

impl<'a> EventAudience<'a> {
    fn new(event: &'a WorkspaceEvent, server_id: ServerId) -> Self {
        Self {
            event,
            server_id,
            conversation: None,
            verdicts: HashMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct DefaultPipelineService {
    repos: PipelineRepos,
    command_service: Arc<dyn CommandService>,
    broadcaster: Arc<dyn Broadcaster>,
    cooldowns: Arc<DashMap<PipelineId, Instant>>,
    rate_limits: Arc<DashMap<PipelineId, (u32, Instant)>>,
    /// The relay's direct space, which has no pipelines.
    direct_space_id: Option<ServerId>,
    /// The cap on pipelines per workspace.
    limits: InstanceLimits,
    /// The server's default workspace, where no per-workspace cap applies and accounts in
    /// probation create none; `None` where none is wired.
    default_workspace: Option<crate::default_workspace_rules::DefaultWorkspaceRules>,
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
            direct_space_id: None,
            limits: InstanceLimits::default(),
            default_workspace: None,
        }
    }

    pub fn with_default_workspace(
        mut self,
        rules: crate::default_workspace_rules::DefaultWorkspaceRules,
    ) -> Self {
        self.default_workspace = Some(rules);
        self
    }

    /// Whether `server_id` is the default workspace, after refusing `user_id` there while in
    /// probation (`Validation` saying they can `action` once it ends).
    async fn in_default_workspace(
        &self,
        server_id: ServerId,
        user_id: UserId,
        action: &str,
    ) -> ServiceResult<bool> {
        let Some(rules) = &self.default_workspace else {
            return Ok(false);
        };
        if !rules.is_default(server_id).await? {
            return Ok(false);
        }
        rules.ensure_out_of_probation(user_id, action).await?;
        Ok(true)
    }

    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// The relay's direct space, or `None` where none is wired.
    pub fn with_direct_space(mut self, direct_space_id: Option<ServerId>) -> Self {
        self.direct_space_id = direct_space_id;
        self
    }

    /// `Validation` in the direct space, else `PermissionDenied` for a non-member.
    async fn ensure_member(&self, server_id: ServerId, user_id: UserId) -> ServiceResult<()> {
        crate::util::ensure_workspace_features(self.direct_space_id, server_id)?;
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
        crate::util::ensure_workspace_features(self.direct_space_id, pipeline.server_id)?;
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

    /// Whether `creator` may see `audience`'s event, by the rules a reader of the workspace
    /// gets: a message, edit, deletion or reaction only in a conversation the creator may read
    /// (a public channel as a member who is not a guest, any other conversation as its member);
    /// any other event as a workspace subscriber would see it. A creator who is no longer a
    /// member of the workspace sees nothing.
    ///
    /// # Errors
    ///
    /// A repository failure while looking up the creator's standing, the conversation or the
    /// membership.
    async fn creator_sees(
        &self,
        audience: &mut EventAudience<'_>,
        creator: UserId,
    ) -> ServiceResult<bool> {
        if let Some(verdict) = audience.verdicts.get(&creator) {
            return Ok(*verdict);
        }
        let verdict = self.decide_creator_sees(audience, creator).await?;
        audience.verdicts.insert(creator, verdict);
        Ok(verdict)
    }

    /// The uncached decision behind [`Self::creator_sees`].
    ///
    /// # Errors
    ///
    /// As [`Self::creator_sees`].
    async fn decide_creator_sees(
        &self,
        audience: &mut EventAudience<'_>,
        creator: UserId,
    ) -> ServiceResult<bool> {
        let Some(standing) = self
            .repos
            .server_member_repo
            .get_member_standing(audience.server_id, creator)
            .await?
        else {
            return Ok(false);
        };
        let Some(conversation_id) = event_conversation_id(audience.event) else {
            return Ok(user_can_see_event_as(
                audience.event,
                &creator.to_string(),
                standing.role.is_guest(),
            ));
        };
        let Some(conversation) = self
            .audience_conversation(audience, conversation_id)
            .await?
        else {
            return Ok(false);
        };
        match crate::util::ensure_conversation_access_as(
            self.repos.conversation_member_repo.as_ref(),
            conversation,
            creator,
            standing.role,
        )
        .await
        {
            Ok(()) => Ok(true),
            Err(ServiceError::NotFound | ServiceError::PermissionDenied) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The event's conversation, loaded on first use; `None` when it is not in the workspace.
    ///
    /// # Errors
    ///
    /// A repository failure while loading the conversation.
    async fn audience_conversation<'b>(
        &self,
        audience: &'b mut EventAudience<'_>,
        conversation_id: ConversationId,
    ) -> ServiceResult<Option<&'b Conversation>> {
        if audience.conversation.is_none() {
            let loaded = match crate::util::ensure_conversation_in_server(
                self.repos.conversation_repo.as_ref(),
                conversation_id,
                audience.server_id,
            )
            .await
            {
                Ok(conversation) => Some(conversation),
                Err(ServiceError::NotFound) => None,
                Err(e) => return Err(e),
            };
            audience.conversation = Some(loaded);
        }
        Ok(audience.conversation.as_ref().and_then(Option::as_ref))
    }

    /// `pipeline` as `viewer` sees it: only its creator is told when it last fired.
    fn dto_for(pipeline: Pipeline, viewer: UserId) -> PipelineDto {
        let mut dto = PipelineDto::from(pipeline);
        if dto.created_by != viewer {
            dto.last_fired_at = None;
        }
        dto
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

    /// Refuses a description longer than [`MAX_DESCRIPTION_CHARS`] characters.
    ///
    /// # Errors
    ///
    /// `Validation` saying so.
    fn validate_description(description: Option<&str>) -> ServiceResult<()> {
        match description {
            Some(description) => {
                ensure_chars(description, MAX_DESCRIPTION_CHARS, "a pipeline description")
            }
            None => Ok(()),
        }
    }

    /// Refuses a filter with more than [`MAX_FILTER_CONDITIONS`] conditions or a pattern longer
    /// than [`MAX_FILTER_PATTERN_CHARS`] characters.
    ///
    /// # Errors
    ///
    /// `Validation` naming the bound.
    fn validate_filter(filter: Option<&PipelineFilterDto>) -> ServiceResult<()> {
        let Some(filter) = filter else {
            return Ok(());
        };
        if filter.conditions.len() > MAX_FILTER_CONDITIONS {
            return Err(ServiceError::Validation(format!(
                "a pipeline filter may have at most {MAX_FILTER_CONDITIONS} conditions"
            )));
        }
        for condition in &filter.conditions {
            if let FilterConditionDto::MessageContains { pattern } = condition {
                ensure_chars(
                    pattern,
                    MAX_FILTER_PATTERN_CHARS,
                    "a pipeline filter pattern",
                )?;
            }
        }
        Ok(())
    }

    /// Refuses more than [`MAX_ACTIONS`] actions, or an action that names a user command, a
    /// macro or no builtin command, has a command id longer than 256 bytes, or has arguments
    /// that serialize to more than [`MAX_STEP_ARGS_BYTES`]. A user command is refused because
    /// each action runs with its own budget of builtin steps, which a user command could fill on
    /// every firing.
    ///
    /// # Errors
    ///
    /// `Validation` naming the reason.
    fn validate_actions(actions: &[PipelineActionDto]) -> ServiceResult<()> {
        if actions.len() > MAX_ACTIONS {
            return Err(ServiceError::Validation(format!(
                "a pipeline may have at most {MAX_ACTIONS} actions"
            )));
        }
        for action in actions {
            let PipelineActionDto::ExecuteCommand { command_id, args } = action;
            if is_composed_command(command_id) {
                return Err(ServiceError::Validation(COMPOSED_ACTION.to_string()));
            }
            ensure_command_id_size(command_id, "a pipeline action's command id")?;
            if !crate::command::registry::is_builtin_command(command_id) {
                return Err(ServiceError::Validation(format!(
                    "Unknown command: {command_id}"
                )));
            }
            ensure_args_size(
                args.as_ref(),
                MAX_STEP_ARGS_BYTES,
                "a pipeline action's args",
            )?;
        }
        Ok(())
    }

    /// Refuses a create or update whose name, description, trigger, filter, actions or cooldown
    /// is out of bounds.
    ///
    /// # Errors
    ///
    /// `Validation` naming the field.
    fn validate_definition(
        name: &str,
        description: Option<&str>,
        trigger: &PipelineTriggerDto,
        filter: Option<&PipelineFilterDto>,
        actions: &[PipelineActionDto],
        cooldown_secs: Option<i32>,
    ) -> ServiceResult<()> {
        Self::validate_name(name)?;
        Self::validate_description(description)?;
        Self::validate_trigger(trigger)?;
        Self::validate_filter(filter)?;
        Self::validate_actions(actions)?;
        Self::validate_cooldown(cooldown_secs)
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

/// Whether `command_id` names a user command or a macro, which a pipeline action may not run.
fn is_composed_command(command_id: &str) -> bool {
    command_id.starts_with("user.") || command_id.starts_with("macro.")
}

fn event_conversation_id(event: &WorkspaceEvent) -> Option<ConversationId> {
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
        Self::validate_definition(
            &cmd.name,
            cmd.description.as_deref(),
            &cmd.trigger,
            cmd.filter.as_ref(),
            &cmd.actions,
            cmd.cooldown_secs,
        )?;

        // Quota check
        let in_default_workspace = self
            .in_default_workspace(cmd.server_id, cmd.actor.user_id, "create a pipeline")
            .await?;
        let is_moderator = self
            .repos
            .server_member_repo
            .get_member(cmd.server_id, cmd.actor.user_id)
            .await?
            .is_some_and(|m| m.member_role.is_moderator());
        let per_user = member_share(
            self.limits.pipelines_per_workspace,
            MAX_PIPELINES_PER_USER_PER_SERVER,
            is_moderator,
        );
        let count = self
            .repos
            .pipeline_repo
            .count_for_user(cmd.server_id, cmd.actor.user_id)
            .await?;

        if count >= per_user {
            return Err(ServiceError::Validation(format!(
                "quota exceeded: maximum {per_user} pipelines per user per server"
            )));
        }
        // Pipelines belong to their creator: in the default workspace the member share bounds
        // them and no account can fill a workspace cap for everyone.
        if !in_default_workspace && self.limits.pipelines_per_workspace.is_some() {
            let in_workspace = self
                .repos
                .pipeline_repo
                .count_for_server(cmd.server_id)
                .await?;
            ensure_below(
                self.limits.pipelines_per_workspace,
                in_workspace,
                "pipelines per workspace",
            )?;
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

        let viewer = cmd.actor.user_id;
        Ok(pipelines
            .into_iter()
            .map(|p| Self::dto_for(p, viewer))
            .collect())
    }

    async fn update(&self, cmd: UpdatePipelineCmd) -> ServiceResult<PipelineDto> {
        let existing = self
            .repos
            .pipeline_repo
            .get_by_id(cmd.pipeline_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        self.ensure_can_manage(&existing, cmd.actor.user_id).await?;
        Self::validate_definition(
            &cmd.name,
            cmd.description.as_deref(),
            &cmd.trigger,
            cmd.filter.as_ref(),
            &cmd.actions,
            cmd.cooldown_secs,
        )?;

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

        Ok(Self::dto_for(stored, cmd.actor.user_id))
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

        Ok(Self::dto_for(stored, cmd.actor.user_id))
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
        let mut audience = EventAudience::new(&event, server_id);

        for pipeline in &pipelines {
            if !Self::matches_trigger(pipeline, &event) {
                continue;
            }

            // A pipeline reacts only to events its creator may see: one they may not see neither
            // fires it nor counts towards its cooldown or suspension, and is not reported.
            match self.creator_sees(&mut audience, pipeline.created_by).await {
                Ok(true) => {}
                Ok(false) => continue,
                Err(e) => {
                    tracing::warn!(
                        pipeline_id = %pipeline.id,
                        "could not decide whether the pipeline's creator may see the event; skipped: {e}"
                    );
                    continue;
                }
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
                        member_user_ids: vec![pipeline.created_by],
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
            for action in pipeline.actions.iter().take(MAX_ACTIONS) {
                match action {
                    PipelineAction::ExecuteCommand { command_id, .. }
                        if is_composed_command(command_id) =>
                    {
                        tracing::warn!(
                            pipeline_id = %pipeline.id,
                            command_id = %command_id,
                            "pipeline action names a user command or a macro; action skipped",
                        );
                    }
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
                member_user_ids: vec![pipeline.created_by],
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
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use rim_core::conversation::model::{ConversationType, PostingPolicy};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::pipeline::model::{PipelineAction, PipelineTrigger, PipelineTriggerKind};
    use rim_core::repository::error::RepoResult;
    use rim_core::server_member::model::{MemberStanding, ServerMember, ServerMemberRole};
    use rim_core::server_member::repository::ServerMemberRepository;
    use rim_services::Actor;
    use rim_services::command::dto::{CommandResultDto, CommandResultStatusDto};
    use rim_services::command::service::MockCommandService;
    use rim_services::events::MessageEventData;
    use rim_services::pipeline::dto::{
        PipelineActionDto, PipelineTriggerDto, PipelineTriggerKindDto,
    };
    use rim_services::workspace_state::dto::WorkspaceStateScopeDto;

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
            async fn count_for_server(&self, server_id: ServerId) -> RepoResult<u64>;
            async fn delete_for_user(&self, server_id: ServerId, user_id: UserId) -> RepoResult<u64>;
        }
    }

    mockall::mock! {
        ServerMemberRepository {}
        #[async_trait::async_trait]
        impl ServerMemberRepository for ServerMemberRepository {
            async fn count_members(&self, server_id: ServerId) -> RepoResult<u64>;
            async fn get_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<Option<ServerMember>>;
            async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<ServerMember>;
            async fn change_member_role(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<()>;
            async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;
            async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>>;
            async fn get_members(&self, server_id: ServerId, user_ids: &[UserId]) -> RepoResult<Vec<ServerMember>>;
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn join_direct_space(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn get_member_standing(&self, server_id: rim_core::server::model::ServerId, user_id: rim_core::user::model::UserId) -> RepoResult<Option<rim_core::server_member::model::MemberStanding>>;
            async fn get_owner_id(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<Option<rim_core::user::model::UserId>>;
        }
    }

    fn make_member_repo() -> MockServerMemberRepository {
        make_member_repo_as(ServerMemberRole::Member)
    }

    /// A member repository where everyone is a member with `role`.
    fn make_member_repo_as(role: ServerMemberRole) -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        repo.expect_is_member().returning(|_, _| Ok(true));
        repo.expect_get_member()
            .returning(move |server_id, user_id| {
                Ok(Some(ServerMember {
                    id: rim_core::server_member::model::ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role: role,
                    joined_at: chrono::Utc::now(),
                }))
            });
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
        DefaultPipelineService::new(
            PipelineRepos {
                pipeline_repo: Arc::new(pipeline_repo),
                server_member_repo: Arc::new(member_repo),
                conversation_repo: Arc::new(MockConversationRepository::new()),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
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
                edited_at: None,
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
                    command_id: "chat.post_message".to_string(),
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
    async fn create_follows_the_pipelines_per_workspace_cap() {
        for (cap, in_workspace, allowed) in [
            (Some(50), Some(49), true),
            (Some(50), Some(50), false),
            (None, None, true),
        ] {
            let mut repo = MockPipelineRepository::new();
            repo.expect_count_for_user().returning(|_, _| Ok(0));
            match in_workspace {
                Some(n) => {
                    repo.expect_count_for_server().returning(move |_| Ok(n));
                }
                None => {
                    repo.expect_count_for_server().never();
                }
            }
            repo.expect_create()
                .times(usize::from(allowed))
                .returning(|p| Ok(p.clone()));
            let svc = make_service(repo, make_member_repo()).with_limits(
                rim_services::limits::InstanceLimits {
                    pipelines_per_workspace: cap,
                    ..Default::default()
                },
            );
            let result = svc
                .create(CreatePipelineCmd {
                    actor: Actor::new(UserId::new()),
                    server_id: ServerId::new(),
                    name: "Capped".to_string(),
                    description: None,
                    trigger: PipelineTriggerDto {
                        kind: PipelineTriggerKindDto::NewMessage,
                        conversation_id: None,
                    },
                    filter: None,
                    actions: vec![PipelineActionDto::ExecuteCommand {
                        command_id: "chat.post_message".to_string(),
                        args: None,
                    }],
                    cooldown_secs: None,
                })
                .await;
            match result {
                Ok(_) => assert!(allowed, "{cap:?} {in_workspace:?}"),
                Err(ServiceError::Validation(message)) => {
                    assert!(!allowed, "{cap:?} {in_workspace:?}");
                    assert_eq!(message, "this instance allows 50 pipelines per workspace");
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn the_default_workspace_takes_a_members_share_but_no_workspace_cap_or_probation() {
        let lobby = ServerId::new();
        let now = chrono::Utc::now();
        let (old, new) = (UserId::new(), UserId::new());
        let accounts = vec![
            crate::probation_fixtures::account(old, now - chrono::Duration::days(3), None),
            crate::probation_fixtures::account(new, now - chrono::Duration::hours(1), None),
        ];
        let limits = rim_services::limits::InstanceLimits {
            pipelines_per_workspace: Some(50),
            probation_hours: Some(24),
            ..Default::default()
        };
        for (actor, held, outcome) in [
            (old, 9, Ok(())),
            (
                old,
                10,
                Err("quota exceeded: maximum 10 pipelines per user per server"),
            ),
            (
                new,
                0,
                Err("a new account can create a pipeline in 1380 minutes"),
            ),
        ] {
            let mut repo = MockPipelineRepository::new();
            repo.expect_count_for_user().returning(move |_, _| Ok(held));
            repo.expect_count_for_server().never();
            repo.expect_create().returning(|p| Ok(p.clone()));
            let svc = make_service(repo, make_member_repo())
                .with_limits(limits)
                .with_default_workspace(
                    crate::default_workspace_rules::DefaultWorkspaceRules::testing(
                        lobby,
                        accounts.clone(),
                        limits,
                        now,
                    ),
                );
            let result = svc
                .create(CreatePipelineCmd {
                    actor: Actor::new(actor),
                    server_id: lobby,
                    name: "Mine".to_string(),
                    description: None,
                    trigger: PipelineTriggerDto {
                        kind: PipelineTriggerKindDto::NewMessage,
                        conversation_id: None,
                    },
                    filter: None,
                    actions: vec![PipelineActionDto::ExecuteCommand {
                        command_id: "chat.post_message".to_string(),
                        args: None,
                    }],
                    cooldown_secs: None,
                })
                .await;
            match (result, outcome) {
                (Ok(_), Ok(())) => {}
                (Err(ServiceError::Validation(message)), Err(expected)) => {
                    assert_eq!(message, expected)
                }
                (other, expected) => panic!("expected {expected:?}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_member_holds_a_fifth_of_the_pipelines_per_workspace_cap() {
        for (role, held, allowed) in [
            (ServerMemberRole::Member, 9, true),
            (ServerMemberRole::Member, 10, false),
            (ServerMemberRole::Moderator, 10, true),
            (ServerMemberRole::Owner, 49, true),
        ] {
            let mut repo = MockPipelineRepository::new();
            repo.expect_count_for_user().returning(move |_, _| Ok(held));
            repo.expect_count_for_server().returning(|_| Ok(0));
            repo.expect_create()
                .times(usize::from(allowed))
                .returning(|p| Ok(p.clone()));
            let svc = make_service(repo, make_member_repo_as(role)).with_limits(
                rim_services::limits::InstanceLimits {
                    pipelines_per_workspace: Some(50),
                    ..Default::default()
                },
            );
            let result = svc
                .create(CreatePipelineCmd {
                    actor: Actor::new(UserId::new()),
                    server_id: ServerId::new(),
                    name: "Shared".to_string(),
                    description: None,
                    trigger: PipelineTriggerDto {
                        kind: PipelineTriggerKindDto::NewMessage,
                        conversation_id: None,
                    },
                    filter: None,
                    actions: vec![PipelineActionDto::ExecuteCommand {
                        command_id: "chat.post_message".to_string(),
                        args: None,
                    }],
                    cooldown_secs: None,
                })
                .await;
            match result {
                Ok(_) => assert!(allowed, "{role:?} {held}"),
                Err(ServiceError::Validation(message)) => {
                    assert!(!allowed, "{role:?} {held}");
                    assert_eq!(
                        message,
                        "quota exceeded: maximum 10 pipelines per user per server"
                    );
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
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
        let mut world = World::new();
        let creator = world.member(ServerMemberRole::Member);
        let channel = world.conversation(ConversationType::Channel, false, &[]);
        let h = world.harness(vec![make_pipeline(world.server_id, creator)]);

        let results = h
            .svc
            .evaluate_event(world.message(channel, "hello"))
            .await
            .expect("evaluate");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].actions_executed, 1);
        assert!(results[0].skipped_reason.is_none());
    }

    #[tokio::test]
    async fn evaluate_respects_cooldown() {
        let mut world = World::new();
        let creator = world.member(ServerMemberRole::Member);
        let channel = world.conversation(ConversationType::Channel, false, &[]);
        let mut pipeline = make_pipeline(world.server_id, creator);
        pipeline.cooldown_secs = Some(60);
        let h = world.harness(vec![pipeline]);
        let event = world.message(channel, "hello");

        let results = h.svc.evaluate_event(event.clone()).await.expect("first");
        assert_eq!(results[0].actions_executed, 1);

        let results = h.svc.evaluate_event(event).await.expect("second");
        assert_eq!(results[0].actions_executed, 0);
        assert_eq!(
            results[0].skipped_reason.as_deref(),
            Some("cooldown active")
        );
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
                source: None,
            }
        }

        /// The author changing a workspace state key.
        fn state_changed(&self) -> WorkspaceEvent {
            WorkspaceEvent::WorkspaceStateChanged {
                workspace_id: self.server_id,
                key: "hello world".to_string(),
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

    /// A bulk delete fires no pipeline: one moderation action would otherwise run a
    /// `MESSAGE_DELETED` pipeline once per deleted message.
    #[test]
    fn a_bulk_delete_fires_no_message_deleted_pipeline() {
        let scene = Scene::new();
        let bulk = WorkspaceEvent::MessagesDeleted {
            workspace_id: scene.server_id,
            conversation_id: scene.conv_id,
            conversation_type: "Channel".to_string(),
            member_user_ids: vec![],
            is_private: false,
            message_ids: vec![rim_core::message::model::MessageId::new()],
            thread_ids: vec![],
        };
        let kinds = [
            PipelineTriggerKind::NewMessage,
            PipelineTriggerKind::MessageUpdated,
            PipelineTriggerKind::MessageDeleted,
            PipelineTriggerKind::ReactionsUpdated,
            PipelineTriggerKind::MemberJoined,
            PipelineTriggerKind::WorkspaceStateChanged,
        ];
        for kind in kinds {
            for scope in [None, Some(scene.conv_id)] {
                assert!(
                    !DefaultPipelineService::matches_trigger(&scene.pipeline(kind, scope), &bulk),
                    "{kind:?} {scope:?}"
                );
            }
        }
    }

    /// A triggered pipeline whose filter fails is reported as skipped and runs no action.
    #[tokio::test]
    async fn evaluate_reports_a_failed_filter_and_runs_no_action() {
        let mut world = World::new();
        let creator = world.member(ServerMemberRole::Member);
        let channel = world.conversation(ConversationType::Channel, false, &[]);
        let h = world.harness(vec![world.pipeline(creator)]);

        let results = h
            .svc
            .evaluate_event(world.message(channel, "hello world"))
            .await
            .expect("evaluate");
        assert_eq!(h.commands(), 0);
        assert!(h.stamped().is_empty());
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

    #[derive(Default)]
    struct RecordingBroadcaster(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for RecordingBroadcaster {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().expect("events").push(event);
        }
    }

    fn success() -> CommandResultDto {
        CommandResultDto {
            status: CommandResultStatusDto::Success,
            message: "ok".to_string(),
            payload: None,
            follow_up_commands: vec![],
            pipeline_stages: None,
            confirmation_required: None,
        }
    }

    /// A workspace whose members, conversations and conversation members the evaluator's
    /// repositories report.
    struct World {
        server_id: ServerId,
        roles: HashMap<UserId, ServerMemberRole>,
        conversations: HashMap<ConversationId, Conversation>,
        joined: HashSet<(ConversationId, UserId)>,
    }

    impl World {
        fn new() -> Self {
            Self {
                server_id: ServerId::new(),
                roles: HashMap::new(),
                conversations: HashMap::new(),
                joined: HashSet::new(),
            }
        }

        /// A new workspace member holding `role`.
        fn member(&mut self, role: ServerMemberRole) -> UserId {
            let user_id = UserId::new();
            self.roles.insert(user_id, role);
            user_id
        }

        /// A new conversation of `kind` in the workspace that `members` have joined.
        fn conversation(
            &mut self,
            kind: ConversationType,
            is_private: bool,
            members: &[UserId],
        ) -> ConversationId {
            let id = ConversationId::new();
            let now = chrono::Utc::now();
            self.conversations.insert(
                id,
                Conversation {
                    id,
                    conversation_type: kind,
                    title: None,
                    topic: None,
                    created_at: now,
                    last_activity_at: now,
                    server_id: self.server_id,
                    is_private,
                    archived_at: None,
                    created_by: None,
                    slow_mode_seconds: 0,
                    posting_policy: PostingPolicy::Everyone,
                    posting_allows_threads: true,
                    encrypted_at: None,
                },
            );
            self.joined.extend(members.iter().map(|m| (id, *m)));
            id
        }

        /// A new message reading `text` in `conversation_id`, by someone outside the workspace,
        /// with the event's routing fields taken from the conversation when it exists.
        fn message(&self, conversation_id: ConversationId, text: &str) -> WorkspaceEvent {
            let mut event = make_new_message_event(self.server_id, conversation_id, UserId::new());
            if let WorkspaceEvent::NewMessage {
                conversation_type,
                is_private,
                member_user_ids,
                message,
                ..
            } = &mut event
            {
                if let Some(conversation) = self.conversations.get(&conversation_id) {
                    *conversation_type = conversation.conversation_type.as_str().to_string();
                    *is_private = conversation.is_private;
                }
                *member_user_ids = self
                    .joined
                    .iter()
                    .filter(|(c, _)| *c == conversation_id)
                    .map(|(_, u)| *u)
                    .collect();
                message.text = Some(text.to_string());
            }
            event
        }

        /// A pipeline of `creator` on new messages that contain "bye".
        fn pipeline(&self, creator: UserId) -> Pipeline {
            let mut pipeline = make_pipeline(self.server_id, creator);
            pipeline.filter = Some(PipelineFilter {
                conditions: vec![FilterCondition::MessageContains {
                    pattern: "bye".to_string(),
                }],
            });
            pipeline
        }

        fn member_repo(&self) -> MockServerMemberRepository {
            let (server_id, roles) = (self.server_id, self.roles.clone());
            let mut repo = MockServerMemberRepository::new();
            repo.expect_get_member_standing()
                .returning(move |server, user| {
                    Ok(roles
                        .get(&user)
                        .filter(|_| server == server_id)
                        .map(|role| MemberStanding::new(*role)))
                });
            repo
        }

        fn conversation_repo(&self) -> MockConversationRepository {
            let conversations = self.conversations.clone();
            let mut repo = MockConversationRepository::new();
            repo.expect_get_by_id().returning(move |id| {
                let found = conversations.get(&id).cloned();
                Box::pin(async move { Ok(found) })
            });
            repo
        }

        fn conversation_member_repo(&self) -> MockConversationMemberRepository {
            let joined = self.joined.clone();
            let mut repo = MockConversationMemberRepository::new();
            repo.expect_is_member().returning(move |c, u| {
                let member = joined.contains(&(c, u));
                Box::pin(async move { Ok(member) })
            });
            repo
        }

        /// The evaluator over this workspace running `pipelines`, recording what it does.
        fn harness(&self, pipelines: Vec<Pipeline>) -> Harness {
            let stamped = Arc::new(Mutex::new(Vec::new()));
            let disabled = Arc::new(Mutex::new(Vec::new()));
            let mut repo = MockPipelineRepository::new();
            repo.expect_list_enabled_for_server()
                .returning(move |_| Ok(pipelines.clone()));
            let record = Arc::clone(&stamped);
            repo.expect_set_last_fired_at().returning(move |id, _| {
                record.lock().expect("stamped").push(id);
                Ok(())
            });
            let record = Arc::clone(&disabled);
            repo.expect_set_enabled().returning(move |id, _| {
                record.lock().expect("disabled").push(id);
                Ok(make_pipeline(ServerId::new(), UserId::new()))
            });

            let commands = Arc::new(AtomicUsize::new(0));
            let count = Arc::clone(&commands);
            let mut command_service = MockCommandService::new();
            command_service
                .expect_execute_command()
                .returning(move |_| {
                    count.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Ok(success()) })
                });

            let events = Arc::new(RecordingBroadcaster::default());
            let svc = DefaultPipelineService::new(
                PipelineRepos {
                    pipeline_repo: Arc::new(repo),
                    server_member_repo: Arc::new(self.member_repo()),
                    conversation_repo: Arc::new(self.conversation_repo()),
                    conversation_member_repo: Arc::new(self.conversation_member_repo()),
                },
                Arc::new(command_service),
                Arc::clone(&events) as Arc<dyn Broadcaster>,
            );
            Harness {
                svc,
                commands,
                stamped,
                disabled,
                events,
            }
        }
    }

    /// The evaluator with what it did recorded: commands run, pipelines stamped as fired or
    /// switched off, events broadcast.
    struct Harness {
        svc: DefaultPipelineService,
        commands: Arc<AtomicUsize>,
        stamped: Arc<Mutex<Vec<PipelineId>>>,
        disabled: Arc<Mutex<Vec<PipelineId>>>,
        events: Arc<RecordingBroadcaster>,
    }

    impl Harness {
        /// The ids of the pipelines that fired on `event`, sorted.
        async fn fired(&self, event: WorkspaceEvent) -> Vec<String> {
            let mut fired: Vec<String> = self
                .svc
                .evaluate_event(event)
                .await
                .expect("evaluate")
                .into_iter()
                .filter(|r| r.skipped_reason.is_none())
                .map(|r| r.pipeline_id)
                .collect();
            fired.sort();
            fired
        }

        fn commands(&self) -> usize {
            self.commands.load(Ordering::SeqCst)
        }

        fn stamped(&self) -> Vec<PipelineId> {
            self.stamped.lock().expect("stamped").clone()
        }

        fn disabled(&self) -> Vec<PipelineId> {
            self.disabled.lock().expect("disabled").clone()
        }

        fn events(&self) -> Vec<WorkspaceEvent> {
            self.events.0.lock().expect("events").clone()
        }
    }

    /// The sorted ids of `pipelines`.
    fn ids<'a>(pipelines: impl IntoIterator<Item = &'a Pipeline>) -> Vec<String> {
        let mut ids: Vec<String> = pipelines.into_iter().map(|p| p.id.to_string()).collect();
        ids.sort();
        ids
    }

    /// A message in a private channel fires the pipeline of a creator who joined the channel and
    /// not that of a workspace member who did not: the skipped pipeline runs nothing, is not
    /// stamped as fired, and its creator is told nothing.
    #[tokio::test]
    async fn a_private_channel_message_fires_only_its_members_pipelines() {
        let mut world = World::new();
        let inside = world.member(ServerMemberRole::Member);
        let outside = world.member(ServerMemberRole::Member);
        let channel = world.conversation(ConversationType::Channel, true, &[inside]);
        let (theirs, snooper) = (world.pipeline(inside), world.pipeline(outside));
        let h = world.harness(vec![theirs.clone(), snooper.clone()]);

        let fired = h.fired(world.message(channel, "goodbye")).await;

        assert_eq!(fired, ids([&theirs]));
        assert_eq!(h.commands(), 1);
        assert_eq!(h.stamped(), [theirs.id]);
        let events = h.events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(
            matches!(
                &events[0],
                WorkspaceEvent::PipelineFired { pipeline_id, member_user_ids, .. }
                    if *pipeline_id == theirs.id && member_user_ids == &[inside]
            ),
            "{events:?}"
        );
    }

    /// A public channel's message fires the pipeline of every member who is not a guest,
    /// whether or not they joined the channel.
    #[tokio::test]
    async fn a_public_channel_message_fires_every_members_pipeline() {
        let mut world = World::new();
        let owner = world.member(ServerMemberRole::Owner);
        let moderator = world.member(ServerMemberRole::Moderator);
        let member = world.member(ServerMemberRole::Member);
        let channel = world.conversation(ConversationType::Channel, false, &[owner]);
        let pipelines: Vec<Pipeline> = [owner, moderator, member]
            .iter()
            .map(|u| world.pipeline(*u))
            .collect();
        let h = world.harness(pipelines.clone());

        let fired = h.fired(world.message(channel, "goodbye")).await;

        assert_eq!(fired, ids(&pipelines));
        assert_eq!(h.commands(), 3);
    }

    /// A DM or group message fires only its participants' pipelines; the workspace's Owner is
    /// no exception.
    #[tokio::test]
    async fn a_dm_or_group_message_fires_only_its_participants_pipelines() {
        for kind in [ConversationType::Direct, ConversationType::Group] {
            let mut world = World::new();
            let owner = world.member(ServerMemberRole::Owner);
            let a = world.member(ServerMemberRole::Member);
            let b = world.member(ServerMemberRole::Member);
            let conversation = world.conversation(kind, false, &[a, b]);
            let participants = [world.pipeline(a), world.pipeline(b)];
            let owners = world.pipeline(owner);
            let mut pipelines = participants.to_vec();
            pipelines.push(owners);
            let h = world.harness(pipelines);

            let fired = h.fired(world.message(conversation, "goodbye")).await;

            assert_eq!(fired, ids(&participants), "{kind}");
            assert_eq!(h.stamped().len(), 2, "{kind}");
        }
    }

    /// A guest's pipeline fires only in conversations the guest joined, a public channel
    /// included; a public channel they did not join or a DM between others fires nothing.
    #[tokio::test]
    async fn a_guest_pipeline_fires_only_where_the_guest_is_a_member() {
        let mut world = World::new();
        let guest = world.member(ServerMemberRole::Guest);
        let other = world.member(ServerMemberRole::Member);
        let third = world.member(ServerMemberRole::Member);
        let joined_public = world.conversation(ConversationType::Channel, false, &[guest]);
        let joined_private = world.conversation(ConversationType::Channel, true, &[guest]);
        let their_dm = world.conversation(ConversationType::Direct, false, &[guest, other]);
        let unjoined_public = world.conversation(ConversationType::Channel, false, &[other]);
        let others_dm = world.conversation(ConversationType::Direct, false, &[other, third]);
        let pipeline = world.pipeline(guest);
        let h = world.harness(vec![pipeline.clone()]);

        for (conversation, fires) in [
            (joined_public, true),
            (joined_private, true),
            (their_dm, true),
            (unjoined_public, false),
            (others_dm, false),
        ] {
            let fired = h.fired(world.message(conversation, "goodbye")).await;
            let expected = if fires { ids([&pipeline]) } else { vec![] };
            assert_eq!(fired, expected, "{conversation}");
        }
        assert_eq!(h.stamped().len(), 3);
    }

    /// A creator who is no longer a member of the workspace has no pipeline firing, not even on
    /// a public channel, and a message from a conversation outside the workspace fires nothing.
    #[tokio::test]
    async fn a_departed_creator_or_a_foreign_conversation_fires_nothing() {
        let mut world = World::new();
        let member = world.member(ServerMemberRole::Member);
        let channel = world.conversation(ConversationType::Channel, false, &[]);
        let departed = world.pipeline(UserId::new());
        let present = world.pipeline(member);
        let h = world.harness(vec![departed, present.clone()]);

        assert_eq!(
            h.fired(world.message(channel, "goodbye")).await,
            ids([&present])
        );
        assert!(
            h.fired(world.message(ConversationId::new(), "goodbye"))
                .await
                .is_empty()
        );
        assert_eq!(h.stamped(), [present.id]);
    }

    /// Events the creator may not see neither start the cooldown nor count towards the
    /// suspension: after more of them than the firing limit, the first visible message still
    /// fires and the pipeline stays enabled.
    #[tokio::test]
    async fn unseen_events_count_towards_neither_cooldown_nor_suspension() {
        let mut world = World::new();
        let creator = world.member(ServerMemberRole::Member);
        let other = world.member(ServerMemberRole::Member);
        let hidden = world.conversation(ConversationType::Channel, true, &[other]);
        let public = world.conversation(ConversationType::Channel, false, &[]);
        let mut pipeline = world.pipeline(creator);
        pipeline.cooldown_secs = Some(60);
        let h = world.harness(vec![pipeline.clone()]);

        for _ in 0..=RATE_LIMIT_MAX_FIRINGS {
            assert!(h.fired(world.message(hidden, "goodbye")).await.is_empty());
        }
        assert_eq!(
            h.fired(world.message(public, "goodbye")).await,
            ids([&pipeline])
        );
        assert!(h.disabled().is_empty());
        assert_eq!(h.stamped(), [pipeline.id]);
        assert_eq!(h.events().len(), 1);
    }

    /// Pipelines that share a creator cost one standing, one conversation and one membership
    /// lookup per event.
    #[tokio::test]
    async fn pipelines_sharing_a_creator_are_decided_once_per_event() {
        let mut world = World::new();
        let creator = world.member(ServerMemberRole::Member);
        let channel = world.conversation(ConversationType::Channel, true, &[creator]);
        let pipelines: Vec<Pipeline> = (0..3).map(|_| world.pipeline(creator)).collect();
        let mut repo = MockPipelineRepository::new();
        let listed = pipelines.clone();
        repo.expect_list_enabled_for_server()
            .returning(move |_| Ok(listed.clone()));
        repo.expect_set_last_fired_at()
            .times(3)
            .returning(|_, _| Ok(()));
        let mut members = MockServerMemberRepository::new();
        members
            .expect_get_member_standing()
            .times(1)
            .returning(|_, _| Ok(Some(MemberStanding::new(ServerMemberRole::Member))));
        let mut conversations = MockConversationRepository::new();
        let stored = world.conversations[&channel].clone();
        conversations
            .expect_get_by_id()
            .times(1)
            .returning(move |_| {
                let stored = stored.clone();
                Box::pin(async move { Ok(Some(stored)) })
            });
        let mut conversation_members = MockConversationMemberRepository::new();
        conversation_members
            .expect_is_member()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(true) }));
        let mut command_service = MockCommandService::new();
        command_service
            .expect_execute_command()
            .times(3)
            .returning(|_| Box::pin(async { Ok(success()) }));
        let svc = DefaultPipelineService::new(
            PipelineRepos {
                pipeline_repo: Arc::new(repo),
                server_member_repo: Arc::new(members),
                conversation_repo: Arc::new(conversations),
                conversation_member_repo: Arc::new(conversation_members),
            },
            Arc::new(command_service),
            Arc::new(rim_services::events::NoopBroadcaster),
        );

        let results = svc
            .evaluate_event(world.message(channel, "goodbye"))
            .await
            .expect("evaluate");
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.actions_executed == 1));
    }

    /// Workspace-wide events reach a pipeline as a subscriber would see them: a guest's pipeline
    /// fires when the guest joins and not when someone else does, and a state change kept for
    /// one user fires only that user's pipeline.
    #[tokio::test]
    async fn workspace_events_fire_a_pipeline_as_its_creator_would_see_them() {
        let mut world = World::new();
        let guest = world.member(ServerMemberRole::Guest);
        let member = world.member(ServerMemberRole::Member);
        let on = |kind, creator| {
            let mut pipeline = make_pipeline(world.server_id, creator);
            pipeline.trigger = PipelineTrigger {
                kind,
                conversation_id: None,
            };
            pipeline
        };
        let guest_joins = on(PipelineTriggerKind::MemberJoined, guest);
        let member_joins = on(PipelineTriggerKind::MemberJoined, member);
        let guest_state = on(PipelineTriggerKind::WorkspaceStateChanged, guest);
        let member_state = on(PipelineTriggerKind::WorkspaceStateChanged, member);
        let h = world.harness(vec![
            guest_joins.clone(),
            member_joins.clone(),
            guest_state.clone(),
            member_state.clone(),
        ]);
        let joined = |user_id| WorkspaceEvent::MemberJoined {
            workspace_id: world.server_id,
            user_id,
            username: "newcomer".to_string(),
            source: None,
        };
        let state = |scope, user_id| WorkspaceEvent::WorkspaceStateChanged {
            workspace_id: world.server_id,
            key: "k".to_string(),
            scope,
            user_id,
            version: 1,
        };

        assert_eq!(h.fired(joined(UserId::new())).await, ids([&member_joins]));
        assert_eq!(
            h.fired(joined(guest)).await,
            ids([&guest_joins, &member_joins])
        );
        assert_eq!(
            h.fired(state(WorkspaceStateScopeDto::User, Some(member)))
                .await,
            ids([&member_state])
        );
        assert_eq!(
            h.fired(state(WorkspaceStateScopeDto::Server, Some(member)))
                .await,
            ids([&guest_state, &member_state])
        );
    }

    /// When a pipeline last fired is shown to its creator only: another member listing the
    /// workspace's pipelines, or a moderator switching one off, reads none.
    #[tokio::test]
    async fn only_the_creator_reads_when_a_pipeline_last_fired() {
        let creator = UserId::new();
        let mut pipeline = make_pipeline(ServerId::new(), creator);
        pipeline.last_fired_at = Some(chrono::Utc::now());
        let mut repo = MockPipelineRepository::new();
        let stored = pipeline.clone();
        repo.expect_list_for_server()
            .returning(move |_| Ok(vec![stored.clone()]));
        let stored = pipeline.clone();
        repo.expect_get_by_id()
            .returning(move |_| Ok(Some(stored.clone())));
        let stored = pipeline.clone();
        repo.expect_set_enabled()
            .returning(move |_, _| Ok(stored.clone()));
        let mut members = make_member_repo();
        members
            .expect_get_member_standing()
            .returning(|_, _| Ok(Some(MemberStanding::new(ServerMemberRole::Moderator))));
        let svc = make_service(repo, members);

        let listed_by = |user_id| {
            svc.list(ListPipelinesCmd {
                actor: Actor::new(user_id),
                server_id: pipeline.server_id,
                enabled: None,
            })
        };
        let seen = listed_by(creator).await.expect("list");
        assert_eq!(seen[0].last_fired_at, pipeline.last_fired_at);
        let seen = listed_by(UserId::new()).await.expect("list");
        assert_eq!(seen[0].last_fired_at, None);

        let toggled = svc
            .toggle(TogglePipelineCmd {
                actor: Actor::new(UserId::new()),
                pipeline_id: pipeline.id,
                enabled: false,
            })
            .await
            .expect("toggle");
        assert_eq!(toggled.last_fired_at, None);
    }

    fn action(command_id: &str, args: Option<serde_json::Value>) -> PipelineActionDto {
        PipelineActionDto::ExecuteCommand {
            command_id: command_id.to_string(),
            args,
        }
    }

    fn create_cmd(actions: Vec<PipelineActionDto>) -> CreatePipelineCmd {
        CreatePipelineCmd {
            actor: Actor::new(UserId::new()),
            server_id: ServerId::new(),
            name: "Bounded".to_string(),
            description: None,
            trigger: PipelineTriggerDto {
                kind: PipelineTriggerKindDto::NewMessage,
                conversation_id: None,
            },
            filter: None,
            actions,
            cooldown_secs: None,
        }
    }

    #[tokio::test]
    async fn create_and_update_refuse_definitions_out_of_bounds() {
        let mut repo = MockPipelineRepository::new();
        repo.expect_create().never();
        repo.expect_update().never();
        let creator = UserId::new();
        let stored = make_pipeline(ServerId::new(), creator);
        let stored_id = stored.id;
        repo.expect_get_by_id()
            .returning(move |_| Ok(Some(stored.clone())));
        let mut members = make_member_repo();
        members
            .expect_get_member_standing()
            .returning(|_, _| Ok(Some(MemberStanding::new(ServerMemberRole::Member))));
        let svc = make_service(repo, members);
        let search = || action("chat.search_messages", None);
        let contains = |pattern: String| PipelineFilterDto {
            conditions: vec![FilterConditionDto::MessageContains { pattern }],
        };
        let cases = [
            (
                create_cmd(vec![search(); MAX_ACTIONS + 1]),
                "a pipeline may have at most 10 actions",
            ),
            (
                create_cmd(vec![action(
                    "chat.search_messages",
                    Some(serde_json::json!({ "query": "x".repeat(4096) })),
                )]),
                "a pipeline action's args may be at most 4096 bytes",
            ),
            (
                create_cmd(vec![action(&format!("chat.{}", "x".repeat(300)), None)]),
                "a pipeline action's command id may be at most 256 bytes",
            ),
            (
                create_cmd(vec![action("user.many_steps", None)]),
                COMPOSED_ACTION,
            ),
            (
                create_cmd(vec![action("macro.other", None)]),
                COMPOSED_ACTION,
            ),
            (
                CreatePipelineCmd {
                    description: Some("d".repeat(1001)),
                    ..create_cmd(vec![search()])
                },
                "a pipeline description may be at most 1000 characters",
            ),
            (
                CreatePipelineCmd {
                    filter: Some(PipelineFilterDto {
                        conditions: vec![
                            FilterConditionDto::MessageContains {
                                pattern: "bye".to_string()
                            };
                            MAX_FILTER_CONDITIONS + 1
                        ],
                    }),
                    ..create_cmd(vec![search()])
                },
                "a pipeline filter may have at most 5 conditions",
            ),
            (
                CreatePipelineCmd {
                    filter: Some(contains("p".repeat(257))),
                    ..create_cmd(vec![search()])
                },
                "a pipeline filter pattern may be at most 256 characters",
            ),
        ];
        for (cmd, refusal) in cases {
            let update = UpdatePipelineCmd {
                actor: Actor::new(creator),
                pipeline_id: stored_id,
                name: cmd.name.clone(),
                description: cmd.description.clone(),
                trigger: cmd.trigger.clone(),
                filter: cmd.filter.clone(),
                actions: cmd.actions.clone(),
                cooldown_secs: cmd.cooldown_secs,
            };
            for err in [
                svc.create(cmd).await.unwrap_err(),
                svc.update(update).await.unwrap_err(),
            ] {
                assert!(
                    matches!(&err, ServiceError::Validation(m) if m == refusal),
                    "{err:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn create_and_update_refuse_an_unknown_command() {
        let mut repo = MockPipelineRepository::new();
        repo.expect_create().never();
        repo.expect_update().never();
        let creator = UserId::new();
        let stored = make_pipeline(ServerId::new(), creator);
        let stored_id = stored.id;
        repo.expect_get_by_id()
            .returning(move |_| Ok(Some(stored.clone())));
        let mut members = make_member_repo();
        members
            .expect_get_member_standing()
            .returning(|_, _| Ok(Some(MemberStanding::new(ServerMemberRole::Member))));
        let svc = make_service(repo, members);

        let err = svc
            .create(create_cmd(vec![action("chat.nope", None)]))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "Unknown command: chat.nope"),
            "{err:?}"
        );

        let cmd = create_cmd(vec![action("plugin.dev.example.x", None)]);
        let err = svc
            .update(UpdatePipelineCmd {
                actor: Actor::new(creator),
                pipeline_id: stored_id,
                name: cmd.name,
                description: cmd.description,
                trigger: cmd.trigger,
                filter: cmd.filter,
                actions: cmd.actions,
                cooldown_secs: cmd.cooldown_secs,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "Unknown command: plugin.dev.example.x"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_firing_runs_at_most_the_action_cap_and_skips_composed_commands() {
        let mut world = World::new();
        let creator = world.member(ServerMemberRole::Member);
        let channel = world.conversation(ConversationType::Channel, false, &[]);
        let mut pipeline = make_pipeline(world.server_id, creator);
        pipeline.actions = (0..MAX_ACTIONS + 5)
            .map(|i| PipelineAction::ExecuteCommand {
                command_id: if i == 0 {
                    "user.many_steps".to_string()
                } else {
                    "test.cmd".to_string()
                },
                args: None,
            })
            .collect();
        let h = world.harness(vec![pipeline]);

        let results = h
            .svc
            .evaluate_event(world.message(channel, "hello"))
            .await
            .expect("evaluate");

        assert_eq!(h.commands(), MAX_ACTIONS - 1);
        assert_eq!(results[0].actions_executed, MAX_ACTIONS - 1);
    }
}
