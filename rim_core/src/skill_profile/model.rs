use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::user::model::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillTier {
    Beginner,
    Intermediate,
    Advanced,
}

impl SkillTier {
    pub fn from_str(s: &str) -> Self {
        match s {
            "intermediate" => Self::Intermediate,
            "advanced" => Self::Advanced,
            _ => Self::Beginner,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Beginner => "beginner",
            Self::Intermediate => "intermediate",
            Self::Advanced => "advanced",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkillProfile {
    pub user_id: UserId,
    pub features_used: HashMap<String, DateTime<Utc>>,
    pub skill_tier: SkillTier,
    pub updated_at: DateTime<Utc>,
}
