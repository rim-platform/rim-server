use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::principal::model::{Principal, PrincipalId};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait PrincipalRepository: Send + Sync {
    async fn get_by_id(&self, id: PrincipalId) -> RepoResult<Option<Principal>>;
    async fn get_user_principal(&self, user_id: UserId) -> RepoResult<Option<Principal>>;
    async fn upsert_principal(&self, principal: Principal) -> RepoResult<Principal>;
}
