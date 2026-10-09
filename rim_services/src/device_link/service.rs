use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::device_link::command::{
    CancelDeviceLinkCmd, CompleteDeviceLinkCmd, GetDeviceLinkAnswerQuery, GetDeviceLinkQuery,
    SealedPayload, SendDeviceLinkAnswerCmd, SendDeviceLinkOfferCmd, StartDeviceLinkCmd,
};
use crate::device_link::dto::{DeviceLinkDto, StartedDeviceLinkDto};

/// Device linking: a courier for the sealed offer and answer through which a signed-in device
/// certifies a new device's key. It never reads the sealed bytes, and depends on nothing of key
/// login but the account's principal id.
///
/// Anonymous calls answer `Unauthenticated` alike for every link they cannot act on; calls
/// with a session answer `NotFound` for a malformed or unknown link id and for another
/// user's link.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait DeviceLinkService: Send + Sync {
    /// Start a link for the signed-in account, cancelling its previous live link.
    ///
    /// # Errors
    /// - `Validation` when the account has no key identity.
    /// - `RateLimited` when the account starts too many links.
    async fn start(&self, cmd: StartDeviceLinkCmd) -> ServiceResult<StartedDeviceLinkDto>;

    /// Store the new device's sealed offer on an `open` link. Sending the same bytes again
    /// while `offered` succeeds.
    ///
    /// # Errors
    /// - `Validation` for a malformed or oversized sealed message, before the link is looked
    ///   up.
    /// - `Unauthenticated`, with no detail, for any link it cannot act on.
    /// - `RateLimited` per IP.
    async fn send_offer(&self, cmd: SendDeviceLinkOfferCmd) -> ServiceResult<()>;

    /// The state, expiry and offer of one of the caller's links.
    ///
    /// # Errors
    /// `NotFound` for a malformed or unknown link id, or another user's link.
    async fn get(&self, query: GetDeviceLinkQuery) -> ServiceResult<DeviceLinkDto>;

    /// Store the sealed answer on an `offered` link; it then expires 10 minutes later. Sending
    /// the same bytes again while `answered` succeeds.
    ///
    /// # Errors
    /// - `Validation` for a malformed or oversized sealed message.
    /// - `NotFound` as for [`DeviceLinkService::get`].
    /// - `Conflict` for a wrong state, an expired link, or other bytes already stored.
    async fn send_answer(&self, cmd: SendDeviceLinkAnswerCmd) -> ServiceResult<DeviceLinkDto>;

    /// The sealed answer once the link is `answered`; `None` while `open` or `offered`.
    ///
    /// # Errors
    /// `Unauthenticated`, with no detail, for any other link; `RateLimited` per IP.
    async fn get_answer(
        &self,
        query: GetDeviceLinkAnswerQuery,
    ) -> ServiceResult<Option<SealedPayload>>;

    /// Mark an `answered` link `consumed`. Succeeds again while `consumed`.
    ///
    /// # Errors
    /// `NotFound` as for [`DeviceLinkService::get`]; `Conflict` for a wrong state or an
    /// expired link.
    async fn complete(&self, cmd: CompleteDeviceLinkCmd) -> ServiceResult<()>;

    /// Cancel a live link. The link's user may repeat it on a cancelled or expired link.
    ///
    /// # Errors
    /// - `Conflict` when the link's user cancels a `consumed` link.
    /// - `Unauthenticated`, with no detail, for any other caller and any link it cannot act
    ///   on; `RateLimited` per IP.
    async fn cancel(&self, cmd: CancelDeviceLinkCmd) -> ServiceResult<()>;
}
