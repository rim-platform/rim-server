use crate::user_notification;
use rim_core::notification::model::UserNotification;

impl From<user_notification::Model> for UserNotification {
    fn from(m: user_notification::Model) -> Self {
        Self {
            id: m.id.into(),
            user_id: m.user_id.into(),
            server_id: m.server_id.into(),
            conversation_id: m.conversation_id.into(),
            message_id: m.message_id.into(),
            reason: m.reason.into(),
            created_at: m.created_at.into(),
            read_at: m.read_at.map(Into::into),
            emailed_at: m.emailed_at.map(Into::into),
        }
    }
}
