use crate::notification_preference;
use rim_core::notification_preference::model::NotificationPreference;

impl From<notification_preference::Model> for NotificationPreference {
    fn from(m: notification_preference::Model) -> Self {
        Self {
            user_id: m.user_id.into(),
            conversation_id: m.conversation_id.into(),
            muted_until: m.muted_until.map(Into::into),
        }
    }
}
