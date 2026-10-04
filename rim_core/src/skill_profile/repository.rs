use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::repository::error::RepoResult;
use crate::skill_profile::model::SkillProfile;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait SkillProfileRepository: Send + Sync {
    async fn get_or_create(&self, user_id: UserId) -> RepoResult<SkillProfile>;

    async fn update(
        &self,
        user_id: UserId,
        features_used: &std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
        skill_tier: crate::skill_profile::model::SkillTier,
    ) -> RepoResult<SkillProfile>;
}
