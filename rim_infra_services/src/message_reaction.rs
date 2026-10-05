use std::collections::HashMap;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;

use rim_core::message::model::MessageId;
use rim_core::message_reaction::model::NewMessageReaction;
use rim_infra_db::TransactionScope;
use rim_infra_db::begin_transaction;
use rim_services::events::{EventObserver, Observers, ReactionGroupData, WorkspaceEvent};
use rim_services::message_reaction::command::{
    GetMessageReactionsQuery, GetReactionsForMessagesQuery, ToggleReactionCmd,
};
use rim_services::message_reaction::dto::ReactionGroupDto;
use rim_services::message_reaction::service::MessageReactionService;
use rim_services::{ServiceError, ServiceResult};

use crate::ReactionRepos;
use crate::repos::AppRepos;
use crate::util::{
    ConversationAccessRepos, ensure_conversation_in_server, ensure_conversation_member_access,
    ensure_conversation_writable,
};

#[derive(Clone)]
pub struct DefaultMessageReactionService {
    repos: ReactionRepos,
    db: Option<DatabaseConnection>,
    observers: Observers,
}

impl DefaultMessageReactionService {
    pub fn new(repos: ReactionRepos, db: Option<DatabaseConnection>, observers: Observers) -> Self {
        Self {
            repos,
            db,
            observers,
        }
    }

    async fn begin_write(&self) -> ServiceResult<(ReactionRepos, Option<TransactionScope>)> {
        if let Some(db) = &self.db {
            let scope = begin_transaction(db)
                .await
                .map_err(ServiceError::internal)?;
            let repos = AppRepos::with_handle(&scope.handle()).for_reaction();
            Ok((repos, Some(scope)))
        } else {
            Ok((self.repos.clone(), None))
        }
    }

    async fn commit_if_present(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    fn group_reactions(
        reactions: Vec<rim_core::message_reaction::model::MessageReaction>,
        actor_user_id: rim_core::user::model::UserId,
    ) -> Vec<ReactionGroupDto> {
        use std::collections::BTreeMap;

        use rim_services::message_reaction::dto::MAX_REACTION_USER_IDS;

        let mut groups: BTreeMap<String, (u32, bool, Vec<rim_core::user::model::UserId>)> =
            BTreeMap::new();
        for r in &reactions {
            let entry = groups
                .entry(r.emoji.clone())
                .or_insert((0, false, Vec::new()));
            entry.0 += 1;
            if r.user_id == actor_user_id {
                entry.1 = true;
            }
            entry.2.push(r.user_id);
        }

        groups
            .into_iter()
            .map(|(emoji, (count, reacted_by_me, user_ids))| {
                let mut user_ids = user_ids;
                user_ids.truncate(MAX_REACTION_USER_IDS);
                ReactionGroupDto {
                    emoji,
                    count,
                    reacted_by_me,
                    user_ids,
                }
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
        ensure_conversation_writable(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            actor_user_id,
            server_id,
            conversation_id,
        )
        .await?;

        let message = self
            .repos
            .message_repo
            .get_by_id(message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if message.conversation_id != conversation_id {
            return Err(ServiceError::NotFound);
        }

        if message.is_deleted {
            return Err(ServiceError::Validation(
                "cannot react to a deleted message".to_string(),
            ));
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
        if emoji.chars().count() > 32 {
            return Err(ServiceError::Validation("emoji too long".to_string()));
        }

        self.ensure_actor_can_react(
            cmd.server_id,
            cmd.conversation_id,
            cmd.message_id,
            cmd.actor.user_id,
        )
        .await?;

        // Run find + add/remove atomically inside a transaction.
        let (repos, scope) = self.begin_write().await?;

        let existing = repos
            .message_reaction_repo
            .find_reaction(cmd.message_id, cmd.actor.user_id, &emoji)
            .await?;

        if existing.is_some() {
            repos
                .message_reaction_repo
                .remove_reaction(cmd.message_id, cmd.actor.user_id, &emoji)
                .await?;
        } else {
            repos
                .message_reaction_repo
                .add_reaction(NewMessageReaction {
                    message_id: cmd.message_id,
                    user_id: cmd.actor.user_id,
                    emoji,
                })
                .await?;
        }

        let all = repos
            .message_reaction_repo
            .get_reactions_for_message(cmd.message_id)
            .await?;

        Self::commit_if_present(scope).await?;

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
            .await?;

        self.observers.emit(WorkspaceEvent::ReactionsUpdated {
            workspace_id: cmd.server_id,
            conversation_id: cmd.conversation_id,
            conversation_type: conv.conversation_type.as_str().to_string(),
            is_private: conv.is_private,
            member_user_ids,
            message_id: cmd.message_id,
            reactions: groups
                .iter()
                .map(|g| ReactionGroupData {
                    emoji: g.emoji.clone(),
                    count: g.count,
                    user_ids: g.user_ids.clone(),
                })
                .collect(),
        });

        Ok(groups)
    }

    async fn get_reactions(
        &self,
        query: GetMessageReactionsQuery,
    ) -> ServiceResult<Vec<ReactionGroupDto>> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;

        let message = self
            .repos
            .message_repo
            .get_by_id(query.message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if message.conversation_id != query.conversation_id {
            return Err(ServiceError::NotFound);
        }

        let all = self
            .repos
            .message_reaction_repo
            .get_reactions_for_message(query.message_id)
            .await?;

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
            .await?;

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
