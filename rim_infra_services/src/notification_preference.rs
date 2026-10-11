use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::instrument;

use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::notification_preference::repository::NotificationPreferenceRepository;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_services::notification_preference::command::{
    IsMutedQuery, ListMutedConversationsQuery, MuteConversationCmd, UnmuteConversationCmd,
};
use rim_services::notification_preference::dto::MutedConversationDto;
use rim_services::notification_preference::service::NotificationPreferenceService;
use rim_services::{ServiceError, ServiceResult};

use crate::util::{
    ConversationAccessRepos, ensure_conversation_member_access, ensure_server_member,
};

#[derive(Clone)]
pub struct NotificationPreferenceRepos {
    pub notification_preference_repo: Arc<dyn NotificationPreferenceRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
}

pub struct DefaultNotificationPreferenceService {
    repos: NotificationPreferenceRepos,
}

impl DefaultNotificationPreferenceService {
    pub fn new(repos: NotificationPreferenceRepos) -> Self {
        Self { repos }
    }

    /// The read gate of the conversation a mute names.
    async fn ensure_readable(
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

#[async_trait]
impl NotificationPreferenceService for DefaultNotificationPreferenceService {
    #[instrument(name = "notification.mute_conversation", skip_all, fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id))]
    async fn mute_conversation(&self, cmd: MuteConversationCmd) -> ServiceResult<bool> {
        self.ensure_readable(cmd.actor.user_id, cmd.server_id, cmd.conversation_id)
            .await?;

        self.repos
            .notification_preference_repo
            .set_mute(cmd.actor.user_id, cmd.conversation_id, cmd.muted_until)
            .await?;

        Ok(true)
    }

    #[instrument(name = "notification.unmute_conversation", skip_all, fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id))]
    async fn unmute_conversation(&self, cmd: UnmuteConversationCmd) -> ServiceResult<bool> {
        self.ensure_readable(cmd.actor.user_id, cmd.server_id, cmd.conversation_id)
            .await?;

        self.repos
            .notification_preference_repo
            .remove_mute(cmd.actor.user_id, cmd.conversation_id)
            .await?;

        Ok(true)
    }

    async fn is_muted(&self, query: IsMutedQuery) -> ServiceResult<bool> {
        self.repos
            .notification_preference_repo
            .is_muted(query.actor.user_id, query.conversation_id)
            .await
            .map_err(ServiceError::from)
    }

    #[instrument(name = "notification.list_muted", skip_all, fields(actor_id = %query.actor.user_id, server_id = %query.server_id))]
    async fn list_muted_conversations(
        &self,
        query: ListMutedConversationsQuery,
    ) -> ServiceResult<Vec<MutedConversationDto>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let prefs = self
            .repos
            .notification_preference_repo
            .get_muted_conversations(query.actor.user_id)
            .await?;
        let ids: Vec<_> = prefs.iter().map(|p| p.conversation_id).collect();
        let in_workspace: HashSet<_> = self
            .repos
            .conversation_repo
            .get_by_ids(&ids)
            .await?
            .into_iter()
            .filter(|c| c.server_id == query.server_id)
            .map(|c| c.id)
            .collect();

        Ok(prefs
            .into_iter()
            .filter(|p| in_workspace.contains(&p.conversation_id))
            .map(|p| MutedConversationDto {
                conversation_id: p.conversation_id,
                muted_until: p.muted_until,
            })
            .collect())
    }
}
