//! Who may start a DM with a local user, and with whom on other servers a local user may start
//! one: the operator's settings, then the floors of the recipient's workspaces, then the
//! recipient's choice. Strictness only increases from one layer to the next.

use rim_core::dm_policy::model::{DmInbound, MemberDm};
use rim_core::dm_policy::model::{OutsideDm, OutsideFloor, SharedWorkspace, UserWorkspaceDm};
use rim_core::dm_policy::repository::DmPolicyRepository;
use rim_core::relay::repository::RelayRepository;
use rim_core::user::model::UserId;
use rim_services::dm_policy::dto::{DmFloorSourceDto, DmOperatorPolicy};
use rim_services::{ServiceError, ServiceResult};

/// How a DM's first message reaches its recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DmPolicy {
    /// Straight into the recipient's conversations.
    Deliver,
    /// As a request the recipient accepts or ignores.
    Request,
    /// Not at all; the sender's server is told as for a block.
    Refuse,
}

/// How strict a layer is about DMs from outside the recipient's workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Strictness {
    Open,
    Requests,
    Closed,
}

impl From<OutsideFloor> for Strictness {
    fn from(floor: OutsideFloor) -> Self {
        match floor {
            OutsideFloor::None => Self::Open,
            OutsideFloor::Requests => Self::Requests,
            OutsideFloor::Closed => Self::Closed,
        }
    }
}

impl From<OutsideDm> for Strictness {
    fn from(choice: OutsideDm) -> Self {
        match choice {
            OutsideDm::Requests => Self::Requests,
            OutsideDm::None => Self::Closed,
        }
    }
}

/// What this server knows about a local recipient and the sender.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalRecipient<'a> {
    /// The sender is the recipient's contact, or the recipient accepted them before.
    pub contact: bool,
    /// The workspaces here the recipient shares with the sender.
    pub shared: &'a [SharedWorkspace],
    /// The recipient's workspaces, whose floors apply to senders outside them.
    pub workspaces: &'a [UserWorkspaceDm],
    /// The recipient's choice for senders outside their workspaces.
    pub outside: OutsideDm,
}

/// Everything the policy weighs for one DM.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DmPolicyFacts<'a> {
    pub operator: &'a DmOperatorPolicy,
    pub own_server_id: &'a str,
    /// The server the DM comes from: the sender's relay, or the host of the conversation.
    pub sender_server_id: &'a str,
    /// The recipient's server.
    pub recipient_server_id: &'a str,
    /// The recipient, when they are a local user; `None` for one on another server, whose own
    /// server applies the rest.
    pub recipient: Option<LocalRecipient<'a>>,
}

/// The recipient's effective setting for this DM.
///
/// A DM from this server to another needs the operator's outbound setting to admit that server.
/// For a sender sharing a workspace with the recipient: an Owner or Moderator of one of them,
/// or a contact, is delivered; otherwise it is delivered when the recipient's choice (or the
/// workspace's default) for one of them is `direct`, else it is a request. For a sender outside
/// the recipient's workspaces: from another server, the operator's inbound setting must admit
/// that server (contacts included); then a contact is delivered; otherwise the strictest of the
/// operator's floor, the recipient's workspaces' floors and the recipient's choice decides
/// between a request and a refusal.
pub(crate) fn effective_dm_policy(facts: &DmPolicyFacts<'_>) -> DmPolicy {
    let from_here = facts.sender_server_id == facts.own_server_id;
    if from_here
        && facts.recipient_server_id != facts.own_server_id
        && !facts.operator.admits_outbound(facts.recipient_server_id)
    {
        return DmPolicy::Refuse;
    }
    let Some(recipient) = facts.recipient else {
        return DmPolicy::Deliver;
    };
    if !recipient.shared.is_empty() {
        let delivered = recipient.contact
            || recipient
                .shared
                .iter()
                .any(|s| s.sender_role.is_moderator() || s.workspace.members() == MemberDm::Direct);
        return if delivered {
            DmPolicy::Deliver
        } else {
            DmPolicy::Request
        };
    }
    if !from_here && !facts.operator.admits_inbound(facts.sender_server_id) {
        return DmPolicy::Refuse;
    }
    if recipient.contact {
        return DmPolicy::Deliver;
    }
    let operator = match facts.operator.inbound {
        DmInbound::Open => Strictness::Open,
        DmInbound::Requests | DmInbound::Partners => Strictness::Requests,
        DmInbound::None => Strictness::Closed,
    };
    let strictest = recipient
        .workspaces
        .iter()
        .map(|w| Strictness::from(w.outside_floor))
        .chain([operator, Strictness::from(recipient.outside)])
        .max()
        .unwrap_or(Strictness::Requests);
    match strictest {
        Strictness::Open => DmPolicy::Deliver,
        Strictness::Requests => DmPolicy::Request,
        Strictness::Closed => DmPolicy::Refuse,
    }
}

/// The refusal of a DM a local user starts with someone on a server the operator's outbound
/// setting does not admit.
pub(crate) fn outbound_refused() -> ServiceError {
    ServiceError::Forbidden("this server starts no direct messages with that server".to_string())
}

/// The floor that refuses DMs from outside a person's workspaces, whatever they choose: the
/// operator's inbound `none`, else the first of their workspaces whose floor is `closed`.
pub(crate) fn closing_floor(
    operator: &DmOperatorPolicy,
    workspaces: &[UserWorkspaceDm],
) -> Option<DmFloorSourceDto> {
    if operator.inbound == DmInbound::None {
        return Some(DmFloorSourceDto::Operator);
    }
    workspaces
        .iter()
        .find(|w| w.outside_floor == OutsideFloor::Closed)
        .map(|w| DmFloorSourceDto::Workspace {
            workspace_id: w.server_id,
            name: w.name.clone(),
        })
}

/// Where the policy reads a local recipient's facts.
#[derive(Clone, Copy)]
pub(crate) struct PolicySource<'a> {
    pub relay_repo: &'a dyn RelayRepository,
    pub dm_policy_repo: &'a dyn DmPolicyRepository,
    pub operator: &'a DmOperatorPolicy,
    pub own_server_id: &'a str,
}

impl PolicySource<'_> {
    /// The policy for a DM from `sender_principal_id`, coming from `sender_server_id`, to the
    /// local user `recipient`.
    pub(crate) async fn inbound(
        &self,
        recipient: UserId,
        sender_principal_id: &str,
        sender_server_id: &str,
    ) -> ServiceResult<DmPolicy> {
        let shared = self
            .dm_policy_repo
            .shared_workspaces(recipient, sender_principal_id)
            .await?;
        let contact = self
            .relay_repo
            .is_contact(recipient, sender_principal_id)
            .await?
            || self
                .relay_repo
                .accepted_from(recipient, sender_principal_id)
                .await?;
        self.judge(recipient, sender_server_id, contact, &shared)
            .await
    }

    /// The policy for a DM to the local user `recipient` from a sender this server could not
    /// verify, coming from `sender_server_id`: as from a stranger, whoever the sender claims
    /// to be.
    pub(crate) async fn stranger(
        &self,
        recipient: UserId,
        sender_server_id: &str,
    ) -> ServiceResult<DmPolicy> {
        self.judge(recipient, sender_server_id, false, &[]).await
    }

    /// The policy for a DM to `recipient` from `sender_server_id`, from a sender who is their
    /// contact or not and shares `shared` with them.
    async fn judge(
        &self,
        recipient: UserId,
        sender_server_id: &str,
        contact: bool,
        shared: &[SharedWorkspace],
    ) -> ServiceResult<DmPolicy> {
        let (workspaces, outside) = if shared.is_empty() && !contact {
            (
                self.dm_policy_repo.user_workspaces(recipient).await?,
                self.dm_policy_repo.outside(recipient).await?,
            )
        } else {
            (Vec::new(), OutsideDm::default())
        };
        Ok(effective_dm_policy(&DmPolicyFacts {
            operator: self.operator,
            own_server_id: self.own_server_id,
            sender_server_id,
            recipient_server_id: self.own_server_id,
            recipient: Some(LocalRecipient {
                contact,
                shared,
                workspaces: &workspaces,
                outside,
            }),
        }))
    }
}

/// The policy for a DM a local user starts with someone whose server is `recipient_server_id`.
pub(crate) fn outbound(
    operator: &DmOperatorPolicy,
    own_server_id: &str,
    recipient_server_id: &str,
) -> DmPolicy {
    effective_dm_policy(&DmPolicyFacts {
        operator,
        own_server_id,
        sender_server_id: own_server_id,
        recipient_server_id,
        recipient: None,
    })
}

#[cfg(test)]
mod tests {
    use rim_core::dm_policy::model::DmOutbound;
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::ServerMemberRole;

    use super::*;

    const HERE: &str = "rims1_here";
    const PARTNER: &str = "rims1_partner";
    const ELSEWHERE: &str = "rims1_elsewhere";

    fn operator(inbound: DmInbound, outbound: DmOutbound) -> DmOperatorPolicy {
        DmOperatorPolicy {
            inbound,
            outbound,
            partners: vec![PARTNER.to_string()],
        }
    }

    fn defaults() -> DmOperatorPolicy {
        DmOperatorPolicy::default()
    }

    fn workspace(
        floor: OutsideFloor,
        default: MemberDm,
        choice: Option<MemberDm>,
    ) -> UserWorkspaceDm {
        UserWorkspaceDm {
            server_id: ServerId::new(),
            name: "Acme".to_string(),
            outside_floor: floor,
            member_default: default,
            choice,
        }
    }

    fn shared(role: ServerMemberRole, workspace: UserWorkspaceDm) -> SharedWorkspace {
        SharedWorkspace {
            workspace,
            sender_role: role,
        }
    }

    /// A stranger: no contact, no shared workspace, the recipient in `workspaces` choosing
    /// `outside`.
    fn stranger<'a>(workspaces: &'a [UserWorkspaceDm], outside: OutsideDm) -> LocalRecipient<'a> {
        LocalRecipient {
            contact: false,
            shared: &[],
            workspaces,
            outside,
        }
    }

    fn inbound(operator: &DmOperatorPolicy, from: &str, recipient: LocalRecipient<'_>) -> DmPolicy {
        effective_dm_policy(&DmPolicyFacts {
            operator,
            own_server_id: HERE,
            sender_server_id: from,
            recipient_server_id: HERE,
            recipient: Some(recipient),
        })
    }

    #[test]
    fn a_stranger_with_default_settings_files_a_request_from_anywhere() {
        for from in [HERE, ELSEWHERE] {
            assert_eq!(
                inbound(&defaults(), from, stranger(&[], OutsideDm::Requests)),
                DmPolicy::Request
            );
        }
    }

    #[test]
    fn a_remote_member_of_a_direct_workspace_is_delivered_as_before() {
        let shared = [shared(
            ServerMemberRole::Member,
            workspace(OutsideFloor::None, MemberDm::Direct, None),
        )];
        let recipient = LocalRecipient {
            contact: false,
            shared: &shared,
            workspaces: &[],
            outside: OutsideDm::Requests,
        };
        assert_eq!(
            inbound(&defaults(), ELSEWHERE, recipient),
            DmPolicy::Deliver
        );
    }

    #[test]
    fn floors_only_tighten() {
        let open = operator(DmInbound::Open, DmOutbound::Allowed);
        // No layer opens past the person's `requests`.
        assert_eq!(
            inbound(&open, ELSEWHERE, stranger(&[], OutsideDm::Requests)),
            DmPolicy::Request
        );
        // A workspace floor of `requests` changes nothing; `closed` refuses whatever the person
        // chose.
        let requests = [workspace(OutsideFloor::Requests, MemberDm::Direct, None)];
        assert_eq!(
            inbound(&open, ELSEWHERE, stranger(&requests, OutsideDm::Requests)),
            DmPolicy::Request
        );
        let closed = [
            workspace(OutsideFloor::None, MemberDm::Direct, None),
            workspace(OutsideFloor::Closed, MemberDm::Direct, None),
        ];
        assert_eq!(
            inbound(&open, ELSEWHERE, stranger(&closed, OutsideDm::Requests)),
            DmPolicy::Refuse
        );
        // The person's `none` holds under looser floors.
        assert_eq!(
            inbound(&open, HERE, stranger(&requests, OutsideDm::None)),
            DmPolicy::Refuse
        );
        // The operator's `none` refuses local strangers too.
        let none = operator(DmInbound::None, DmOutbound::Allowed);
        assert_eq!(
            inbound(&none, HERE, stranger(&[], OutsideDm::Requests)),
            DmPolicy::Refuse
        );
        assert_eq!(
            inbound(&none, ELSEWHERE, stranger(&[], OutsideDm::Requests)),
            DmPolicy::Refuse
        );
    }

    #[test]
    fn partners_admit_listed_servers_and_this_one() {
        let partners = operator(DmInbound::Partners, DmOutbound::Allowed);
        assert_eq!(
            inbound(&partners, PARTNER, stranger(&[], OutsideDm::Requests)),
            DmPolicy::Request
        );
        assert_eq!(
            inbound(&partners, HERE, stranger(&[], OutsideDm::Requests)),
            DmPolicy::Request
        );
        assert_eq!(
            inbound(&partners, ELSEWHERE, stranger(&[], OutsideDm::Requests)),
            DmPolicy::Refuse
        );
    }

    #[test]
    fn contacts_skip_floors_but_not_the_server_boundary() {
        let contact = |workspaces| LocalRecipient {
            contact: true,
            shared: &[],
            workspaces,
            outside: OutsideDm::None,
        };
        let closed = [workspace(OutsideFloor::Closed, MemberDm::Direct, None)];
        assert_eq!(
            inbound(&defaults(), ELSEWHERE, contact(&closed)),
            DmPolicy::Deliver
        );
        let none = operator(DmInbound::None, DmOutbound::Allowed);
        assert_eq!(inbound(&none, HERE, contact(&[])), DmPolicy::Deliver);
        assert_eq!(inbound(&none, ELSEWHERE, contact(&[])), DmPolicy::Refuse);
        let partners = operator(DmInbound::Partners, DmOutbound::Allowed);
        assert_eq!(inbound(&partners, PARTNER, contact(&[])), DmPolicy::Deliver);
        assert_eq!(
            inbound(&partners, ELSEWHERE, contact(&[])),
            DmPolicy::Refuse
        );
    }

    #[test]
    fn members_follow_the_recipients_choice_or_the_workspace_default() {
        let member = |default, choice| {
            [shared(
                ServerMemberRole::Member,
                workspace(OutsideFloor::Closed, default, choice),
            )]
        };
        let co_member = |shared: &[SharedWorkspace]| {
            inbound(
                &operator(DmInbound::None, DmOutbound::None),
                ELSEWHERE,
                LocalRecipient {
                    contact: false,
                    shared,
                    workspaces: &[],
                    outside: OutsideDm::None,
                },
            )
        };
        assert_eq!(
            co_member(&member(MemberDm::Direct, None)),
            DmPolicy::Deliver
        );
        assert_eq!(
            co_member(&member(MemberDm::Requests, None)),
            DmPolicy::Request
        );
        assert_eq!(
            co_member(&member(MemberDm::Direct, Some(MemberDm::Requests))),
            DmPolicy::Request
        );
        assert_eq!(
            co_member(&member(MemberDm::Requests, Some(MemberDm::Direct))),
            DmPolicy::Deliver
        );
        // One shared workspace where the recipient takes DMs directly is enough.
        let two = [
            member(MemberDm::Requests, None)[0].clone(),
            member(MemberDm::Direct, None)[0].clone(),
        ];
        assert_eq!(co_member(&two), DmPolicy::Deliver);
    }

    #[test]
    fn owners_moderators_and_contacts_among_members_are_delivered() {
        for role in [ServerMemberRole::Owner, ServerMemberRole::Moderator] {
            let shared = [shared(
                role,
                workspace(
                    OutsideFloor::None,
                    MemberDm::Requests,
                    Some(MemberDm::Requests),
                ),
            )];
            let recipient = LocalRecipient {
                contact: false,
                shared: &shared,
                workspaces: &[],
                outside: OutsideDm::Requests,
            };
            assert_eq!(inbound(&defaults(), HERE, recipient), DmPolicy::Deliver);
        }
        let shared = [shared(
            ServerMemberRole::Guest,
            workspace(OutsideFloor::None, MemberDm::Requests, None),
        )];
        let recipient = LocalRecipient {
            contact: true,
            shared: &shared,
            workspaces: &[],
            outside: OutsideDm::Requests,
        };
        assert_eq!(inbound(&defaults(), HERE, recipient), DmPolicy::Deliver);
    }

    #[test]
    fn outbound_follows_the_operator() {
        assert_eq!(outbound(&defaults(), HERE, ELSEWHERE), DmPolicy::Deliver);
        let partners = operator(DmInbound::Requests, DmOutbound::Partners);
        assert_eq!(outbound(&partners, HERE, PARTNER), DmPolicy::Deliver);
        assert_eq!(outbound(&partners, HERE, ELSEWHERE), DmPolicy::Refuse);
        let none = operator(DmInbound::Requests, DmOutbound::None);
        assert_eq!(outbound(&none, HERE, PARTNER), DmPolicy::Refuse);
        // A DM between two people here is no outbound DM.
        assert_eq!(outbound(&none, HERE, HERE), DmPolicy::Deliver);
    }

    #[test]
    fn the_closing_floor_names_the_operator_first_then_a_workspace() {
        let closed = workspace(OutsideFloor::Closed, MemberDm::Direct, None);
        let workspaces = [
            workspace(OutsideFloor::Requests, MemberDm::Direct, None),
            closed.clone(),
        ];
        assert_eq!(closing_floor(&defaults(), &workspaces[..1]), None);
        assert_eq!(
            closing_floor(&defaults(), &workspaces),
            Some(DmFloorSourceDto::Workspace {
                workspace_id: closed.server_id,
                name: "Acme".to_string(),
            })
        );
        assert_eq!(
            closing_floor(&operator(DmInbound::None, DmOutbound::Allowed), &workspaces),
            Some(DmFloorSourceDto::Operator)
        );
    }
}
