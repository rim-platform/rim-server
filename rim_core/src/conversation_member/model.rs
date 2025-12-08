use crate::conversation::model::ConversationId;
use crate::model_id;
use crate::user::model::UserId;

model_id!(ConversationMemberId);

pub struct ConversationMember {
    pub id: ConversationMemberId,
    pub user_id: UserId,
    pub conversation_id: ConversationId,
}
