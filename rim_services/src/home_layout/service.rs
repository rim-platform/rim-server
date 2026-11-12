use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::home_layout::dto::{HomeLayoutDto, HomeLayoutSavedDto};
use crate::home_layout::request::SaveHomeLayoutCmd;
use crate::{Actor, ServiceResult};

/// The signed-in person's Home layout, kept on their home server, one per account.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait HomeLayoutService: Send + Sync {
    /// The caller's stored layout, or `None` when none is stored.
    ///
    /// # Errors
    ///
    /// `Forbidden` for a person from another server or a bot.
    async fn home_layout(&self, actor: Actor) -> ServiceResult<Option<HomeLayoutDto>>;

    /// Store the caller's layout unless the stored one has a later `edited_at`, an `edited_at`
    /// later than the server's clock counting as the clock.
    ///
    /// # Errors
    ///
    /// `Validation` naming the check a layout fails; `Forbidden` for a person from another
    /// server or a bot.
    async fn save_home_layout(&self, cmd: SaveHomeLayoutCmd) -> ServiceResult<HomeLayoutSavedDto>;

    /// Delete the caller's layout. Returns whether one was stored.
    ///
    /// # Errors
    ///
    /// `Forbidden` for a person from another server or a bot.
    async fn reset_home_layout(&self, actor: Actor) -> ServiceResult<bool>;
}
