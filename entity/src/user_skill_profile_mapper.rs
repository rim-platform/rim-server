use std::collections::HashMap;

use crate::user_skill_profile;
use rim_core::skill_profile::model::{SkillProfile, SkillTier};

impl From<user_skill_profile::Model> for SkillProfile {
    fn from(m: user_skill_profile::Model) -> Self {
        let features_used: HashMap<String, chrono::DateTime<chrono::Utc>> = m
            .features_used
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| {
                        v.as_str()
                            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
                            .map(|dt| (k.clone(), dt))
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            user_id: m.user_id.into(),
            features_used,
            skill_tier: SkillTier::from_db_str(&m.skill_tier),
            updated_at: m.updated_at.into(),
        }
    }
}
