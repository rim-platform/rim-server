//! Bot accounts of a workspace and their tokens. A bot is a `user` row of the kind `bot`, so it
//! is keyed by its [`UserId`].

use chrono::{DateTime, Utc};

use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(BotTokenId);

/// A bot as its workspace sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bot {
    pub user_id: UserId,
    pub server_id: ServerId,
    /// The bot's display name, from its `user` row.
    pub name: String,
    /// Who runs it, as the person who added it says.
    pub run_by: String,
    /// Where what it reads goes, as the person who added it says.
    pub reads_go_to: String,
    pub created_by: UserId,
    /// The name of the person who added it; `None` when that account has no name left to show.
    pub added_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub deactivated_at: Option<DateTime<Utc>>,
}

/// A new bot, with what its `user` row is created with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBot {
    pub server_id: ServerId,
    pub username: String,
    pub name: String,
    pub run_by: String,
    pub reads_go_to: String,
    pub created_by: UserId,
}

/// The fields of a bot to change; `None` leaves a field as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BotPatch {
    pub name: Option<String>,
    pub run_by: Option<String>,
    pub reads_go_to: Option<String>,
}

/// A bot's token, never with its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotToken {
    pub id: BotTokenId,
    pub bot_user_id: UserId,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A new token: the SHA-256 of the string shown once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBotToken {
    pub bot_user_id: UserId,
    pub label: String,
    pub token_hash: String,
    pub created_by: UserId,
}

/// A token found by its hash, with the bot's standing as far as the bot tables know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotTokenHolder {
    pub token: BotToken,
    pub server_id: ServerId,
    pub bot_deactivated_at: Option<DateTime<Utc>>,
}
