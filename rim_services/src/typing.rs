use rim_core::conversation::model::ConversationId;
use rim_core::user::model::UserId;

pub trait TypingService: Send + Sync {
    fn should_broadcast(&self, user_id: UserId, conversation_id: ConversationId) -> bool;
}
