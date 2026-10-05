use async_trait::async_trait;
use chrono::Utc;

use rim_services::ServiceResult;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::read_state::dto::{
    MarkAllAsReadCmd, MarkAsReadCmd, UnreadCountDto, UnreadCountsQuery,
};
use rim_services::read_state::service::ReadStateService;

use crate::ReadStateRepos;
use crate::util::{
    ConversationAccessRepos, ensure_conversation_member_access, ensure_server_member,
};

#[derive(Clone)]
pub struct DefaultReadStateService {
    repos: ReadStateRepos,
    observers: Observers,
}

impl DefaultReadStateService {
    pub fn new(repos: ReadStateRepos, observers: Observers) -> Self {
        Self { repos, observers }
    }
}

#[async_trait]
impl ReadStateService for DefaultReadStateService {
    async fn mark_as_read(&self, cmd: MarkAsReadCmd) -> ServiceResult<()> {
        // Clamp read_at: never allow a future timestamp.
        let now = Utc::now();
        let read_at = cmd.read_at.min(now);

        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.workspace_id,
            cmd.conversation_id,
        )
        .await?;

        // Upsert with forward-only guard (handled at DB level).
        self.repos
            .read_state_repo
            .upsert(cmd.actor.user_id, cmd.conversation_id, read_at)
            .await?;

        self.observers.emit(WorkspaceEvent::ReadStateUpdated {
            workspace_id: cmd.workspace_id,
            conversation_id: cmd.conversation_id,
            user_id: cmd.actor.user_id,
            last_read_at: read_at,
        });

        Ok(())
    }

    async fn mark_all_as_read(&self, cmd: MarkAllAsReadCmd) -> ServiceResult<()> {
        let now = Utc::now();

        // Verify server membership.
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.workspace_id,
            cmd.actor.user_id,
        )
        .await?;

        // Conversations the user is a member of — the same set the unread counts cover. Public
        // channels they can read but never joined have no unread state to clear.
        let conversation_ids = self
            .repos
            .conversation_repo
            .list_joined_conversation_ids(cmd.workspace_id, cmd.actor.user_id)
            .await?;

        if conversation_ids.is_empty() {
            return Ok(());
        }

        // Bulk upsert with forward-only guard.
        self.repos
            .read_state_repo
            .mark_all_read(cmd.actor.user_id, &conversation_ids, now)
            .await?;

        for conv_id in &conversation_ids {
            self.observers.emit(WorkspaceEvent::ReadStateUpdated {
                workspace_id: cmd.workspace_id,
                conversation_id: *conv_id,
                user_id: cmd.actor.user_id,
                last_read_at: now,
            });
        }

        Ok(())
    }

    async fn get_unread_counts(
        &self,
        query: UnreadCountsQuery,
    ) -> ServiceResult<Vec<UnreadCountDto>> {
        // Verify server membership.
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.workspace_id,
            query.actor.user_id,
        )
        .await?;

        // Only conversations the user is a member of (joined public channels, private
        // channels, DMs, groups): unjoined public channels are readable but not followed, so
        // they carry no unread badge (Track 1.1).
        let conversation_ids = self
            .repos
            .conversation_repo
            .list_joined_conversation_ids(query.workspace_id, query.actor.user_id)
            .await?;

        if conversation_ids.is_empty() {
            return Ok(vec![]);
        }

        // Batch query unread counts.
        let counts = self
            .repos
            .read_state_repo
            .get_unread_counts(query.actor.user_id, &conversation_ids)
            .await?;

        Ok(counts
            .into_iter()
            .map(|(conversation_id, count)| UnreadCountDto {
                conversation_id,
                count,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::ConversationId;
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::conversation_read_state::repository::MockConversationReadStateRepository;
    use rim_core::server::model::ServerId;
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::events::NoopBroadcaster;
    use rim_services::read_state::dto::{MarkAllAsReadCmd, UnreadCountsQuery};
    use rim_services::read_state::service::ReadStateService;

    use super::DefaultReadStateService;
    use crate::ReadStateRepos;

    /// Track 1.1: unread counts and mark-all-read cover only the conversations the user is a
    /// member of — never every public channel they could read.
    fn service(
        joined: Vec<ConversationId>,
        read_state_repo: MockConversationReadStateRepository,
    ) -> DefaultReadStateService {
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_list_joined_conversation_ids()
            .returning(move |_, _| {
                let joined = joined.clone();
                Box::pin(async move { Ok(joined) })
            });
        conversation_repo
            .expect_list_accessible_conversation_ids()
            .never();
        DefaultReadStateService::new(
            ReadStateRepos {
                read_state_repo: Arc::new(read_state_repo),
                conversation_repo: Arc::new(conversation_repo),
                server_member_repo: Arc::new(server_member_repo),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            },
            rim_services::events::Observers::new(Arc::new(NoopBroadcaster)),
        )
    }

    #[tokio::test]
    async fn unread_counts_cover_joined_conversations_only() {
        let joined = vec![ConversationId::new(), ConversationId::new()];
        let expected = joined.clone();
        let mut read_state_repo = MockConversationReadStateRepository::new();
        read_state_repo
            .expect_get_unread_counts()
            .withf(move |_, ids| ids == expected.as_slice())
            .returning(|_, ids| {
                let counts = ids.iter().map(|id| (*id, 1)).collect();
                Box::pin(async move { Ok(counts) })
            });

        let counts = service(joined.clone(), read_state_repo)
            .get_unread_counts(UnreadCountsQuery {
                actor: Actor::new(UserId::new()),
                workspace_id: ServerId::new(),
            })
            .await
            .expect("unread counts");
        let ids: Vec<_> = counts.iter().map(|c| c.conversation_id).collect();
        assert_eq!(ids, joined);
    }

    #[tokio::test]
    async fn unread_counts_empty_without_memberships() {
        let mut read_state_repo = MockConversationReadStateRepository::new();
        read_state_repo.expect_get_unread_counts().never();
        let counts = service(vec![], read_state_repo)
            .get_unread_counts(UnreadCountsQuery {
                actor: Actor::new(UserId::new()),
                workspace_id: ServerId::new(),
            })
            .await
            .expect("unread counts");
        assert!(counts.is_empty());
    }

    #[tokio::test]
    async fn mark_all_as_read_covers_joined_conversations_only() {
        let joined = vec![ConversationId::new()];
        let expected = joined.clone();
        let mut read_state_repo = MockConversationReadStateRepository::new();
        read_state_repo
            .expect_mark_all_read()
            .withf(move |_, ids, _| ids == expected.as_slice())
            .returning(|_, _, _| Box::pin(async { Ok(()) }));
        service(joined, read_state_repo)
            .mark_all_as_read(MarkAllAsReadCmd {
                actor: Actor::new(UserId::new()),
                workspace_id: ServerId::new(),
            })
            .await
            .expect("mark all as read");
    }
}
