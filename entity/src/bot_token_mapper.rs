use crate::bot_token;
use rim_core::bot::model::BotToken;

impl From<bot_token::Model> for BotToken {
    fn from(m: bot_token::Model) -> Self {
        Self {
            id: m.id.into(),
            bot_user_id: m.bot_user_id.into(),
            label: m.label,
            created_at: m.created_at.into(),
            last_used_at: m.last_used_at.map(Into::into),
            revoked_at: m.revoked_at.map(Into::into),
        }
    }
}
