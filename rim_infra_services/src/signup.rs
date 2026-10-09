//! Account creation shared by every signup path: the password signups in `auth.rs` and key-only
//! signup in `key_auth.rs`. A registration token is checked, claimed and announced the same way
//! whichever credential the new account signs in with.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use rim_auth::csrf::generate_csrf_token;
use rim_auth::session_token::{generate_session_token, hash_token};
use rim_core::auth::model::{BootstrapToken, InstanceInvite, Invite, NewSession, Session};
use rim_core::auth::repository::{
    BootstrapTokenRepository, InstanceInviteRepository, InviteRepository, SessionRepository,
};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::moderation::repository::ModerationRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::{NewUser, User, UserId};
use rim_core::user::repository::UserRepository;
use rim_services::auth::command::SignupToken;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::{ServiceError, ServiceResult};

use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::util::{add_workspace_member, ensure_not_banned};

/// The repositories a registration token is checked and claimed with. Inside a transaction,
/// built from the transaction's handle.
#[derive(Clone)]
pub struct SignupRepos {
    pub bootstrap_token_repo: Arc<dyn BootstrapTokenRepository>,
    pub instance_invite_repo: Arc<dyn InstanceInviteRepository>,
    pub invite_repo: Arc<dyn InviteRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    /// Ban checks on the workspace-invite join.
    pub moderation_repo: Arc<dyn ModerationRepository>,
}

/// A registration token that passed its checks before the transaction, to be claimed inside it.
#[derive(Debug, Clone)]
pub(crate) enum CheckedToken {
    Bootstrap(BootstrapToken),
    InstanceInvite(InstanceInvite),
    WorkspaceInvite(Invite),
}

/// Check a username: 3 to 32 characters after trimming, ASCII letters, digits, `_` and `-`.
///
/// # Errors
/// `Validation` naming the rule that failed.
pub(crate) fn validate_username(username: &str) -> ServiceResult<()> {
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
    Ok(())
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
            target = "audit",
            event = "signup_rate_limited",
            remote_ip = %remote_ip,
            retry_after_seconds = %retry_after_seconds,
            "open-mode signup rate limited"
        );
        return Err(ServiceError::RateLimited {
            retry_after_seconds,
        });
    }
    Ok(())
}

/// Look a registration token up and check it is still usable, outside any transaction.
///
/// # Errors
/// As [`check_bootstrap_token`], [`check_instance_invite`] and [`check_workspace_invite`].
pub(crate) async fn check_token(
    repos: &SignupRepos,
    token: &SignupToken,
) -> ServiceResult<CheckedToken> {
    Ok(match token {
        SignupToken::Bootstrap(raw) => CheckedToken::Bootstrap(
            check_bootstrap_token(repos.bootstrap_token_repo.as_ref(), raw).await?,
        ),
        SignupToken::InstanceInvite(raw) => CheckedToken::InstanceInvite(
            check_instance_invite(repos.instance_invite_repo.as_ref(), raw).await?,
        ),
        SignupToken::WorkspaceInvite(raw) => CheckedToken::WorkspaceInvite(
            check_workspace_invite(repos.invite_repo.as_ref(), raw).await?,
        ),
    })
}

/// The bootstrap token `raw`, if it is unused, unrevoked and unexpired.
///
/// # Errors
/// `InvalidToken` when unknown, used or revoked (indistinguishable on purpose),
/// `ExpiredToken` when expired.
pub(crate) async fn check_bootstrap_token(
    repo: &dyn BootstrapTokenRepository,
    raw: &str,
) -> ServiceResult<BootstrapToken> {
    let token = repo
        .get_by_token_hash(&hash_token(raw))
        .await?
        .ok_or(ServiceError::InvalidToken)?;
    if token.used_at.is_some() || token.revoked_at.is_some() {
        return Err(ServiceError::InvalidToken);
    }
    if token.expires_at < Utc::now() {
        return Err(ServiceError::ExpiredToken);
    }
    Ok(token)
}

/// The instance invite `raw`, if it is unrevoked and unexpired. Whether uses remain is decided
/// when it is claimed.
///
/// # Errors
/// `InvalidToken` when unknown or revoked, `ExpiredToken` when expired.
pub(crate) async fn check_instance_invite(
    repo: &dyn InstanceInviteRepository,
    raw: &str,
) -> ServiceResult<InstanceInvite> {
    let invite = repo
        .get_by_token_hash(&hash_token(raw))
        .await?
        .ok_or(ServiceError::InvalidToken)?;
    if invite.revoked_at.is_some() {
        return Err(ServiceError::InvalidToken);
    }
    if invite.expires_at < Utc::now() {
        return Err(ServiceError::ExpiredToken);
    }
    Ok(invite)
}

/// The workspace invite `raw`, if it is unrevoked and unexpired. Whether uses remain is decided
/// when it is claimed.
///
/// # Errors
/// `NotFound` when unknown, `PermissionDenied` when revoked or expired.
pub(crate) async fn check_workspace_invite(
    repo: &dyn InviteRepository,
    raw: &str,
) -> ServiceResult<Invite> {
    let invite = repo
        .get_by_token_hash(&hash_token(raw))
        .await?
        .ok_or(ServiceError::NotFound)?;
    if invite.revoked_at.is_some() {
        return Err(ServiceError::PermissionDenied);
    }
    if let Some(expires_at) = invite.expires_at
        && expires_at < Utc::now()
    {
        return Err(ServiceError::PermissionDenied);
    }
    Ok(invite)
}

/// Claim a checked token for the new user `user_id`, inside the signup's transaction. A
/// workspace invite also makes the user a member of its server. Returns that server, `None`
/// for the other kinds (the account starts with no workspace).
///
/// # Errors
/// - Bootstrap token and instance invite: `InvalidToken` when another request claimed it since
///   the check (the double-spend race; the caller's transaction rolls back).
/// - Workspace invite: `PermissionDenied` when the user is banned there or the invite was used
///   up since the check.
pub(crate) async fn claim_token(
    repos: &SignupRepos,
    token: &CheckedToken,
    user_id: UserId,
) -> ServiceResult<Option<ServerId>> {
    match token {
        CheckedToken::Bootstrap(token) => {
            // Atomic try-mark: of two requests claiming the token, only one wins.
            if !repos
                .bootstrap_token_repo
                .try_mark_used(token.id, user_id)
                .await?
            {
                tracing::info!(
                    target = "audit",
                    event = "bootstrap_token_double_spend_rejected",
                    bootstrap_token_id = %token.id,
                    "bootstrap token already consumed by concurrent request"
                );
                return Err(ServiceError::InvalidToken);
            }
            Ok(None)
        }
        CheckedToken::InstanceInvite(invite) => {
            if !repos.instance_invite_repo.try_consume(invite.id).await? {
                // Exhausted since we last checked — treat as invalid.
                return Err(ServiceError::InvalidToken);
            }
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
            )
            .await?;
            if !repos.invite_repo.try_consume(invite.id).await? {
                // Don't leak whether it's exhausted vs missing.
                return Err(ServiceError::PermissionDenied);
            }
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
        CheckedToken::Bootstrap(token) => {
            tracing::info!(
                target = "audit",
                event = "bootstrap_token_consumed",
                bootstrap_token_id = %token.id,
                consumed_by_user_id = %user_id,
                "bootstrap token consumed"
            );
            tracing::info!(user_id = %user_id, "user created via bootstrap token");
        }
        CheckedToken::InstanceInvite(invite) => {
            tracing::info!(
                target = "audit",
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
                target = "audit",
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
/// (case-insensitive) when there is one, before inserting. The email must already be
/// normalised and the display name resolved.
///
/// # Errors
/// - `Conflict` when the username is taken, or the insert hits a unique index (a race on the
///   username, the email or the principal id).
/// - `EmailTaken` when the email is in use.
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
    if let Some(email) = &new_user.email
        && user_repo.find_by_email(email).await?.is_some()
    {
        // Audited so operators can detect enumeration attacks or signup bot activity.
        tracing::info!(
            target = "audit",
            event = "email_taken_attempt",
            email = %email,
            username = %new_user.username,
            "signup rejected: email already in use"
        );
        return Err(ServiceError::EmailTaken);
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
mod tests {
    use std::sync::Mutex;

    use rim_core::auth::model::{BootstrapTokenId, InstanceInviteId, InviteId, SessionId};
    use rim_core::auth::repository::{
        MockBootstrapTokenRepository, MockInstanceInviteRepository, MockInviteRepository,
        MockSessionRepository,
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
            bootstrap_token_repo: Arc::new(MockBootstrapTokenRepository::new()),
            instance_invite_repo: Arc::new(MockInstanceInviteRepository::new()),
            invite_repo: Arc::new(MockInviteRepository::new()),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: Arc::new(MockModerationRepository::new()),
        }
    }

    fn bootstrap_token(expires_in: Duration, used: bool) -> BootstrapToken {
        BootstrapToken {
            id: BootstrapTokenId::new(),
            token_hash: hash_token("raw"),
            email: "admin@example.com".to_string(),
            expires_at: Utc::now() + expires_in,
            used_at: used.then(Utc::now),
            consumed_by_user_id: None,
            revoked_at: None,
            created_at: Utc::now(),
        }
    }

    fn instance_invite(expires_in: Duration, revoked: bool) -> InstanceInvite {
        InstanceInvite {
            id: InstanceInviteId::new(),
            created_by_user_id: UserId::new(),
            token_hash: hash_token("raw"),
            expires_at: Utc::now() + expires_in,
            max_uses: 1,
            use_count: 0,
            revoked_at: revoked.then(Utc::now),
            created_at: Utc::now(),
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
    fn validate_username_accepts_and_refuses() {
        assert!(validate_username("alice_01-x").is_ok());
        assert!(matches!(
            validate_username("ab"),
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            validate_username("a".repeat(33).as_str()),
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            validate_username("ali ce"),
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
    async fn check_token_bootstrap_outcomes() {
        for (token, expected) in [
            (Some(bootstrap_token(Duration::minutes(5), false)), None),
            (
                Some(bootstrap_token(Duration::minutes(5), true)),
                Some("InvalidToken"),
            ),
            (
                Some(bootstrap_token(Duration::minutes(-5), false)),
                Some("ExpiredToken"),
            ),
            (None, Some("InvalidToken")),
        ] {
            let mut repo = MockBootstrapTokenRepository::new();
            repo.expect_get_by_token_hash()
                .returning(move |_| Ok(token.clone()));
            let mut r = repos();
            r.bootstrap_token_repo = Arc::new(repo);
            let res = check_token(&r, &SignupToken::Bootstrap("raw".to_string())).await;
            match expected {
                None => assert!(matches!(res, Ok(CheckedToken::Bootstrap(_)))),
                Some("InvalidToken") => assert!(matches!(res, Err(ServiceError::InvalidToken))),
                Some(_) => assert!(matches!(res, Err(ServiceError::ExpiredToken))),
            }
        }
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
            let res = check_token(&r, &SignupToken::InstanceInvite("raw".to_string())).await;
            match expected {
                None => assert!(matches!(res, Ok(CheckedToken::InstanceInvite(_)))),
                Some("InvalidToken") => assert!(matches!(res, Err(ServiceError::InvalidToken))),
                Some(_) => assert!(matches!(res, Err(ServiceError::ExpiredToken))),
            }
        }
    }

    #[tokio::test]
    async fn check_token_workspace_invite_outcomes() {
        for (invite, expected) in [
            (Some(workspace_invite(None, false)), None),
            (Some(workspace_invite(None, true)), Some("PermissionDenied")),
            (
                Some(workspace_invite(Some(Duration::minutes(-5)), false)),
                Some("PermissionDenied"),
            ),
            (None, Some("NotFound")),
        ] {
            let mut repo = MockInviteRepository::new();
            repo.expect_get_by_token_hash()
                .returning(move |_| Ok(invite.clone()));
            let mut r = repos();
            r.invite_repo = Arc::new(repo);
            let res = check_token(&r, &SignupToken::WorkspaceInvite("raw".to_string())).await;
            match expected {
                None => assert!(matches!(res, Ok(CheckedToken::WorkspaceInvite(_)))),
                Some("NotFound") => assert!(matches!(res, Err(ServiceError::NotFound))),
                Some(_) => assert!(matches!(res, Err(ServiceError::PermissionDenied))),
            }
        }
    }

    #[tokio::test]
    async fn claim_token_bootstrap_refuses_a_double_spend() {
        for claimed in [true, false] {
            let mut repo = MockBootstrapTokenRepository::new();
            repo.expect_try_mark_used()
                .returning(move |_, _| Ok(claimed));
            let mut r = repos();
            r.bootstrap_token_repo = Arc::new(repo);
            let token = CheckedToken::Bootstrap(bootstrap_token(Duration::minutes(5), false));
            let res = claim_token(&r, &token, UserId::new()).await;
            if claimed {
                assert_eq!(res.unwrap(), None);
            } else {
                assert!(matches!(res, Err(ServiceError::InvalidToken)));
            }
        }
    }

    #[tokio::test]
    async fn claim_token_instance_invite_refuses_an_exhausted_invite() {
        for consumed in [true, false] {
            let mut repo = MockInstanceInviteRepository::new();
            repo.expect_try_consume().returning(move |_| Ok(consumed));
            let mut r = repos();
            r.instance_invite_repo = Arc::new(repo);
            let token = CheckedToken::InstanceInvite(instance_invite(Duration::minutes(5), false));
            let res = claim_token(&r, &token, UserId::new()).await;
            if consumed {
                assert_eq!(res.unwrap(), None);
            } else {
                assert!(matches!(res, Err(ServiceError::InvalidToken)));
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

        let mut r = repos();
        r.moderation_repo = Arc::new(moderation);
        r.server_member_repo = Arc::new(members);
        r.conversation_repo = Arc::new(conversations);
        r.invite_repo = Arc::new(invites);
        let res = claim_token(&r, &CheckedToken::WorkspaceInvite(invite), user_id).await;
        assert_eq!(res.unwrap(), Some(server_id));
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
        assert!(claim_token(&r, &token, UserId::new()).await.is_err());
    }

    #[test]
    fn announce_claimed_emits_member_joined_for_a_workspace_invite_only() {
        let seen = Arc::new(Recording::default());
        let observers = Observers::new(Arc::clone(&seen) as Arc<dyn Broadcaster>);
        let user_id = UserId::new();
        announce_claimed(
            &CheckedToken::Bootstrap(bootstrap_token(Duration::minutes(5), false)),
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
            .expect_find_by_email()
            .returning(|_| Ok(Some(user())));
        assert!(matches!(
            create_user(&email_taken, new_user(Some("a@example.com"))).await,
            Err(ServiceError::EmailTaken)
        ));
    }

    #[tokio::test]
    async fn create_user_without_email_skips_the_email_check() {
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_username().returning(|_| Ok(None));
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
        }
    }
}
