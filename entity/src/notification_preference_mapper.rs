use crate::{notification_preference, sea_orm_active_enums};
use rim_core::notification::model::{DigestFrequency, NotificationLevel, NotificationReason};
use rim_core::notification_preference::model::NotificationPreference;

impl From<notification_preference::Model> for NotificationPreference {
    fn from(m: notification_preference::Model) -> Self {
        Self {
            user_id: m.user_id.into(),
            conversation_id: m.conversation_id.into(),
            muted_until: m.muted_until.map(Into::into),
            level: m.level.map(Into::into),
        }
    }
}

impl From<sea_orm_active_enums::NotificationLevel> for NotificationLevel {
    fn from(value: sea_orm_active_enums::NotificationLevel) -> Self {
        match value {
            sea_orm_active_enums::NotificationLevel::All => NotificationLevel::All,
            sea_orm_active_enums::NotificationLevel::Mentions => NotificationLevel::Mentions,
            sea_orm_active_enums::NotificationLevel::None => NotificationLevel::None,
        }
    }
}

impl From<NotificationLevel> for sea_orm_active_enums::NotificationLevel {
    fn from(value: NotificationLevel) -> Self {
        match value {
            NotificationLevel::All => sea_orm_active_enums::NotificationLevel::All,
            NotificationLevel::Mentions => sea_orm_active_enums::NotificationLevel::Mentions,
            NotificationLevel::None => sea_orm_active_enums::NotificationLevel::None,
        }
    }
}

impl From<sea_orm_active_enums::NotificationReason> for NotificationReason {
    fn from(value: sea_orm_active_enums::NotificationReason) -> Self {
        match value {
            sea_orm_active_enums::NotificationReason::Direct => NotificationReason::Direct,
            sea_orm_active_enums::NotificationReason::Mention => NotificationReason::Mention,
            sea_orm_active_enums::NotificationReason::Keyword => NotificationReason::Keyword,
            sea_orm_active_enums::NotificationReason::All => NotificationReason::All,
            sea_orm_active_enums::NotificationReason::ThreadReply => {
                NotificationReason::ThreadReply
            }
            sea_orm_active_enums::NotificationReason::Reaction => NotificationReason::Reaction,
        }
    }
}

impl From<NotificationReason> for sea_orm_active_enums::NotificationReason {
    fn from(value: NotificationReason) -> Self {
        match value {
            NotificationReason::Direct => sea_orm_active_enums::NotificationReason::Direct,
            NotificationReason::Mention => sea_orm_active_enums::NotificationReason::Mention,
            NotificationReason::Keyword => sea_orm_active_enums::NotificationReason::Keyword,
            NotificationReason::All => sea_orm_active_enums::NotificationReason::All,
            NotificationReason::ThreadReply => {
                sea_orm_active_enums::NotificationReason::ThreadReply
            }
            NotificationReason::Reaction => sea_orm_active_enums::NotificationReason::Reaction,
        }
    }
}

impl From<sea_orm_active_enums::DigestFrequency> for DigestFrequency {
    fn from(value: sea_orm_active_enums::DigestFrequency) -> Self {
        match value {
            sea_orm_active_enums::DigestFrequency::Off => DigestFrequency::Off,
            sea_orm_active_enums::DigestFrequency::Hourly => DigestFrequency::Hourly,
            sea_orm_active_enums::DigestFrequency::Daily => DigestFrequency::Daily,
        }
    }
}

impl From<DigestFrequency> for sea_orm_active_enums::DigestFrequency {
    fn from(value: DigestFrequency) -> Self {
        match value {
            DigestFrequency::Off => sea_orm_active_enums::DigestFrequency::Off,
            DigestFrequency::Hourly => sea_orm_active_enums::DigestFrequency::Hourly,
            DigestFrequency::Daily => sea_orm_active_enums::DigestFrequency::Daily,
        }
    }
}
