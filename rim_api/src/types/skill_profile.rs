use async_graphql::{Enum, SimpleObject};

// ── Skill Profile ────────────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SkillTier {
    Beginner,
    Intermediate,
    Advanced,
}

impl From<rim_core::skill_profile::model::SkillTier> for SkillTier {
    fn from(value: rim_core::skill_profile::model::SkillTier) -> Self {
        use rim_core::skill_profile::model::SkillTier as D;
        match value {
            D::Beginner => Self::Beginner,
            D::Intermediate => Self::Intermediate,
            D::Advanced => Self::Advanced,
        }
    }
}

impl From<SkillTier> for rim_core::skill_profile::model::SkillTier {
    fn from(value: SkillTier) -> Self {
        match value {
            SkillTier::Beginner => Self::Beginner,
            SkillTier::Intermediate => Self::Intermediate,
            SkillTier::Advanced => Self::Advanced,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct FeatureUsage {
    pub feature_id: String,
    pub first_used_at: chrono::DateTime<chrono::Utc>,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct SkillProfileGql {
    pub user_id: async_graphql::ID,
    pub features_used: Vec<FeatureUsage>,
    pub skill_tier: SkillTier,
}

impl From<rim_services::skill_profile::dto::SkillProfileDto> for SkillProfileGql {
    fn from(value: rim_services::skill_profile::dto::SkillProfileDto) -> Self {
        let mut features_used: Vec<FeatureUsage> = value
            .features_used
            .into_iter()
            .map(|(feature_id, first_used_at)| FeatureUsage {
                feature_id,
                first_used_at,
            })
            .collect();
        features_used.sort_by_key(|f| f.first_used_at);

        Self {
            user_id: async_graphql::ID(value.user_id.to_string()),
            features_used,
            skill_tier: value.skill_tier.into(),
        }
    }
}
