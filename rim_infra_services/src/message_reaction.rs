use std::collections::HashMap;

use async_trait::async_trait;

use rim_core::message::model::MessageId;
use rim_core::message_reaction::model::NewMessageReaction;
use rim_services::events::{Observers, ReactionGroupData};
use rim_services::message_reaction::command::{
    GetMessageReactionsQuery, GetReactionsForMessagesQuery, ToggleReactionCmd,
};
use rim_services::message_reaction::dto::ReactionGroupDto;
use rim_services::message_reaction::service::MessageReactionService;
use rim_services::{ServiceError, ServiceResult};

use crate::ReactionRepos;
use crate::util::{
    ensure_conversation_access, ensure_conversation_in_server, ensure_server_member,
};

#[derive(Clone)]
pub struct DefaultMessageReactionService {
    repos: ReactionRepos,
    observers: Observers,
}

impl DefaultMessageReactionService {
    pub fn new(repos: ReactionRepos, observers: Observers) -> Self {
        Self { repos, observers }
    }

    fn group_reactions(
        reactions: Vec<rim_core::message_reaction::model::MessageReaction>,
        actor_user_id: rim_core::user::model::UserId,
    ) -> Vec<ReactionGroupDto> {
        use std::collections::BTreeMap;

        let mut groups: BTreeMap<String, (u32, bool)> = BTreeMap::new();
        for r in &reactions {
            let entry = groups.entry(r.emoji.clone()).or_insert((0, false));
            entry.0 += 1;
            if r.user_id == actor_user_id {
                entry.1 = true;
            }
        }

        groups
            .into_iter()
            .map(|(emoji, (count, reacted_by_me))| ReactionGroupDto {
                emoji,
                count,
                reacted_by_me,
            })
            .collect()
    }

    async fn ensure_actor_can_react(
        &self,
        server_id: rim_core::server::model::ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
        message_id: rim_core::message::model::MessageId,
        actor_user_id: rim_core::user::model::UserId,
    ) -> ServiceResult<()> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            server_id,
            actor_user_id,
        )
        .await?;

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            conversation_id,
            server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            actor_user_id,
        )
        .await?;

        let message = self
            .repos
            .message_repo
            .get_by_id(message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if message.conversation_id != conversation_id {
            return Err(ServiceError::NotFound);
        }

        Ok(())
    }
}

#[async_trait]
impl MessageReactionService for DefaultMessageReactionService {
    async fn toggle_reaction(
        &self,
        cmd: ToggleReactionCmd,
    ) -> ServiceResult<Vec<ReactionGroupDto>> {
        let emoji = cmd.emoji.trim().to_string();
        if emoji.is_empty() {
            return Err(ServiceError::Validation(
                "emoji must be non-empty".to_string(),
            ));
        }

        self.ensure_actor_can_react(
            cmd.server_id,
            cmd.conversation_id,
            cmd.message_id,
            cmd.actor.user_id,
        )
        .await?;

        let existing = self
            .repos
            .message_reaction_repo
            .find_reaction(cmd.message_id, cmd.actor.user_id, &emoji)
            .await
            .map_err(rim_services::map_repo_error)?;

        if existing.is_some() {
            self.repos
                .message_reaction_repo
                .remove_reaction(cmd.message_id, cmd.actor.user_id, &emoji)
                .await
                .map_err(rim_services::map_repo_error)?;
        } else {
            self.repos
                .message_reaction_repo
                .add_reaction(NewMessageReaction {
                    message_id: cmd.message_id,
                    user_id: cmd.actor.user_id,
                    emoji,
                })
                .await
                .map_err(rim_services::map_repo_error)?;
        }

        let all = self
            .repos
            .message_reaction_repo
            .get_reactions_for_message(cmd.message_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        let groups = Self::group_reactions(all, cmd.actor.user_id);

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        let member_user_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(cmd.conversation_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();

        self.observers.reaction.on_reactions_changed(
            &cmd.server_id.to_string(),
            &cmd.conversation_id.to_string(),
            conv.conversation_type.as_str(),
            member_user_ids,
            &cmd.message_id.to_string(),
            groups
                .iter()
                .map(|g| ReactionGroupData {
                    emoji: g.emoji.clone(),
                    count: g.count,
                })
                .collect(),
        );

        Ok(groups)
    }

    async fn get_reactions(
        &self,
        query: GetMessageReactionsQuery,
    ) -> ServiceResult<Vec<ReactionGroupDto>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            query.conversation_id,
            query.server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            query.actor.user_id,
        )
        .await?;

        let message = self
            .repos
            .message_repo
            .get_by_id(query.message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if message.conversation_id != query.conversation_id {
            return Err(ServiceError::NotFound);
        }

        let all = self
            .repos
            .message_reaction_repo
            .get_reactions_for_message(query.message_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(Self::group_reactions(all, query.actor.user_id))
    }

    async fn get_reactions_for_messages(
        &self,
        query: GetReactionsForMessagesQuery,
    ) -> ServiceResult<HashMap<MessageId, Vec<ReactionGroupDto>>> {
        if query.message_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let raw = self
            .repos
            .message_reaction_repo
            .get_reactions_for_messages(&query.message_ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        let grouped = raw
            .into_iter()
            .map(|(msg_id, reactions)| {
                (
                    msg_id,
                    Self::group_reactions(reactions, query.actor.user_id),
                )
            })
            .collect();

        Ok(grouped)
    }
}
