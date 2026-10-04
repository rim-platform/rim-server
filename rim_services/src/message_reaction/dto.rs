use rim_core::user::model::UserId;

pub const MAX_REACTION_USER_IDS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReactionGroupDto {
    pub emoji: String,
    pub count: u32,
    pub reacted_by_me: bool,
    pub user_ids: Vec<UserId>,
}
