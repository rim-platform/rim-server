//! Posting into a conversation whose message request its recipient has not decided.

use std::sync::Arc;

use rim_core::conversation::model::ConversationId;
use rim_core::relay::repository::RelayRepository;
use rim_core::user::model::UserId;
use rim_services::{ServiceError, ServiceResult};

/// What a recipient who posts into a conversation they have not accepted is told.
pub(crate) const REQUEST_UNDECIDED: &str = "accept the message request before posting here";

/// `Ok` unless `conversation_id` stays out of `user_id`'s list
/// ([`RelayRepository::conversation_hidden`]): their message request for it is pending or
/// ignored, or they blocked the person who started it. They may read such a conversation first,
/// and post nothing into it until they accept. Always `Ok` without a relay repository.
///
/// # Errors
/// `Forbidden` with [`REQUEST_UNDECIDED`] for such a conversation; storage errors.
pub(crate) async fn ensure_request_decided(
    relay_repo: Option<&Arc<dyn RelayRepository>>,
    user_id: UserId,
    conversation_id: ConversationId,
) -> ServiceResult<()> {
    let Some(relay_repo) = relay_repo else {
        return Ok(());
    };
    if relay_repo
        .conversation_hidden(user_id, conversation_id.into())
        .await?
    {
        return Err(ServiceError::Forbidden(REQUEST_UNDECIDED.to_string()));
    }
    Ok(())
}
