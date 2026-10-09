use std::sync::Arc;

use async_trait::async_trait;

use rim_core::channel_lens::model::{
    FilterCondition, FilterField, FilterLogic, FilterOp, FilterSpec, NewChannelLens,
};
use rim_core::channel_lens::repository::ChannelLensRepository;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::message::model::Message;
use rim_core::message::repository::MessageRepository;
use rim_core::message_pin::repository::MessagePinRepository;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_services::channel_lens::command::{
    ApplyLensQuery, CreateLensCmd, DeleteLensCmd, ListLensesQuery, UpdateLensCmd,
};
use rim_services::channel_lens::dto::ChannelLensDto;
use rim_services::channel_lens::service::ChannelLensService;
use rim_services::message::dto::MessageDto;
use rim_services::{ServiceError, ServiceResult};

use crate::util::{
    ConversationAccessRepos, ensure_conversation_member_access, ensure_server_member,
};

/// Maximum number of lenses a user may own per conversation.
const MAX_LENSES_PER_USER_PER_CONVERSATION: u64 = 10;

/// Maximum name length for a lens.
const MAX_NAME_LENGTH: usize = 100;

/// Default message limit when applying a lens.
const DEFAULT_APPLY_LIMIT: u32 = 50;

#[derive(Clone)]
pub struct ChannelLensRepos {
    pub channel_lens_repo: Arc<dyn ChannelLensRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub message_pin_repo: Arc<dyn MessagePinRepository>,
}

pub struct DefaultChannelLensService {
    repos: ChannelLensRepos,
}

impl DefaultChannelLensService {
    pub fn new(repos: ChannelLensRepos) -> Self {
        Self { repos }
    }

    /// Lenses read a conversation's messages, so they need the same read gate as the messages
    /// themselves (a private channel the actor is not in is NotFound).
    async fn ensure_can_read_conversation(
        &self,
        actor: rim_core::user::model::UserId,
        server_id: rim_core::server::model::ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> ServiceResult<()> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            actor,
            server_id,
            conversation_id,
        )
        .await?;
        Ok(())
    }
}

fn validate_name(name: &str) -> ServiceResult<()> {
    if name.is_empty() || name.chars().count() > MAX_NAME_LENGTH {
        return Err(ServiceError::Validation(format!(
            "lens name must be between 1 and {} characters",
            MAX_NAME_LENGTH
        )));
    }
    Ok(())
}

fn validate_filter_spec(spec: &FilterSpec) -> ServiceResult<()> {
    if spec.conditions.is_empty() {
        return Err(ServiceError::Validation(
            "filter spec must have at least one condition".into(),
        ));
    }
    if spec.conditions.len() > 20 {
        return Err(ServiceError::Validation(
            "filter spec must not exceed 20 conditions".into(),
        ));
    }
    Ok(())
}

/// Check whether a message matches a single filter condition.
///
/// For the `IsPinned` field we need an external set of pinned message IDs.
fn matches_condition(
    msg: &Message,
    cond: &FilterCondition,
    pinned_ids: &std::collections::HashSet<rim_core::message::model::MessageId>,
) -> bool {
    match cond.field {
        FilterField::HasLinks => {
            let has_links = msg
                .text
                .as_ref()
                .is_some_and(|t| t.contains("http://") || t.contains("https://"));
            let expected = cond.value.as_bool().unwrap_or(true);
            has_links == expected
        }
        FilterField::AuthorId => match cond.op {
            FilterOp::Eq => {
                if let Some(s) = cond.value.as_str() {
                    msg.sent_by.to_string() == s
                } else {
                    false
                }
            }
            FilterOp::In => {
                if let Some(arr) = cond.value.as_array() {
                    let user_str = msg.sent_by.to_string();
                    arr.iter().any(|v| v.as_str() == Some(user_str.as_str()))
                } else {
                    false
                }
            }
            _ => false,
        },
        FilterField::HasAttachments => {
            let expected = cond.value.as_bool().unwrap_or(true);
            msg.has_attachments == expected
        }
        FilterField::TextContains => {
            if let (Some(text), Some(pattern)) = (&msg.text, cond.value.as_str()) {
                text.to_lowercase().contains(&pattern.to_lowercase())
            } else {
                false
            }
        }
        FilterField::IsPinned => {
            let expected = cond.value.as_bool().unwrap_or(true);
            let is_pinned = pinned_ids.contains(&msg.id);
            is_pinned == expected
        }
    }
}

fn matches_filter(
    msg: &Message,
    spec: &FilterSpec,
    pinned_ids: &std::collections::HashSet<rim_core::message::model::MessageId>,
) -> bool {
    match spec.logic {
        FilterLogic::And => spec
            .conditions
            .iter()
            .all(|c| matches_condition(msg, c, pinned_ids)),
        FilterLogic::Or => spec
            .conditions
            .iter()
            .any(|c| matches_condition(msg, c, pinned_ids)),
    }
}

#[async_trait]
impl ChannelLensService for DefaultChannelLensService {
    async fn create_lens(&self, cmd: CreateLensCmd) -> ServiceResult<ChannelLensDto> {
        self.ensure_can_read_conversation(cmd.actor.user_id, cmd.server_id, cmd.conversation_id)
            .await?;

        validate_name(&cmd.name)?;

        let filter_spec: FilterSpec = cmd.filter_spec.into();
        validate_filter_spec(&filter_spec)?;

        // Enforce per-user-per-conversation limit
        let count = self
            .repos
            .channel_lens_repo
            .count_user_lenses(cmd.conversation_id, cmd.actor.user_id)
            .await?;

        if count >= MAX_LENSES_PER_USER_PER_CONVERSATION {
            return Err(ServiceError::Validation(format!(
                "maximum of {} lenses per conversation reached",
                MAX_LENSES_PER_USER_PER_CONVERSATION
            )));
        }

        let lens = self
            .repos
            .channel_lens_repo
            .create(NewChannelLens {
                server_id: cmd.server_id,
                conversation_id: cmd.conversation_id,
                created_by: cmd.actor.user_id,
                name: cmd.name,
                filter_spec,
                is_shared: cmd.is_shared,
            })
            .await?;

        Ok(ChannelLensDto::from(lens))
    }

    async fn update_lens(&self, cmd: UpdateLensCmd) -> ServiceResult<ChannelLensDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let mut lens = self
            .repos
            .channel_lens_repo
            .get_by_id(cmd.lens_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        // Verify the lens belongs to the server the caller claims
        if lens.server_id != cmd.server_id {
            return Err(ServiceError::NotFound);
        }

        // Only the creator can update
        if lens.created_by != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        if let Some(name) = cmd.name {
            validate_name(&name)?;
            lens.name = name;
        }

        if let Some(spec_dto) = cmd.filter_spec {
            let spec: FilterSpec = spec_dto.into();
            validate_filter_spec(&spec)?;
            lens.filter_spec = spec;
        }

        if let Some(is_shared) = cmd.is_shared {
            lens.is_shared = is_shared;
        }

        lens.updated_at = chrono::Utc::now();

        let updated = self.repos.channel_lens_repo.update(&lens).await?;

        Ok(ChannelLensDto::from(updated))
    }

    async fn delete_lens(&self, cmd: DeleteLensCmd) -> ServiceResult<()> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let lens = self
            .repos
            .channel_lens_repo
            .get_by_id(cmd.lens_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        // Verify the lens belongs to the server the caller claims
        if lens.server_id != cmd.server_id {
            return Err(ServiceError::NotFound);
        }

        // Only the creator can delete
        if lens.created_by != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        self.repos.channel_lens_repo.delete(cmd.lens_id).await?;

        Ok(())
    }

    async fn list_lenses(&self, query: ListLensesQuery) -> ServiceResult<Vec<ChannelLensDto>> {
        self.ensure_can_read_conversation(
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;

        let lenses = self
            .repos
            .channel_lens_repo
            .list_for_conversation(query.conversation_id, query.actor.user_id)
            .await?;

        Ok(lenses.into_iter().map(Into::into).collect())
    }

    async fn apply_lens(&self, query: ApplyLensQuery) -> ServiceResult<Vec<MessageDto>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let lens = self
            .repos
            .channel_lens_repo
            .get_by_id(query.lens_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        // Ensure the lens belongs to this conversation and is accessible
        if lens.conversation_id != query.conversation_id {
            return Err(ServiceError::NotFound);
        }
        if !lens.is_shared && lens.created_by != query.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        self.ensure_can_read_conversation(
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;

        let limit = query.limit.clamp(1, DEFAULT_APPLY_LIMIT);

        // Fetch a batch of recent messages to filter.
        // We fetch a larger batch and then apply the filter in-memory.
        let fetch_limit = limit * 10;
        let params = rim_core::message::repository::ListMessagesParams {
            conversation_id: query.conversation_id,
            thread_id: None,
            limit: fetch_limit,
            dir: rim_core::repository::pagination::PageDir::Before,
            cursor: None,
            include_deleted: false,
            order: rim_core::repository::pagination::SortOrder::Desc,
        };

        let page = self.repos.message_repo.list_messages(params).await?;

        let messages = page.items;

        // Determine if we need pinned IDs for filtering
        let needs_pinned = lens
            .filter_spec
            .conditions
            .iter()
            .any(|c| c.field == FilterField::IsPinned);

        let pinned_ids: std::collections::HashSet<rim_core::message::model::MessageId> =
            if needs_pinned {
                let pins = self
                    .repos
                    .message_pin_repo
                    .list_pinned(query.conversation_id)
                    .await?;
                pins.into_iter().map(|p| p.message_id).collect()
            } else {
                std::collections::HashSet::new()
            };

        let filtered: Vec<Message> = messages
            .into_iter()
            .filter(|msg| matches_filter(msg, &lens.filter_spec, &pinned_ids))
            .take(limit as usize)
            .collect();

        let dtos = filtered
            .into_iter()
            .map(|msg| to_simple_dto(msg, query.server_id))
            .collect();

        Ok(dtos)
    }
}

/// Convert a Message to a simple MessageDto without loading attachments/threads.
/// For lens results we keep it lightweight.
fn to_simple_dto(msg: Message, server_id: rim_core::server::model::ServerId) -> MessageDto {
    MessageDto {
        id: msg.id,
        server_id,
        conversation_id: msg.conversation_id,
        thread_id: msg.thread_id,
        root_thread_id: None,
        author_id: msg.sent_by,
        text: msg.text,
        has_attachments: msg.has_attachments,
        attachments: Vec::new(),
        parent_message_id: msg.parent_message_id,
        forwarded_from_message_id: msg.forwarded_from_message_id,
        thread_root_message_id: None,
        is_deleted: msg.is_deleted,
        sent_at: msg.sent_at,
        encryption: None,
    }
}
