use async_trait::async_trait;
use chrono::Utc;

use rim_services::ServiceResult;
use rim_services::events::Observers;
use rim_services::read_state::dto::{
    MarkAllAsReadCmd, MarkAsReadCmd, UnreadCountDto, UnreadCountsQuery,
};
use rim_services::read_state::service::ReadStateService;

use crate::ReadStateRepos;
use crate::util::{
    ensure_conversation_access, ensure_conversation_in_server, ensure_server_member,
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

        // Verify server membership.
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.workspace_id,
            cmd.actor.user_id,
        )
        .await?;

        // Verify conversation access.
        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.workspace_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            cmd.actor.user_id,
        )
        .await?;

        // Upsert with forward-only guard (handled at DB level).
        self.repos
            .read_state_repo
            .upsert(cmd.actor.user_id, cmd.conversation_id, read_at)
            .await
            .map_err(rim_services::map_repo_error)?;

        // Emit event so the user's other sessions get notified.
        self.observers.read_state.on_read_state_updated(
            cmd.workspace_id,
            cmd.conversation_id,
            cmd.actor.user_id,
            read_at,
        );

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

        // Get all accessible conversations for the user in this workspace.
        let conversation_ids = self
            .repos
            .conversation_repo
            .list_accessible_conversation_ids(cmd.workspace_id, cmd.actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        if conversation_ids.is_empty() {
            return Ok(());
        }

        // Bulk upsert with forward-only guard.
        self.repos
            .read_state_repo
            .mark_all_read(cmd.actor.user_id, &conversation_ids, now)
            .await
            .map_err(rim_services::map_repo_error)?;

        // Emit events so the user's other sessions get notified.
        for conv_id in &conversation_ids {
            self.observers.read_state.on_read_state_updated(
                cmd.workspace_id,
                *conv_id,
                cmd.actor.user_id,
                now,
            );
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

        // Get all accessible conversations.
        let conversation_ids = self
            .repos
            .conversation_repo
            .list_accessible_conversation_ids(query.workspace_id, query.actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        if conversation_ids.is_empty() {
            return Ok(vec![]);
        }

        // Batch query unread counts.
        let counts = self
            .repos
            .read_state_repo
            .get_unread_counts(query.actor.user_id, &conversation_ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(counts
            .into_iter()
            .map(|(conversation_id, count)| UnreadCountDto {
                conversation_id,
                count,
            })
            .collect())
    }
}
