use rim_core::user::model::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationParticipants {
    DirectMessage { other_user_id: UserId },

    Group { member_ids: Vec<UserId> },

    Channel,
}
