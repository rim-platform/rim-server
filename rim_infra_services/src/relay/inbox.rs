//! How a relay files a delivered item for one recipient who holds no subscription to its
//! workspace: into the queue, held by a DM request, dropped, or refused.

use chrono::Duration;
use rim_core::relay::model::{DmRequestId, DmRequestStatus};

use crate::relay::policy::DmPolicy;

/// Most pending requests per recipient.
pub(crate) const MAX_PENDING_PER_USER: i64 = 100;
/// Most items a pending request holds; later ones are dropped until it is accepted.
pub(crate) const MAX_HELD_ITEMS: i64 = 3;
/// Most bytes the items of one pending request take; a later item past it is dropped until the
/// request is accepted. Larger than any one envelope, so an invite always fits.
pub(crate) const MAX_HELD_BYTES_PER_REQUEST: i64 = 1024 * 1024;
/// Most bytes all items held for a recipient take: those of pending requests and the invites
/// ignored requests keep. Past it a new invite is refused (`limited`) and a later item is
/// dropped. Held items are a pool apart from the queue's byte cap: they are not counted toward
/// it and never dropped to make room in it, so this cap does not follow the configured queue
/// cap; it is a quarter of the public relay's (64 MiB).
pub(crate) const MAX_HELD_BYTES_PER_USER: i64 = 16 * 1024 * 1024;
/// Most accepted conversations a recipient holds from one other host. Past it an invite the
/// recipient's settings would deliver is refused (`limited`); accepting a request is not
/// limited, and no row is evicted to make room. Far more conversations than a person keeps with
/// the people of one server, while the rows stay a few MiB per recipient and host. This
/// server's own invites are not counted.
pub(crate) const MAX_ACCEPTED_PER_HOST: i64 = 5000;
/// The window a sender's daily request limit counts over.
pub(crate) const SENDER_WINDOW: Duration = Duration::days(1);

/// A limit that keeps a new DM request from being filed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestLimit {
    /// The recipient holds `MAX_PENDING_PER_USER` pending requests.
    RecipientFull,
    /// The sender filed their daily limit of requests in the last `SENDER_WINDOW`.
    SenderDaily,
}

/// The limit a new request is over, if any, given the recipient's pending requests and those
/// the sender filed in the last `SENDER_WINDOW`.
pub(crate) fn request_limit(
    pending_for_user: i64,
    filed_by_sender_today: i64,
    sender_daily_limit: i64,
) -> Option<RequestLimit> {
    if pending_for_user >= MAX_PENDING_PER_USER {
        Some(RequestLimit::RecipientFull)
    } else if filed_by_sender_today >= sender_daily_limit {
        Some(RequestLimit::SenderDaily)
    } else {
        None
    }
}
/// Most requests one delivering host files on this relay per day, whichever principals they
/// come from.
pub(crate) const MAX_REQUESTS_PER_HOST_PER_DAY: i64 = 1000;

/// What the relay knows about an item and its recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InboxFacts {
    /// The item is a `DmInvite`.
    pub is_invite: bool,
    /// The item names a conversation (`conversation_id`).
    pub conversation_scoped: bool,
    /// The item names no conversation and is on a stream a direct space sends such items to its
    /// members on: `e`, or the recipient's own `u:`.
    pub direct_stream: bool,
    /// The recipient holds an accepted request for a conversation the delivering host homes,
    /// so takes part in its direct space.
    pub accepted_at_host: bool,
    /// The item decides a join request the recipient filed at the item's workspace on the
    /// delivering host, on their own `u:` stream, and the request is recorded here.
    pub join_decided: bool,
    /// Bytes the delivering host may still have queued here today.
    pub peer_bytes_left: i64,
    /// The recipient blocked the invite's sender.
    pub blocked: bool,
    /// The item's conversation, hosted elsewhere, was accepted by the recipient from someone they
    /// blocked since: whoever started it or brought them into it.
    pub silenced: bool,
    /// How the recipient's effective DM setting takes the invite's sender.
    pub policy: DmPolicy,
    /// The recipient's request for the item's conversation, and how many items it holds.
    pub request: Option<(DmRequestId, DmRequestStatus, i64)>,
    /// Bytes the item takes.
    pub item_bytes: i64,
    /// Bytes the items held by the recipient's request for the conversation take.
    pub request_held_bytes: i64,
    /// Bytes the items held for the recipient take, kept invites of ignored requests included.
    pub user_held_bytes: i64,
    pub pending_for_user: i64,
    pub filed_by_sender_today: i64,
    /// Most requests one sender principal files on this relay per day.
    pub sender_daily_limit: i64,
    /// Requests the delivering host filed on this relay in the last day; 0 for this server's
    /// own events.
    pub filed_by_host_today: i64,
    /// The recipient's accepted conversations from the item's host, counted for an invite the
    /// policy delivers; 0 for this server's own events.
    pub accepted_from_host: i64,
}

impl InboxFacts {
    /// Whether holding the item keeps its request and its recipient within the held byte caps.
    fn holding_fits(&self) -> bool {
        self.request_held_bytes + self.item_bytes <= MAX_HELD_BYTES_PER_REQUEST
            && self.user_held_bytes + self.item_bytes <= MAX_HELD_BYTES_PER_USER
    }
}

/// Where an item goes for one recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filing {
    Deliver,
    Hold(DmRequestId),
    Drop,
    /// Refused with a reason the host sees.
    Refuse(&'static str),
    /// Refused (`limited`): queueing it takes the delivering host past its daily bytes.
    PeerLimited,
    /// Record the conversation as accepted (an invite the policy delivers) and deliver.
    FileAccepted,
    /// File a pending request and hold the invite.
    FileRequest,
}

/// Where an item goes for one recipient: as the request inbox files it, unless queueing it
/// takes the delivering host past its daily bytes.
pub(crate) fn file(facts: &InboxFacts) -> Filing {
    let filing = place(facts);
    let queues = matches!(
        filing,
        Filing::Deliver | Filing::Hold(_) | Filing::FileAccepted | Filing::FileRequest
    );
    if queues && facts.item_bytes > facts.peer_bytes_left {
        Filing::PeerLimited
    } else {
        filing
    }
}

/// Where the request inbox files an item for one recipient, whatever the host's daily bytes.
fn place(facts: &InboxFacts) -> Filing {
    if facts.is_invite && facts.blocked {
        return Filing::Refuse("blocked");
    }
    if facts.silenced {
        return Filing::Drop;
    }
    match facts.request {
        Some((_, DmRequestStatus::Accepted, _)) => Filing::Deliver,
        Some((_, DmRequestStatus::Ignored, _)) => Filing::Drop,
        Some((id, DmRequestStatus::Pending, held)) => {
            if held < MAX_HELD_ITEMS && facts.holding_fits() {
                Filing::Hold(id)
            } else {
                Filing::Drop
            }
        }
        None if !facts.is_invite => {
            if !facts.conversation_scoped
                && (facts.join_decided || facts.direct_stream && facts.accepted_at_host)
            {
                Filing::Deliver
            } else {
                Filing::Refuse("not_subscribed")
            }
        }
        None if facts.policy == DmPolicy::Deliver => {
            if facts.accepted_from_host >= MAX_ACCEPTED_PER_HOST {
                Filing::Refuse("limited")
            } else {
                Filing::FileAccepted
            }
        }
        None if facts.policy == DmPolicy::Refuse => Filing::Refuse("blocked"),
        None if request_limit(
            facts.pending_for_user,
            facts.filed_by_sender_today,
            facts.sender_daily_limit,
        )
        .is_some()
            || facts.filed_by_host_today >= MAX_REQUESTS_PER_HOST_PER_DAY
            || !facts.holding_fits() =>
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
            direct_stream: false,
            accepted_at_host: false,
            join_decided: false,
            peer_bytes_left: i64::MAX,
            blocked: false,
            silenced: false,
            policy: DmPolicy::Request,
            request: None,
            pending_for_user: 0,
            filed_by_sender_today: 0,
            sender_daily_limit: SENDER_LIMIT,
            item_bytes: 1000,
            request_held_bytes: 0,
            user_held_bytes: 0,
            filed_by_host_today: 0,
            accepted_from_host: 0,
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
    fn what_a_blocked_sender_started_is_dropped_whatever_was_decided() {
        let id = DmRequestId::new();
        for request in [
            Some((id, DmRequestStatus::Accepted, 0)),
            Some((id, DmRequestStatus::Pending, 0)),
            None,
        ] {
            assert_eq!(
                file(&InboxFacts {
                    silenced: true,
                    ..message(request)
                }),
                Filing::Drop,
                "{request:?}"
            );
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
    fn a_full_inbox_is_named_before_the_senders_limit() {
        assert_eq!(request_limit(0, 0, SENDER_LIMIT), None);
        assert_eq!(
            request_limit(MAX_PENDING_PER_USER - 1, SENDER_LIMIT - 1, SENDER_LIMIT),
            None
        );
        assert_eq!(
            request_limit(MAX_PENDING_PER_USER, SENDER_LIMIT, SENDER_LIMIT),
            Some(RequestLimit::RecipientFull)
        );
        assert_eq!(
            request_limit(0, SENDER_LIMIT, SENDER_LIMIT),
            Some(RequestLimit::SenderDaily)
        );
    }

    #[test]
    fn a_host_files_a_limited_number_of_requests_a_day_from_any_senders() {
        assert_eq!(
            file(&InboxFacts {
                filed_by_host_today: MAX_REQUESTS_PER_HOST_PER_DAY,
                ..invite()
            }),
            Filing::Refuse("limited")
        );
        assert_eq!(
            file(&InboxFacts {
                filed_by_host_today: MAX_REQUESTS_PER_HOST_PER_DAY - 1,
                ..invite()
            }),
            Filing::FileRequest
        );
        assert_eq!(
            file(&InboxFacts {
                filed_by_host_today: MAX_REQUESTS_PER_HOST_PER_DAY,
                policy: DmPolicy::Deliver,
                ..invite()
            }),
            Filing::FileAccepted,
            "an invite the policy delivers is no request"
        );
    }

    #[test]
    fn a_delivered_invite_past_the_accepted_cap_is_refused() {
        let delivered = |accepted_from_host| InboxFacts {
            policy: DmPolicy::Deliver,
            accepted_from_host,
            ..invite()
        };
        assert_eq!(
            file(&delivered(MAX_ACCEPTED_PER_HOST - 1)),
            Filing::FileAccepted
        );
        assert_eq!(
            file(&delivered(MAX_ACCEPTED_PER_HOST)),
            Filing::Refuse("limited")
        );
        assert_eq!(
            file(&InboxFacts {
                accepted_from_host: MAX_ACCEPTED_PER_HOST,
                ..invite()
            }),
            Filing::FileRequest,
            "a request is counted by the request limits"
        );
        let id = DmRequestId::new();
        assert_eq!(
            file(&InboxFacts {
                accepted_from_host: MAX_ACCEPTED_PER_HOST,
                ..message(Some((id, DmRequestStatus::Accepted, 0)))
            }),
            Filing::Deliver,
            "an accepted conversation keeps flowing"
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
    fn a_pending_request_holds_items_within_its_byte_cap_then_drops() {
        let id = DmRequestId::new();
        let pending = |request_held_bytes, user_held_bytes| InboxFacts {
            request_held_bytes,
            user_held_bytes,
            ..message(Some((id, DmRequestStatus::Pending, 1)))
        };
        let room = MAX_HELD_BYTES_PER_REQUEST - 1000;
        assert_eq!(file(&pending(room, room)), Filing::Hold(id));
        assert_eq!(file(&pending(room + 1, room + 1)), Filing::Drop);
        assert_eq!(
            file(&pending(0, MAX_HELD_BYTES_PER_USER - 999)),
            Filing::Drop,
            "the recipient's held bytes are full"
        );
    }

    #[test]
    fn an_invite_past_the_recipients_held_bytes_is_refused() {
        let full = |user_held_bytes| InboxFacts {
            user_held_bytes,
            ..invite()
        };
        assert_eq!(
            file(&full(MAX_HELD_BYTES_PER_USER - 1000)),
            Filing::FileRequest
        );
        assert_eq!(
            file(&full(MAX_HELD_BYTES_PER_USER - 999)),
            Filing::Refuse("limited")
        );
        assert_eq!(
            file(&InboxFacts {
                policy: DmPolicy::Deliver,
                ..full(MAX_HELD_BYTES_PER_USER)
            }),
            Filing::FileAccepted,
            "a delivered invite is not held"
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
                accepted_at_host: true,
                ..message(None)
            }),
            Filing::Refuse("not_subscribed"),
            "another conversation of the host's direct space"
        );
    }

    /// An item without a conversation: on `w`, or on `e` or the recipient's own `u:` stream.
    fn unscoped(direct_stream: bool, accepted_at_host: bool) -> InboxFacts {
        InboxFacts {
            conversation_scoped: false,
            direct_stream,
            accepted_at_host,
            ..message(None)
        }
    }

    #[test]
    fn workspace_items_are_refused_without_a_subscription() {
        assert_eq!(
            file(&unscoped(false, false)),
            Filing::Refuse("not_subscribed")
        );
        assert_eq!(
            file(&unscoped(false, true)),
            Filing::Refuse("not_subscribed"),
            "a direct space sends nothing on `w` or another person's stream"
        );
    }

    #[test]
    fn direct_space_items_flow_once_the_recipient_accepted_a_conversation_there() {
        assert_eq!(file(&unscoped(true, true)), Filing::Deliver);
        assert_eq!(
            file(&unscoped(true, false)),
            Filing::Refuse("not_subscribed"),
            "no accepted conversation at the host"
        );
    }

    #[test]
    fn a_join_decision_flows_while_its_request_is_recorded() {
        let decided = InboxFacts {
            join_decided: true,
            ..unscoped(false, false)
        };
        assert_eq!(file(&decided), Filing::Deliver);
        assert_eq!(
            file(&InboxFacts {
                peer_bytes_left: 0,
                ..decided.clone()
            }),
            Filing::PeerLimited
        );
        assert_eq!(
            file(&InboxFacts {
                conversation_scoped: true,
                ..decided
            }),
            Filing::Refuse("not_subscribed"),
            "a join decision names no conversation"
        );
    }

    #[test]
    fn an_item_past_the_hosts_daily_bytes_is_refused_and_a_refusal_stays_one() {
        let id = DmRequestId::new();
        let left = |facts: InboxFacts, peer_bytes_left| InboxFacts {
            peer_bytes_left,
            ..facts
        };
        for (what, facts) in [
            (
                "an accepted conversation's item",
                message(Some((id, DmRequestStatus::Accepted, 0))),
            ),
            (
                "a held item",
                message(Some((id, DmRequestStatus::Pending, 0))),
            ),
            ("an invite filed as a request", invite()),
            (
                "an invite the policy delivers",
                InboxFacts {
                    policy: DmPolicy::Deliver,
                    ..invite()
                },
            ),
            ("a direct space item", unscoped(true, true)),
        ] {
            let placed = file(&facts);
            assert_eq!(file(&left(facts.clone(), 1000)), placed, "{what} fits");
            assert_eq!(
                file(&left(facts, 999)),
                Filing::PeerLimited,
                "{what} past the bytes"
            );
        }
        assert_eq!(
            file(&left(message(Some((id, DmRequestStatus::Ignored, 0))), 0)),
            Filing::Drop
        );
        assert_eq!(
            file(&left(
                InboxFacts {
                    silenced: true,
                    ..message(Some((id, DmRequestStatus::Accepted, 0)))
                },
                0
            )),
            Filing::Drop
        );
        assert_eq!(
            file(&left(unscoped(false, false), 0)),
            Filing::Refuse("not_subscribed")
        );
    }
}
