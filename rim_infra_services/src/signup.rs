//! Account creation shared by every signup path: the password signups in `auth.rs` and key-only
//! signup in `key_auth.rs`. A registration token is checked, claimed and announced the same way
//! whichever credential the new account signs in with.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use rim_auth::csrf::generate_csrf_token;
use rim_auth::session_token::{generate_session_token, hash_token};
use rim_core::account_deletion::model::TOMBSTONE_USERNAME_PREFIX;
use rim_core::auth::model::{InstanceInvite, Invite, NewSession, Session};
use rim_core::auth::repository::{InstanceInviteRepository, InviteRepository, SessionRepository};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::default_workspace::repository::DefaultWorkspaceRepository;
use rim_core::dm_policy::repository::DmPolicyRepository;
use rim_core::moderation::repository::ModerationRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::{NewUser, User, UserId};
use rim_core::user::repository::UserRepository;
use rim_services::auth::command::SignupToken;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::{ServiceError, ServiceResult};

use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::reserved_names::ReservedNames;
use crate::util::{add_workspace_member, ensure_not_banned};

/// The repositories a registration token is checked and claimed with. Inside a transaction,
/// built from the transaction's handle.
#[derive(Clone)]
pub struct SignupRepos {
    pub instance_invite_repo: Arc<dyn InstanceInviteRepository>,
    pub invite_repo: Arc<dyn InviteRepository>,
    /// Finds the default workspace a new account joins.
    pub server_repo: Arc<dyn ServerRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    /// Ban checks on the workspace-invite join.
    pub moderation_repo: Arc<dyn ModerationRepository>,
    /// Records the invite chain on the new account, and grants the instance operator role an
    /// owner claim carries.
    pub user_repo: Arc<dyn UserRepository>,
    /// The DM policy a claimed default workspace starts with.
    pub dm_policy_repo: Arc<dyn DmPolicyRepository>,
    /// The claimed default workspace and its owner claim.
    pub default_workspace_repo: Arc<dyn DefaultWorkspaceRepository>,
}

/// A registration token that passed its checks before the transaction, to be claimed inside it.
#[derive(Debug, Clone)]
pub(crate) enum CheckedToken {
    InstanceInvite(InstanceInvite),
    WorkspaceInvite(Invite),
}

/// The refusal of a password signup with another address than its email-bound instance
/// invite's.
pub(crate) const EMAIL_BOUND_INVITE_REFUSAL: &str =
    "this invite admits only the email address it was issued for";

/// The refusal of a password signup on a server whose signups need a key.
pub(crate) const KEYS_REQUIRED_REFUSAL: &str =
    "password signup is turned off on this server; sign up with a key";

/// The refusal of a signup with an address an operator blocked.
pub(crate) const BLOCKED_EMAIL_REFUSAL: &str = "this email cannot sign up here";

/// Prefix reserved for people from other servers held here as remote users; local signups
/// may not use it, in either case.
pub(crate) const REMOTE_USERNAME_PREFIX: &str = "r_";

/// Check a username and return the form it is stored and compared in: trimmed and folded to
/// ASCII lowercase. The trimmed name is 3 to 32 characters of ASCII letters, digits, `_` and
/// `-`, not starting with the remote users' `r_` prefix or the purged accounts' `deleted-`
/// prefix in either case.
///
/// # Errors
/// `Validation` naming the rule that failed.
pub(crate) fn normalize_username(username: &str) -> ServiceResult<String> {
    let trimmed = username.trim();
    let len = trimmed.len();
    if !(3..=32).contains(&len) {
        return Err(ServiceError::Validation(
            "username must be between 3 and 32 characters".to_string(),
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ServiceError::Validation(
            "username may only contain alphanumeric characters, underscores, or hyphens"
                .to_string(),
        ));
    }
    let normalized = trimmed.to_ascii_lowercase();
    if normalized.starts_with(REMOTE_USERNAME_PREFIX) {
        return Err(ServiceError::Validation(format!(
            "usernames starting with {REMOTE_USERNAME_PREFIX} are reserved for people from other servers"
        )));
    }
    if normalized.starts_with(TOMBSTONE_USERNAME_PREFIX) {
        return Err(ServiceError::Validation(format!(
            "usernames starting with {TOMBSTONE_USERNAME_PREFIX} are reserved for deleted accounts"
        )));
    }
    Ok(normalized)
}

/// Normalise an optional display name, falling back to `username` when absent or blank after
/// trim. A non-empty (post-trim) value is capped at 80 characters.
///
/// # Errors
/// `Validation("display name length")` when the trimmed value is longer than 80 characters.
///
/// # Examples
/// ```ignore
/// assert_eq!(normalize_display_name(None, "alice")?, "alice");
/// assert_eq!(normalize_display_name(Some("   "), "alice")?, "alice");
/// assert_eq!(normalize_display_name(Some(" Alice Smith "), "alice")?, "Alice Smith");
/// ```
pub(crate) fn normalize_display_name(input: Option<&str>, username: &str) -> ServiceResult<String> {
    match input {
        None => Ok(username.to_string()),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(username.to_string());
            }
            if trimmed.len() > 80 {
                return Err(ServiceError::Validation("display name length".to_string()));
            }
            Ok(trimmed.to_string())
        }
    }
}

/// Refuse a signup whose username or resolved display name is reserved on this instance.
///
/// # Errors
/// `Validation` ("this name is reserved").
pub(crate) fn check_reserved(
    reserved: &ReservedNames,
    username: &str,
    display_name: &str,
) -> ServiceResult<()> {
    reserved.check(username)?;
    reserved.check(display_name)
}

/// Count an open-mode signup against the caller's IP; `RateLimited` once over the
/// limit, with the `signup_rate_limited` audit event.
pub(crate) fn check_open_signup_rate(
    limiter: &RegistrationRateLimiter,
    remote_ip: &str,
) -> ServiceResult<()> {
    if let RateLimitDecision::Limited {
        retry_after_seconds,
    } = limiter.check_and_record(remote_ip, "signup", Utc::now())
    {
        // Audited so operators see rate-limited signups.
        tracing::info!(
            target: "audit",
            event = "signup_rate_limited",
            remote_ip = %remote_ip,
            retry_after_seconds = %retry_after_seconds,
            "open-mode signup rate limited"
        );
        crate::metrics::rate_limited("signup");
        return Err(ServiceError::RateLimited {
            retry_after_seconds,
        });
    }
    Ok(())
}

/// Look a registration token up and check it is still usable, outside any transaction.
/// `email` is the normalised address a password signup gives, `None` for a key-only signup,
/// which takes the address of an email-bound instance invite.
///
/// # Errors
/// As [`check_instance_invite`] and [`check_workspace_invite`].
pub(crate) async fn check_token(
    repos: &SignupRepos,
    token: &SignupToken,
    email: Option<&str>,
) -> ServiceResult<CheckedToken> {
    Ok(match token {
        SignupToken::InstanceInvite(raw) => CheckedToken::InstanceInvite(
            check_instance_invite(repos.instance_invite_repo.as_ref(), raw, email).await?,
        ),
        SignupToken::WorkspaceInvite(raw) => CheckedToken::WorkspaceInvite(
            check_workspace_invite(repos.invite_repo.as_ref(), raw).await?,
        ),
    })
}

/// The instance invite `raw`, if it is unrevoked, not used up, unexpired and, when bound to an
/// address and `email` is given, presented with that address (compared trimmed and
/// case-insensitively). `email` is `None` for a key-only signup, which an email-bound invite
/// admits with the invite's address. The claim checks the uses again.
///
/// # Errors
/// `InvalidToken` when unknown, revoked or used up, `ExpiredToken` when expired, `Validation`
/// when the invite is bound to another address than `email`.
pub(crate) async fn check_instance_invite(
    repo: &dyn InstanceInviteRepository,
    raw: &str,
    email: Option<&str>,
) -> ServiceResult<InstanceInvite> {
    let invite = repo
        .get_by_token_hash(&hash_token(raw))
        .await?
        .ok_or(ServiceError::InvalidToken)?;
    if invite.revoked_at.is_some() || invite.use_count >= invite.max_uses {
        return Err(ServiceError::InvalidToken);
    }
    if invite.expires_at < Utc::now() {
        return Err(ServiceError::ExpiredToken);
    }
    if let (Some(bound), Some(email)) = (&invite.email, email)
        && !email.trim().eq_ignore_ascii_case(bound.trim())
    {
        tracing::info!(
            target: "audit",
            event = "instance_invite_email_mismatch",
            instance_invite_id = %invite.id,
            "signup refused: the instance invite is bound to another email address"
        );
        return Err(ServiceError::Validation(
            EMAIL_BOUND_INVITE_REFUSAL.to_string(),
        ));
    }
    Ok(invite)
}

/// The workspace invite `raw`, if it is unrevoked, not used up and unexpired. The claim checks
/// the uses again.
///
/// # Errors
/// `InvalidToken` when unknown, revoked or used up, `ExpiredToken` when expired.
pub(crate) async fn check_workspace_invite(
    repo: &dyn InviteRepository,
    raw: &str,
) -> ServiceResult<Invite> {
    let invite = repo
        .get_by_token_hash(&hash_token(raw))
        .await?
        .ok_or(ServiceError::InvalidToken)?;
    if invite.revoked_at.is_some() || invite.use_count >= invite.max_uses {
        return Err(ServiceError::InvalidToken);
    }
    if let Some(expires_at) = invite.expires_at
        && expires_at < Utc::now()
    {
        return Err(ServiceError::ExpiredToken);
    }
    Ok(invite)
}

/// Claim a checked token for the new user `user_id`, inside the signup's transaction, and
/// record on the account the invite and its creator. A workspace invite also makes the user a
/// member of its server. Returns that server, `None` for the other kinds (the account starts
/// with no workspace).
///
/// # Errors
/// - Instance invite: `InvalidToken` when it was used up since the check (the double-spend
///   race; the caller's transaction rolls back).
/// - Workspace invite: `PermissionDenied` when the user is banned there, `ExpiredToken` when the
///   invite expired since the check, `InvalidToken` when it was used up or revoked since.
pub(crate) async fn claim_token(
    repos: &SignupRepos,
    token: &CheckedToken,
    user_id: UserId,
    max_members: Option<u32>,
) -> ServiceResult<Option<ServerId>> {
    match token {
        CheckedToken::InstanceInvite(invite) => {
            if !repos.instance_invite_repo.try_consume(invite.id).await? {
                // Exhausted since the check — treat as invalid.
                return Err(ServiceError::InvalidToken);
            }
            repos
                .user_repo
                .record_invite_chain(user_id, invite.id.into(), invite.created_by_user_id)
                .await?;
            Ok(None)
        }
        CheckedToken::WorkspaceInvite(invite) => {
            // A brand-new account cannot carry a ban yet; checked anyway so every join path
            // refuses banned users the same way.
            ensure_not_banned(repos.moderation_repo.as_ref(), invite.server_id, user_id).await?;
            add_workspace_member(
                repos.server_member_repo.as_ref(),
                repos.conversation_repo.as_ref(),
                repos.conversation_member_repo.as_ref(),
                invite.server_id,
                user_id,
                invite.role,
                max_members,
            )
            .await?;
            if !repos.invite_repo.try_consume(invite.id).await? {
                // Expired since the check reads like an expired invite; used up or revoked
                // reads like unknown.
                if invite.expires_at.is_some_and(|at| at <= Utc::now()) {
                    return Err(ServiceError::ExpiredToken);
                }
                return Err(ServiceError::InvalidToken);
            }
            repos
                .user_repo
                .record_invite_chain(user_id, invite.id.into(), Some(invite.created_by_user_id))
                .await?;
            Ok(Some(invite.server_id))
        }
    }
}

/// After the signup committed: the token's audit event, and for a workspace invite the
/// `MemberJoined` event other members see the newcomer by.
pub(crate) fn announce_claimed(
    token: &CheckedToken,
    user_id: UserId,
    username: &str,
    observers: &Observers,
) {
    match token {
        CheckedToken::InstanceInvite(invite) => {
            tracing::info!(
                target: "audit",
                event = "instance_invite_consumed",
                instance_invite_id = %invite.id,
                consumed_by_user_id = %user_id,
                "instance invite consumed"
            );
            tracing::info!(user_id = %user_id, "user created via instance invite");
        }
        CheckedToken::WorkspaceInvite(invite) => {
            observers.emit(WorkspaceEvent::MemberJoined {
                workspace_id: invite.server_id,
                user_id,
                username: username.to_string(),
            });
            tracing::info!(
                target: "audit",
                event = "accept_invite_success",
                user_id = %user_id,
                invite_id = %invite.id,
                server_id = %invite.server_id,
                "user created via workspace invite"
            );
        }
    }
}

/// Create a user with `user_repo` (the transaction's). Checks the username, then the email
/// with [`check_signup_email`] when there is one, before inserting. The email must already be
/// normalised and the display name resolved.
///
/// # Errors
/// - `Conflict` when the username is taken, or the insert hits a unique index (a race on the
///   username, the email or the principal id).
/// - As [`check_signup_email`].
pub(crate) async fn create_user(
    user_repo: &dyn UserRepository,
    new_user: NewUser,
) -> ServiceResult<User> {
    if user_repo
        .find_by_username(&new_user.username)
        .await?
        .is_some()
    {
        return Err(ServiceError::Conflict(
            "username already exists".to_string(),
        ));
    }
    if let Some(email) = &new_user.email {
        check_signup_email(user_repo, email, &new_user.username).await?;
    }

    let user = user_repo.create_user(new_user).await.map_err(|e| {
        let msg = e.to_string();
        if msg.contains("unique") || msg.contains("duplicate") {
            return RepoError::new(RepoErrorKind::Conflict);
        }
        e
    })?;
    Ok(user)
}

/// Refuse a signup's normalised `email` when an operator blocked it or another account holds
/// it (case-insensitive).
///
/// # Errors
/// - `Validation` ([`BLOCKED_EMAIL_REFUSAL`]) when an operator blocked the email.
/// - `EmailTaken` when the email is in use.
pub(crate) async fn check_signup_email(
    user_repo: &dyn UserRepository,
    email: &str,
    username: &str,
) -> ServiceResult<()> {
    if user_repo.is_email_blocked(email).await? {
        tracing::info!(
            target: "audit",
            event = "blocked_email_refused",
            action = "signup",
            username = %username,
            "signup refused: the email is blocked on this server"
        );
        return Err(ServiceError::Validation(BLOCKED_EMAIL_REFUSAL.to_string()));
    }
    if user_repo.find_by_email(email).await?.is_some() {
        // Audited so operators can detect enumeration attacks or signup bot activity.
        tracing::info!(
            target: "audit",
            event = "email_taken_attempt",
            email = %email,
            username = %username,
            "signup rejected: email already in use"
        );
        return Err(ServiceError::EmailTaken);
    }
    Ok(())
}

/// Record on `user_id` the terms URL the signup accepted, stamped now; nothing when it accepted
/// none. Runs on the signup's transaction.
///
/// # Errors
/// Propagates storage errors.
pub(crate) async fn record_terms(
    user_repo: &dyn UserRepository,
    user_id: UserId,
    accepted: Option<String>,
) -> ServiceResult<()> {
    if let Some(url) = accepted {
        user_repo
            .record_terms_acceptance(user_id, url, Utc::now())
            .await?;
    }
    Ok(())
}

/// Insert a session for `user_id` with `session_repo`, returning it with its raw token and
/// CSRF token.
///
/// # Transaction contract
/// `session_repo` must share the transaction that created the user; otherwise the INSERT runs
/// on a connection that cannot see the uncommitted user row and fails the `fk-session-user`
/// foreign key.
///
/// # Errors
/// Propagates storage errors.
pub(crate) async fn create_session(
    session_repo: &dyn SessionRepository,
    session_ttl: Duration,
    user_id: UserId,
    user_agent: Option<String>,
    ip: Option<String>,
) -> ServiceResult<(Session, String, String)> {
    let raw_token = generate_session_token();
    let token_hash = hash_token(&raw_token);
    let expires_at: DateTime<Utc> = Utc::now() + session_ttl;
    let session = session_repo
        .create_session(NewSession {
            user_id,
            token_hash,
            expires_at,
            user_agent,
            ip,
        })
        .await?;
    let csrf = generate_csrf_token(&raw_token);
    Ok((session, raw_token, csrf))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use rim_core::auth::model::{InstanceInviteId, InviteId, SessionId};
    use rim_core::auth::repository::{
        MockInstanceInviteRepository, MockInviteRepository, MockSessionRepository,
    };
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::moderation::repository::MockModerationRepository;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::repository::MockUserRepository;
    use rim_services::events::Broadcaster;

    use super::*;

    /// Keeps every event broadcast, so a test can check what was announced.
    #[derive(Default)]
    struct Recording(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for Recording {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn repos() -> SignupRepos {
        SignupRepos {
            instance_invite_repo: Arc::new(MockInstanceInviteRepository::new()),
            invite_repo: Arc::new(MockInviteRepository::new()),
            server_repo: Arc::new(rim_core::server::repository::MockServerRepository::new()),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: Arc::new(MockModerationRepository::new()),
            user_repo: Arc::new(MockUserRepository::new()),
            dm_policy_repo: Arc::new(
                rim_core::dm_policy::repository::MockDmPolicyRepository::new(),
            ),
            default_workspace_repo: Arc::new(
                rim_core::default_workspace::repository::MockDefaultWorkspaceRepository::new(),
            ),
        }
    }

    /// Every invite chain a [`chain_recorder`] was asked to write: account, invite, creator.
    pub(crate) type ChainLog = Arc<Mutex<Vec<(UserId, uuid::Uuid, Option<UserId>)>>>;

    /// A user repository recording every invite chain written to it.
    pub(crate) fn chain_recorder() -> (MockUserRepository, ChainLog) {
        let seen: ChainLog = Arc::new(Mutex::new(Vec::new()));
        let mut repo = MockUserRepository::new();
        let sink = Arc::clone(&seen);
        repo.expect_record_invite_chain()
            .returning(move |user, invite, by| {
                sink.lock().unwrap().push((user, invite, by));
                Ok(())
            });
        (repo, seen)
    }

    fn instance_invite(expires_in: Duration, revoked: bool) -> InstanceInvite {
        InstanceInvite {
            id: InstanceInviteId::new(),
            created_by_user_id: Some(UserId::new()),
            token_hash: hash_token("raw"),
            expires_at: Utc::now() + expires_in,
            max_uses: 1,
            use_count: 0,
            revoked_at: revoked.then(Utc::now),
            created_at: Utc::now(),
            email: None,
        }
    }

    fn workspace_invite(expires_in: Option<Duration>, revoked: bool) -> Invite {
        Invite {
            id: InviteId::new(),
            server_id: ServerId::new(),
            created_by_user_id: UserId::new(),
            token_hash: hash_token("raw"),
            expires_at: expires_in.map(|d| Utc::now() + d),
            max_uses: 1,
            use_count: 0,
            revoked_at: revoked.then(Utc::now),
            created_at: Utc::now(),
            role: ServerMemberRole::Member,
        }
    }

    fn new_user(email: Option<&str>) -> NewUser {
        NewUser {
            username: "alice".to_string(),
            password: Some("hash".to_string()),
            email: email.map(str::to_string),
            display_name: None,
            key_identity: None,
        }
    }

    #[test]
    fn normalize_username_accepts_and_refuses() {
        assert_eq!(normalize_username("alice_01-x").unwrap(), "alice_01-x");
        assert!(matches!(
            normalize_username("ab"),
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            normalize_username("a".repeat(33).as_str()),
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            normalize_username("ali ce"),
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            normalize_username("alicé"),
            Err(ServiceError::Validation(_))
        ));
        for reserved in [
            "r_alice",
            "R_alice",
            " r_x1 ",
            "deleted-0199d1a0",
            "Deleted-x",
            "DELETED-X",
        ] {
            assert!(
                matches!(
                    normalize_username(reserved),
                    Err(ServiceError::Validation(_))
                ),
                "{reserved}"
            );
        }
        assert_eq!(normalize_username("ra_alice").unwrap(), "ra_alice");
        assert_eq!(normalize_username("r-alice").unwrap(), "r-alice");
        assert_eq!(
            normalize_username("deleted_alice").unwrap(),
            "deleted_alice"
        );
    }

    #[test]
    fn normalize_username_folds_to_lowercase() {
        assert_eq!(normalize_username("Alice").unwrap(), "alice");
        assert_eq!(normalize_username("ALICE_01-X").unwrap(), "alice_01-x");
        assert_eq!(normalize_username("aLiCe").unwrap(), "alice");
    }

    #[test]
    fn normalize_username_checks_the_length_after_trimming() {
        assert_eq!(normalize_username("  Bob  ").unwrap(), "bob");
        assert!(matches!(
            normalize_username("  ab  "),
            Err(ServiceError::Validation(_))
        ));
        let longest = "A".repeat(32);
        assert_eq!(
            normalize_username(&format!(" {longest} ")).unwrap(),
            "a".repeat(32)
        );
        assert!(matches!(
            normalize_username(&format!(" {longest}A ")),
            Err(ServiceError::Validation(_))
        ));
    }

    #[test]
    fn normalize_display_name_falls_back_trims_and_caps() {
        assert_eq!(normalize_display_name(None, "alice").unwrap(), "alice");
        assert_eq!(
            normalize_display_name(Some("  "), "alice").unwrap(),
            "alice"
        );
        assert_eq!(
            normalize_display_name(Some(" Alice Smith "), "alice").unwrap(),
            "Alice Smith"
        );
        assert!(normalize_display_name(Some(&"x".repeat(80)), "alice").is_ok());
        assert!(matches!(
            normalize_display_name(Some(&"x".repeat(81)), "alice"),
            Err(ServiceError::Validation(_))
        ));
    }

    #[test]
    fn open_signup_rate_is_limited_per_ip() {
        let limiter = RegistrationRateLimiter::new(3600, 1);
        assert!(check_open_signup_rate(&limiter, "192.0.2.1").is_ok());
        assert!(matches!(
            check_open_signup_rate(&limiter, "192.0.2.1"),
            Err(ServiceError::RateLimited { .. })
        ));
        assert!(check_open_signup_rate(&limiter, "192.0.2.2").is_ok());
    }

    #[tokio::test]
    async fn check_token_instance_invite_outcomes() {
        for (invite, expected) in [
            (Some(instance_invite(Duration::minutes(5), false)), None),
            (
                Some(instance_invite(Duration::minutes(5), true)),
                Some("InvalidToken"),
            ),
            (
                Some(instance_invite(Duration::minutes(-5), false)),
                Some("ExpiredToken"),
            ),
            (None, Some("InvalidToken")),
        ] {
            let mut repo = MockInstanceInviteRepository::new();
            repo.expect_get_by_token_hash()
                .returning(move |_| Ok(invite.clone()));
            let mut r = repos();
            r.instance_invite_repo = Arc::new(repo);
            let res = check_token(&r, &SignupToken::InstanceInvite("raw".to_string()), None).await;
            match expected {
                None => assert!(matches!(res, Ok(CheckedToken::InstanceInvite(_)))),
                Some("InvalidToken") => assert!(matches!(res, Err(ServiceError::InvalidToken))),
                Some(_) => assert!(matches!(res, Err(ServiceError::ExpiredToken))),
            }
        }
    }

    #[tokio::test]
    async fn an_email_bound_instance_invite_admits_only_its_address_or_a_key_signup() {
        for (email, admitted) in [
            (Some("ops@example.com"), true),
            (Some(" Ops@Example.COM "), true),
            (Some("other@example.com"), false),
            (None, true),
        ] {
            let mut repo = MockInstanceInviteRepository::new();
            repo.expect_get_by_token_hash().returning(|_| {
                let mut invite = instance_invite(Duration::minutes(5), false);
                invite.email = Some("ops@example.com".to_string());
                Ok(Some(invite))
            });
            let res = check_instance_invite(&repo, "raw", email).await;
            if admitted {
                assert!(res.is_ok(), "{email:?}");
            } else {
                assert!(
                    matches!(&res, Err(ServiceError::Validation(m)) if m == EMAIL_BOUND_INVITE_REFUSAL),
                    "{email:?}: {res:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn check_token_workspace_invite_outcomes() {
        for (invite, expected) in [
            (Some(workspace_invite(None, false)), None),
            (Some(workspace_invite(None, true)), Some("InvalidToken")),
            (
                Some(workspace_invite(Some(Duration::minutes(-5)), false)),
                Some("ExpiredToken"),
            ),
            (None, Some("InvalidToken")),
        ] {
            let mut repo = MockInviteRepository::new();
            repo.expect_get_by_token_hash()
                .returning(move |_| Ok(invite.clone()));
            let mut r = repos();
            r.invite_repo = Arc::new(repo);
            let res = check_token(&r, &SignupToken::WorkspaceInvite("raw".to_string()), None).await;
            match expected {
                None => assert!(matches!(res, Ok(CheckedToken::WorkspaceInvite(_)))),
                Some("InvalidToken") => assert!(matches!(res, Err(ServiceError::InvalidToken))),
                Some(_) => assert!(matches!(res, Err(ServiceError::ExpiredToken))),
            }
        }
    }

    #[tokio::test]
    async fn claim_token_instance_invite_refuses_an_exhausted_invite_and_records_the_chain() {
        for consumed in [true, false] {
            let mut repo = MockInstanceInviteRepository::new();
            repo.expect_try_consume().returning(move |_| Ok(consumed));
            let (users, chains) = chain_recorder();
            let mut r = repos();
            r.instance_invite_repo = Arc::new(repo);
            r.user_repo = Arc::new(users);
            let invite = instance_invite(Duration::minutes(5), false);
            let expected = (invite.id.into(), invite.created_by_user_id);
            let token = CheckedToken::InstanceInvite(invite);
            let user_id = UserId::new();
            let res = claim_token(&r, &token, user_id, None).await;
            if consumed {
                assert_eq!(res.unwrap(), None);
                assert_eq!(
                    *chains.lock().unwrap(),
                    vec![(user_id, expected.0, expected.1)]
                );
            } else {
                assert!(matches!(res, Err(ServiceError::InvalidToken)));
                assert!(chains.lock().unwrap().is_empty());
            }
        }
    }

    #[tokio::test]
    async fn claim_token_workspace_invite_joins_the_server() {
        let invite = workspace_invite(None, false);
        let server_id = invite.server_id;
        let user_id = UserId::new();

        let mut moderation = MockModerationRepository::new();
        moderation
            .expect_get_active_ban()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        let mut members = MockServerMemberRepository::new();
        members
            .expect_add_member()
            .times(1)
            .returning(|server_id, user_id, role| {
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role: role,
                    joined_at: Utc::now(),
                })
            });
        let mut conversations = MockConversationRepository::new();
        conversations
            .expect_list_public_channel_ids()
            .returning(|_| Box::pin(async { Ok(Vec::new()) }));
        let mut invites = MockInviteRepository::new();
        invites
            .expect_try_consume()
            .times(1)
            .returning(|_| Ok(true));

        let (users, chains) = chain_recorder();
        let invite_id: uuid::Uuid = invite.id.into();
        let creator = invite.created_by_user_id;

        let mut r = repos();
        r.moderation_repo = Arc::new(moderation);
        r.server_member_repo = Arc::new(members);
        r.conversation_repo = Arc::new(conversations);
        r.invite_repo = Arc::new(invites);
        r.user_repo = Arc::new(users);
        let res = claim_token(&r, &CheckedToken::WorkspaceInvite(invite), user_id, None).await;
        assert_eq!(res.unwrap(), Some(server_id));
        assert_eq!(
            *chains.lock().unwrap(),
            vec![(user_id, invite_id, Some(creator))]
        );
    }

    /// A workspace invite the consume refuses reads as expired when its expiry has passed
    /// since the check, and as unknown otherwise (used up or revoked since).
    #[tokio::test]
    async fn claim_token_workspace_invite_refused_by_the_consume() {
        for (expires_in, expired) in [
            (Some(Duration::seconds(-1)), true),
            (Some(Duration::minutes(5)), false),
            (None, false),
        ] {
            let mut moderation = MockModerationRepository::new();
            moderation
                .expect_get_active_ban()
                .returning(|_, _, _| Box::pin(async { Ok(None) }));
            let mut members = MockServerMemberRepository::new();
            members
                .expect_add_member()
                .returning(|server_id, user_id, role| {
                    Ok(ServerMember {
                        id: ServerMemberId::new(),
                        server_id,
                        user_id,
                        member_role: role,
                        joined_at: Utc::now(),
                    })
                });
            let mut conversations = MockConversationRepository::new();
            conversations
                .expect_list_public_channel_ids()
                .returning(|_| Box::pin(async { Ok(Vec::new()) }));
            let mut invites = MockInviteRepository::new();
            invites.expect_try_consume().returning(|_| Ok(false));
            let (users, chains) = chain_recorder();

            let mut r = repos();
            r.moderation_repo = Arc::new(moderation);
            r.server_member_repo = Arc::new(members);
            r.conversation_repo = Arc::new(conversations);
            r.invite_repo = Arc::new(invites);
            r.user_repo = Arc::new(users);
            let token = CheckedToken::WorkspaceInvite(workspace_invite(expires_in, false));
            let res = claim_token(&r, &token, UserId::new(), None).await;
            if expired {
                assert!(matches!(res, Err(ServiceError::ExpiredToken)), "{res:?}");
            } else {
                assert!(matches!(res, Err(ServiceError::InvalidToken)), "{res:?}");
            }
            assert!(chains.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn claim_token_workspace_invite_refuses_a_banned_user() {
        let mut moderation = MockModerationRepository::new();
        moderation
            .expect_get_active_ban()
            .returning(|_, _, _| Box::pin(async { Err(RepoError::new(RepoErrorKind::Internal)) }));
        let mut r = repos();
        r.moderation_repo = Arc::new(moderation);
        let token = CheckedToken::WorkspaceInvite(workspace_invite(None, false));
        assert!(claim_token(&r, &token, UserId::new(), None).await.is_err());
    }

    #[test]
    fn announce_claimed_emits_member_joined_for_a_workspace_invite_only() {
        let seen = Arc::new(Recording::default());
        let observers = Observers::new(Arc::clone(&seen) as Arc<dyn Broadcaster>);
        let user_id = UserId::new();
        announce_claimed(
            &CheckedToken::InstanceInvite(instance_invite(Duration::minutes(5), false)),
            user_id,
            "alice",
            &observers,
        );
        assert!(seen.0.lock().unwrap().is_empty());
        let invite = workspace_invite(None, false);
        let server_id = invite.server_id;
        announce_claimed(
            &CheckedToken::WorkspaceInvite(invite),
            user_id,
            "alice",
            &observers,
        );
        let seen = seen.0.lock().unwrap();
        assert!(matches!(
            seen.as_slice(),
            [WorkspaceEvent::MemberJoined { workspace_id, user_id: u, username }]
                if *workspace_id == server_id && *u == user_id && username == "alice"
        ));
    }

    #[tokio::test]
    async fn create_user_checks_username_then_email() {
        let mut taken = MockUserRepository::new();
        taken
            .expect_find_by_username()
            .returning(|_| Ok(Some(user())));
        assert!(matches!(
            create_user(&taken, new_user(Some("a@example.com"))).await,
            Err(ServiceError::Conflict(_))
        ));

        let mut email_taken = MockUserRepository::new();
        email_taken
            .expect_find_by_username()
            .returning(|_| Ok(None));
        email_taken
            .expect_is_email_blocked()
            .returning(|_| Ok(false));
        email_taken
            .expect_find_by_email()
            .returning(|_| Ok(Some(user())));
        assert!(matches!(
            create_user(&email_taken, new_user(Some("a@example.com"))).await,
            Err(ServiceError::EmailTaken)
        ));
    }

    #[tokio::test]
    async fn create_user_refuses_a_blocked_email_before_checking_it_is_taken() {
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_username().returning(|_| Ok(None));
        repo.expect_is_email_blocked()
            .withf(|email| email == "a@example.com")
            .returning(|_| Ok(true));
        repo.expect_find_by_email().never();
        repo.expect_create_user().never();
        assert!(matches!(
            create_user(&repo, new_user(Some("a@example.com"))).await,
            Err(ServiceError::Validation(m)) if m == BLOCKED_EMAIL_REFUSAL
        ));
    }

    #[tokio::test]
    async fn create_user_without_email_skips_the_email_check() {
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_username().returning(|_| Ok(None));
        repo.expect_is_email_blocked().never();
        repo.expect_find_by_email().never();
        repo.expect_create_user().returning(|_| Ok(user()));
        assert!(create_user(&repo, new_user(None)).await.is_ok());
    }

    #[tokio::test]
    async fn create_user_turns_a_unique_violation_into_conflict() {
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_username().returning(|_| Ok(None));
        repo.expect_create_user().returning(|_| {
            Err(RepoError::with_message(
                RepoErrorKind::Internal,
                "duplicate key value violates unique constraint",
            ))
        });
        assert!(matches!(
            create_user(&repo, new_user(None)).await,
            Err(ServiceError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn create_session_returns_tokens_for_the_stored_session() {
        let user_id = UserId::new();
        let mut repo = MockSessionRepository::new();
        repo.expect_create_session().returning(|new| {
            Ok(Session {
                id: SessionId::new(),
                user_id: new.user_id,
                token_hash: new.token_hash,
                created_at: Utc::now(),
                expires_at: new.expires_at,
                last_seen_at: None,
                revoked_at: None,
                user_agent: new.user_agent,
                ip: new.ip,
                device_key_id: None,
            })
        });
        let (session, raw, csrf) = create_session(&repo, Duration::days(1), user_id, None, None)
            .await
            .unwrap();
        assert_eq!(session.user_id, user_id);
        assert_eq!(session.token_hash, hash_token(&raw));
        assert!(rim_auth::csrf::verify_csrf_token(&csrf, &raw));
    }

    pub(super) fn user() -> User {
        let now = Utc::now();
        User {
            id: UserId::new(),
            username: "alice".to_string(),
            password_hash: None,
            created_at: now,
            updated_at: now,
            last_login_at: None,
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: None,
            timezone: None,
            principal_id: Some("rimp1_x".to_string()),
            deletion_scheduled_for: None,
        }
    }
}
