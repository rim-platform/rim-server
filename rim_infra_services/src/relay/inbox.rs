//! How a relay files a delivered item for one recipient: into the queue, held by a DM request,
//! dropped, or refused.

use rim_core::relay::model::{DmRequestId, DmRequestStatus};

use crate::relay::policy::DmPolicy;

/// Most pending requests per recipient.
pub(crate) const MAX_PENDING_PER_USER: i64 = 100;
/// Most items a pending request holds; later ones are dropped until it is accepted.
pub(crate) const MAX_HELD_ITEMS: i64 = 3;

/// What the relay knows about an item and its recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InboxFacts {
    /// The item is a `DmInvite`.
    pub is_invite: bool,
    /// The item names a conversation (`conversation_id`).
    pub conversation_scoped: bool,
    /// The recipient blocked the invite's sender.
    pub blocked: bool,
    /// How the recipient's effective DM setting takes the invite's sender.
    pub policy: DmPolicy,
    /// The recipient's request for the item's conversation, and how many items it holds.
    pub request: Option<(DmRequestId, DmRequestStatus, i64)>,
    pub pending_for_user: i64,
    pub filed_by_sender_today: i64,
    /// Most requests one sender principal files on this relay per day.
    pub sender_daily_limit: i64,
}

/// Where an item goes for one recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filing {
    Deliver,
    Hold(DmRequestId),
    Drop,
    /// Refused with a reason the host sees.
    Refuse(&'static str),
    /// Record the conversation as accepted (an invite the policy delivers) and deliver.
    FileAccepted,
    /// File a pending request and hold the invite.
    FileRequest,
}

pub(crate) fn file(facts: &InboxFacts) -> Filing {
    if facts.is_invite && facts.blocked {
        return Filing::Refuse("blocked");
    }
    match facts.request {
        Some((_, DmRequestStatus::Accepted, _)) => Filing::Deliver,
        Some((_, DmRequestStatus::Ignored, _)) => Filing::Drop,
        Some((id, DmRequestStatus::Pending, held)) => {
            if held < MAX_HELD_ITEMS {
                Filing::Hold(id)
            } else {
                Filing::Drop
            }
        }
        None if !facts.is_invite => {
            if facts.conversation_scoped {
                Filing::Refuse("not_subscribed")
            } else {
                Filing::Deliver
            }
        }
        None if facts.policy == DmPolicy::Deliver => Filing::FileAccepted,
        None if facts.policy == DmPolicy::Refuse => Filing::Refuse("blocked"),
        None if facts.pending_for_user >= MAX_PENDING_PER_USER
            || facts.filed_by_sender_today >= facts.sender_daily_limit =>
        {
            Filing::Refuse("limited")
        }
        None => Filing::FileRequest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENDER_LIMIT: i64 = 20;

    fn invite() -> InboxFacts {
        InboxFacts {
            is_invite: true,
            conversation_scoped: true,
            blocked: false,
            policy: DmPolicy::Request,
            request: None,
            pending_for_user: 0,
            filed_by_sender_today: 0,
            sender_daily_limit: SENDER_LIMIT,
        }
    }

    fn message(request: Option<(DmRequestId, DmRequestStatus, i64)>) -> InboxFacts {
        InboxFacts {
            is_invite: false,
            request,
            ..invite()
        }
    }

    #[test]
    fn an_invite_files_as_the_policy_says() {
        assert_eq!(file(&invite()), Filing::FileRequest);
        assert_eq!(
            file(&InboxFacts {
                policy: DmPolicy::Deliver,
                ..invite()
            }),
            Filing::FileAccepted
        );
        assert_eq!(
            file(&InboxFacts {
                policy: DmPolicy::Refuse,
                ..invite()
            }),
            Filing::Refuse("blocked")
        );
    }

    #[test]
    fn a_blocked_sender_is_refused_even_when_the_policy_delivers() {
        assert_eq!(
            file(&InboxFacts {
                blocked: true,
                policy: DmPolicy::Deliver,
                ..invite()
            }),
            Filing::Refuse("blocked")
        );
    }

    #[test]
    fn a_decided_conversation_ignores_the_policy() {
        let id = DmRequestId::new();
        assert_eq!(
            file(&InboxFacts {
                policy: DmPolicy::Refuse,
                request: Some((id, DmRequestStatus::Accepted, 0)),
                ..invite()
            }),
            Filing::Deliver
        );
    }

    #[test]
    fn limits_refuse_new_requests() {
        assert_eq!(
            file(&InboxFacts {
                pending_for_user: MAX_PENDING_PER_USER,
                ..invite()
            }),
            Filing::Refuse("limited")
        );
        assert_eq!(
            file(&InboxFacts {
                filed_by_sender_today: SENDER_LIMIT,
                ..invite()
            }),
            Filing::Refuse("limited")
        );
        assert_eq!(
            file(&InboxFacts {
                filed_by_sender_today: SENDER_LIMIT - 1,
                ..invite()
            }),
            Filing::FileRequest
        );
    }

    #[test]
    fn a_pending_request_holds_three_items_then_drops() {
        let id = DmRequestId::new();
        for held in 0..MAX_HELD_ITEMS {
            assert_eq!(
                file(&message(Some((id, DmRequestStatus::Pending, held)))),
                Filing::Hold(id)
            );
        }
        assert_eq!(
            file(&message(Some((
                id,
                DmRequestStatus::Pending,
                MAX_HELD_ITEMS
            )))),
            Filing::Drop
        );
    }

    #[test]
    fn accepted_flows_ignored_drops() {
        let id = DmRequestId::new();
        assert_eq!(
            file(&message(Some((id, DmRequestStatus::Accepted, 0)))),
            Filing::Deliver
        );
        assert_eq!(
            file(&message(Some((id, DmRequestStatus::Ignored, 0)))),
            Filing::Drop
        );
    }

    #[test]
    fn conversation_items_without_an_invite_are_refused() {
        assert_eq!(file(&message(None)), Filing::Refuse("not_subscribed"));
        assert_eq!(
            file(&InboxFacts {
                conversation_scoped: false,
                ..message(None)
            }),
            Filing::Deliver
        );
    }
}
