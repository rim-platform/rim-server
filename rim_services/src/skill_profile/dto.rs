use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rim_core::skill_profile::model::{SkillProfile, SkillTier};
use rim_core::user::model::UserId;

use crate::Actor;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SkillProfileDto {
    pub user_id: UserId,
    pub features_used: HashMap<String, DateTime<Utc>>,
    pub skill_tier: SkillTier,
}

impl From<SkillProfile> for SkillProfileDto {
    fn from(p: SkillProfile) -> Self {
        Self {
            user_id: p.user_id,
            features_used: p.features_used,
            skill_tier: p.skill_tier,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecordFeatureUsageCmd {
    pub actor: Actor,
    pub feature_id: String,
}

#[derive(Debug, Clone)]
pub struct OverrideSkillTierCmd {
    pub actor: Actor,
    pub tier: SkillTier,
}
