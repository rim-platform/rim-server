use async_trait::async_trait;

use crate::auth::model::{Invite, InviteId, NewInvite, NewSession, Session, SessionId};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn get_by_id(&self, id: SessionId) -> RepoResult<Option<Session>>;
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Session>>;
    async fn create_session(&self, session: NewSession) -> RepoResult<Session>;
    async fn touch_last_seen_at(&self, id: SessionId) -> RepoResult<()>;
    async fn revoke_session(&self, id: SessionId) -> RepoResult<()>;
    async fn revoke_all_for_user(&self, user_id: UserId) -> RepoResult<u64>;
}

#[async_trait]
pub trait InviteRepository: Send + Sync {
    async fn get_by_id(&self, id: InviteId) -> RepoResult<Option<Invite>>;
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Invite>>;
    async fn create_invite(&self, invite: NewInvite) -> RepoResult<Invite>;
    async fn increment_use_count(&self, id: InviteId) -> RepoResult<()>;
    async fn revoke_invite(&self, id: InviteId) -> RepoResult<()>;
}
