use std::collections::HashMap;

use async_trait::async_trait;
use entity::user_skill_profile;
use rim_core::repository::error::RepoResult;
use rim_core::skill_profile::model::{SkillProfile, SkillTier};
use rim_core::skill_profile::repository::SkillProfileRepository;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmSkillProfileRepository {
    pub db: DbHandle,
}

impl SeaOrmSkillProfileRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

fn features_to_json(
    features: &HashMap<String, chrono::DateTime<chrono::Utc>>,
) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = features
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.to_rfc3339())))
        .collect();
    serde_json::Value::Object(map)
}

#[async_trait]
impl SkillProfileRepository for SeaOrmSkillProfileRepository {
    async fn get_or_create(&self, user_id: UserId) -> RepoResult<SkillProfile> {
        let uuid: Uuid = user_id.into();
        let existing = user_skill_profile::Entity::find()
            .filter(user_skill_profile::Column::UserId.eq(uuid))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        if let Some(model) = existing {
            return Ok(model.into());
        }

        let now = chrono::Utc::now();
        let active = user_skill_profile::ActiveModel {
            user_id: Set(uuid),
            features_used: Set(serde_json::json!({})),
            skill_tier: Set("beginner".to_string()),
            updated_at: Set(now.into()),
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn update(
        &self,
        user_id: UserId,
        features_used: &HashMap<String, chrono::DateTime<chrono::Utc>>,
        skill_tier: SkillTier,
    ) -> RepoResult<SkillProfile> {
        let uuid: Uuid = user_id.into();
        let now = chrono::Utc::now();

        let active = user_skill_profile::ActiveModel {
            user_id: Set(uuid),
            features_used: Set(features_to_json(features_used)),
            skill_tier: Set(skill_tier.as_str().to_string()),
            updated_at: Set(now.into()),
        };

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }
}
