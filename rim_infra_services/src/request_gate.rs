//! Posting into a conversation whose message request its recipient has not decided, or into a
//! DM whose other member blocked the poster.

use std::sync::Arc;

use chrono::Utc;
use rim_core::conversation::model::ConversationId;
use rim_core::relay::repository::RelayRepository;
use rim_core::user::model::UserId;
use rim_services::{ServiceError, ServiceResult};

use crate::relay::accepted_expiry;

/// What a recipient who posts into a conversation they have not accepted is told.
pub(crate) const REQUEST_UNDECIDED: &str = "accept the message request before posting here";

/// What someone blocked by the other member of a DM is told when they start it or post into it.
pub(crate) const DM_NOT_TAKEN: &str = "they do not take direct messages from you";

/// `Ok` unless `conversation_id` stays out of `user_id`'s list
/// ([`RelayRepository::conversation_hidden`]): their message request for it is pending or
/// ignored, or they blocked the person who started the DM or group. They may read such a
/// conversation first, and post nothing into it until they accept. A cleared request (listed
/// nowhere) is accepted by posting, and the post goes through unless the conversation stays
/// hidden for another reason. Also not `Ok` when it is a direct conversation whose other member
/// blocked `user_id` ([`RelayRepository::blocked_by_dm_partner`]), so nothing they write while
/// blocked is stored. Always `Ok` without a relay repository.
///
/// # Errors
/// `Forbidden` with [`REQUEST_UNDECIDED`] for a conversation kept out of the list, and with
/// [`DM_NOT_TAKEN`] for such a DM; storage errors.
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
        // A cleared request is listed nowhere: its user takes the conversation back by posting.
        let taken = relay_repo
            .accept_cleared(user_id, conversation_id.into(), accepted_expiry(Utc::now()))
            .await?;
        if !taken
            || relay_repo
                .conversation_hidden(user_id, conversation_id.into())
                .await?
        {
            return Err(ServiceError::Forbidden(REQUEST_UNDECIDED.to_string()));
        }
    }
    if relay_repo
        .blocked_by_dm_partner(user_id, conversation_id.into())
        .await?
    {
        return Err(ServiceError::Forbidden(DM_NOT_TAKEN.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use rim_core::relay::repository::MockRelayRepository;

    use super::*;

    /// A relay repository answering `conversation_hidden` with `hidden`, one answer per call
    /// and called exactly that often, `accept_cleared` with `taken` and `blocked_by_dm_partner`
    /// with `blocked`; `None` fails the test when asked.
    fn gate(
        hidden: &[bool],
        taken: Option<bool>,
        blocked: Option<bool>,
    ) -> Arc<dyn RelayRepository> {
        let mut relay = MockRelayRepository::new();
        let answers = Mutex::new(VecDeque::from(hidden.to_vec()));
        relay
            .expect_conversation_hidden()
            .times(hidden.len())
            .returning(move |_, _| {
                let hidden = answers
                    .lock()
                    .expect("lock")
                    .pop_front()
                    .expect("an answer");
                Box::pin(async move { Ok(hidden) })
            });
        match taken {
            Some(taken) => {
                relay
                    .expect_accept_cleared()
                    .returning(move |_, _, _| Box::pin(async move { Ok(taken) }));
            }
            None => {
                relay.expect_accept_cleared().never();
            }
        }
        match blocked {
            Some(blocked) => {
                relay
                    .expect_blocked_by_dm_partner()
                    .returning(move |_, _| Box::pin(async move { Ok(blocked) }));
            }
            None => {
                relay.expect_blocked_by_dm_partner().never();
            }
        }
        Arc::new(relay)
    }

    async fn post(relay: Arc<dyn RelayRepository>) -> ServiceResult<()> {
        ensure_request_decided(Some(&relay), UserId::new(), ConversationId::new()).await
    }

    async fn refusal(relay: Arc<dyn RelayRepository>) -> String {
        match post(relay).await {
            Err(ServiceError::Forbidden(message)) => message,
            other => panic!("refused, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn someone_blocked_by_the_other_member_of_a_dm_posts_nothing_into_it() {
        assert_eq!(
            refusal(gate(&[false], None, Some(true))).await,
            DM_NOT_TAKEN
        );
    }

    #[tokio::test]
    async fn an_undecided_request_is_refused_before_the_block_is_asked() {
        assert_eq!(
            refusal(gate(&[true], Some(false), None)).await,
            REQUEST_UNDECIDED
        );
    }

    #[tokio::test]
    async fn a_conversation_neither_hidden_nor_blocked_takes_posts() {
        assert!(post(gate(&[false], None, Some(false))).await.is_ok());
    }

    #[tokio::test]
    async fn posting_takes_back_a_cleared_request() {
        assert!(
            post(gate(&[true, false], Some(true), Some(false)))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_taken_back_conversation_that_stays_hidden_is_refused() {
        assert_eq!(
            refusal(gate(&[true, true], Some(true), None)).await,
            REQUEST_UNDECIDED
        );
    }

    #[tokio::test]
    async fn an_undecided_listed_request_is_not_taken_back() {
        assert_eq!(
            refusal(gate(&[true], Some(false), None)).await,
            REQUEST_UNDECIDED
        );
    }

    #[tokio::test]
    async fn without_a_relay_everything_posts() {
        assert!(
            ensure_request_decided(None, UserId::new(), ConversationId::new())
                .await
                .is_ok()
        );
    }
}
