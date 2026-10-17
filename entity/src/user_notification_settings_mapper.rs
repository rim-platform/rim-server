use crate::user_notification_settings;
use rim_core::notification::model::{DndPause, DndWindow, UserNotificationSettings};

impl From<user_notification_settings::Model> for UserNotificationSettings {
    fn from(m: user_notification_settings::Model) -> Self {
        // The CHECK constraint keeps the three window columns all set or all null.
        let dnd = match (m.dnd_start, m.dnd_end, m.dnd_timezone) {
            (Some(start), Some(end), Some(timezone)) => Some(DndWindow {
                start,
                end,
                timezone,
            }),
            _ => None,
        };
        let dnd_pause = match (m.dnd_paused, m.dnd_paused_until) {
            (false, _) => DndPause::Off,
            (true, None) => DndPause::Indefinite,
            (true, Some(until)) => DndPause::Until(until.into()),
        };
        Self {
            user_id: m.user_id.into(),
            server_id: m.server_id.into(),
            default_level: m.default_level.into(),
            keywords: m.keywords,
            dnd,
            dnd_pause,
            digest: m.digest.into(),
            updated_at: m.updated_at.into(),
            reactions: m.reactions,
        }
    }
}
