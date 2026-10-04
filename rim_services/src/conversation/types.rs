use rim_core::user::model::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ConversationParticipants {
    DirectMessage { other_user_id: UserId },

    Group { member_ids: Vec<UserId> },

    Channel,
}
