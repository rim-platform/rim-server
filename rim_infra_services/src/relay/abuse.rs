//! The operator's abuse tools: blocking principals and accounts on this server and the abuse
//! reports.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use sea_orm::DatabaseConnection;
use tracing::{info, instrument};

use rim_core::auth::repository::SessionRepository;
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::relay::model::{AbuseReport, AbuseReportId, NewAbuseReport, PrincipalBlock};
use rim_core::relay::repository::RelayAbuseRepository;
use rim_core::user::model::{InviteKind, User, UserId};
use rim_core::user::repository::UserRepository;
use rim_identity::is_principal_id;
use rim_infra_db::{TransactionScope, begin_transaction};
use rim_services::operator::OperatorCaller;
use rim_services::relay::dto::{
    AbuseReportDto, AccountRecordDto, AccountSummaryDto, CreatedInviteDto, FileAbuseReport,
    InviteeDto, PrincipalBlockDto, PrincipalBlockOutcome, PrincipalRecordDto,
    PrincipalUnblockOutcome,
};
use rim_services::relay::service::RelayAbuseService;
use rim_services::session_revocation::SessionRevocations;
use rim_services::{ServiceError, ServiceResult};

use crate::operator::require_operator;
use crate::repos::AppRepos;

/// Longest block or report reason, in characters.
const MAX_REASON_CHARS: usize = 1000;
/// How long a request ignored by a block is kept, so its conversation stays dropped.
const IGNORED_REQUEST_TTL: Duration = Duration::days(30);

#[derive(Clone)]
pub struct RelayAbuseRepos {
    pub relay_abuse_repo: Arc<dyn RelayAbuseRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub session_repo: Arc<dyn SessionRepository>,
    /// Accounts, their invite chain, their invites and blocked emails.
    pub user_repo: Arc<dyn UserRepository>,
}

pub struct DefaultRelayAbuseService {
    repos: RelayAbuseRepos,
    /// Runs a ban in one transaction; without it (unit tests) the repositories run as given.
    db: Option<DatabaseConnection>,
    /// Told of the accounts a block signs out, so their live connections close.
    session_revocations: Arc<dyn SessionRevocations>,
}

impl DefaultRelayAbuseService {
    pub fn new(repos: RelayAbuseRepos) -> Self {
        Self {
            repos,
            db: None,
            session_revocations: crate::session_revocation::unshared(),
        }
    }

    /// Report the accounts a block signs out to `revocations`.
    pub fn with_session_revocations(mut self, revocations: Arc<dyn SessionRevocations>) -> Self {
        self.session_revocations = revocations;
        self
    }

    pub fn with_db(mut self, db: DatabaseConnection) -> Self {
        self.db = Some(db);
        self
    }

    async fn begin(&self) -> ServiceResult<(Option<TransactionScope>, RelayAbuseRepos)> {
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_relay_abuse(),
            None => self.repos.clone(),
        };
        Ok((scope, repos))
    }

    async fn account_of(
        repos: &RelayAbuseRepos,
        principal_id: &str,
    ) -> ServiceResult<Option<User>> {
        match repos
            .key_identity_repo
            .find_user_by_principal_id(principal_id)
            .await?
        {
            Some(user_id) => Ok(repos.user_repo.get_by_id(user_id).await?),
            None => Ok(None),
        }
    }

    async fn summaries(
        repos: &RelayAbuseRepos,
        ids: &[UserId],
    ) -> ServiceResult<Vec<AccountSummaryDto>> {
        let users = repos.user_repo.get_by_ids(ids).await?;
        // In the order of `ids`.
        Ok(ids
            .iter()
            .filter_map(|id| users.iter().find(|u| u.id == *id).map(summary))
            .collect())
    }

    /// Whether the account's principal or email is blocked.
    pub(crate) async fn is_blocked(repos: &RelayAbuseRepos, user: &User) -> ServiceResult<bool> {
        if let Some(principal_id) = &user.principal_id
            && repos
                .relay_abuse_repo
                .is_principal_blocked(principal_id)
                .await?
        {
            return Ok(true);
        }
        match &user.email {
            Some(email) => Ok(repos.user_repo.is_email_blocked(email).await?),
            None => Ok(false),
        }
    }

    /// Block `principal_id` and the account `user`, whichever exist, for `reason`, on the
    /// transaction's repositories.
    async fn block_one(
        repos: &RelayAbuseRepos,
        principal_id: Option<String>,
        user: Option<&User>,
        reason: String,
    ) -> ServiceResult<PrincipalBlockOutcome> {
        // Blocked first, so a login racing the session sweep below is refused.
        let principal_block_new = match &principal_id {
            Some(p) => Some(repos.relay_abuse_repo.block_principal(p, &reason).await?),
            None => None,
        };
        let mut outcome = PrincipalBlockOutcome {
            principal_id: principal_id.clone(),
            newly_blocked: principal_block_new.unwrap_or(false),
            account: user.map(summary),
            reason: reason.clone(),
            ended_sessions: 0,
            ignored_requests: 0,
            revoked_invites: 0,
            blocked_email: None,
            invitees: Vec::new(),
        };
        if let Some(user) = user {
            // A password login creates its session under the same user row lock, so it either
            // commits before the sweep below or waits and sees the blocked email.
            repos.user_repo.lock_user(user.id).await?;
            outcome.ended_sessions = repos.session_repo.revoke_all_for_user(user.id).await?;
            outcome.revoked_invites = repos.user_repo.revoke_invites_created_by(user.id).await?;
            if let Some(email) = &user.email {
                let email_block_new = repos.user_repo.block_email(email, &reason).await?;
                outcome.newly_blocked = principal_block_new.unwrap_or(email_block_new);
                outcome.blocked_email = Some(email.clone());
            }
            let invitees = repos.user_repo.invited_users(user.id).await?;
            outcome.invitees = Self::summaries(repos, &invitees).await?;
        }
        if let Some(p) = &principal_id {
            outcome.ignored_requests = repos
                .relay_abuse_repo
                .ignore_requests_from(p, Utc::now() + IGNORED_REQUEST_TTL)
                .await?;
            // The reason stays on record as a report, resolved by the block itself.
            let report = repos
                .relay_abuse_repo
                .insert_report(NewAbuseReport {
                    reporter_user_id: None,
                    subject_principal_id: Some(p.clone()),
                    subject_server_id: None,
                    request_id: None,
                    reason: format!("blocked: {reason}"),
                })
                .await?;
            repos.relay_abuse_repo.resolve_report(report.id).await?;
        }
        Ok(outcome)
    }

    /// The accounts `root`'s invites created, recursively, depth first.
    async fn invite_tree(repos: &RelayAbuseRepos, root: UserId) -> ServiceResult<Vec<InviteeDto>> {
        let mut tree = Vec::new();
        let mut seen = HashSet::from([root]);
        let mut stack = vec![(root, 0u32)];
        while let Some((user_id, depth)) = stack.pop() {
            if user_id != root {
                let Some(user) = repos.user_repo.get_by_id(user_id).await? else {
                    continue;
                };
                tree.push(InviteeDto {
                    depth,
                    account: summary(&user),
                    blocked: Self::is_blocked(repos, &user).await?,
                });
            }
            let invitees = repos.user_repo.invited_users(user_id).await?;
            // Reversed onto the stack, so the oldest invitee comes out first.
            for invitee in invitees.into_iter().rev() {
                if seen.insert(invitee) {
                    stack.push((invitee, depth + 1));
                }
            }
        }
        Ok(tree)
    }

    /// Refuse `by` unless it may use the operator's tools.
    async fn check(&self, by: OperatorCaller) -> ServiceResult<()> {
        require_operator(self.repos.user_repo.as_ref(), by).await
    }

    /// The username of the account holding `principal_id` here.
    async fn username_of(
        repos: &RelayAbuseRepos,
        principal_id: &str,
    ) -> ServiceResult<Option<String>> {
        Ok(Self::account_of(repos, principal_id)
            .await?
            .map(|u| u.username))
    }

    /// Reports with their reporters' and subjects' usernames.
    async fn named_reports(
        repos: &RelayAbuseRepos,
        reports: Vec<AbuseReport>,
    ) -> ServiceResult<Vec<AbuseReportDto>> {
        let reporter_ids: Vec<UserId> = reports
            .iter()
            .filter_map(|r| r.reporter_user_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let reporters: HashMap<UserId, String> = if reporter_ids.is_empty() {
            HashMap::new()
        } else {
            repos
                .user_repo
                .get_by_ids(&reporter_ids)
                .await?
                .into_iter()
                .map(|u| (u.id, u.username))
                .collect()
        };
        let mut subjects: HashMap<String, Option<String>> = HashMap::new();
        for principal_id in reports
            .iter()
            .filter_map(|r| r.subject_principal_id.as_ref())
        {
            if !subjects.contains_key(principal_id) {
                let username = Self::username_of(repos, principal_id).await?;
                subjects.insert(principal_id.clone(), username);
            }
        }
        Ok(reports
            .into_iter()
            .map(|r| {
                let mut dto = AbuseReportDto::from(r);
                dto.reporter_username = dto
                    .reporter_user_id
                    .and_then(|id| reporters.get(&id).cloned());
                dto.subject_username = dto
                    .subject_principal_id
                    .as_ref()
                    .and_then(|p| subjects.get(p).cloned().flatten());
                dto
            })
            .collect())
    }

    /// A block with the username of the account holding its principal.
    async fn named_block(
        repos: &RelayAbuseRepos,
        block: PrincipalBlock,
    ) -> ServiceResult<PrincipalBlockDto> {
        Ok(PrincipalBlockDto {
            username: Self::username_of(repos, &block.principal_id).await?,
            principal_id: block.principal_id,
            reason: block.reason,
            created_at: block.created_at,
        })
    }
}

pub(crate) fn summary(user: &User) -> AccountSummaryDto {
    AccountSummaryDto {
        user_id: user.id,
        username: user.username.clone(),
        principal_id: user.principal_id.clone(),
        created_at: user.created_at,
    }
}

/// A reason trimmed, 1 to 1000 characters.
fn check_reason(reason: &str) -> ServiceResult<String> {
    let reason = reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_REASON_CHARS {
        return Err(ServiceError::Validation(format!(
            "a reason is 1 to {MAX_REASON_CHARS} characters"
        )));
    }
    Ok(reason.to_string())
}

fn check_principal(principal_id: &str) -> ServiceResult<()> {
    if is_principal_id(principal_id) {
        Ok(())
    } else {
        Err(ServiceError::Validation(format!(
            "{principal_id} is not a principal id"
        )))
    }
}

#[async_trait]
impl RelayAbuseService for DefaultRelayAbuseService {
    #[instrument(name = "relay_abuse.block_principal", skip_all)]
    async fn block_principal(
        &self,
        by: OperatorCaller,
        principal_id: String,
        reason: String,
        cascade: bool,
    ) -> ServiceResult<Vec<PrincipalBlockOutcome>> {
        self.check(by).await?;
        check_principal(&principal_id)?;
        let reason = check_reason(&reason)?;
        let (scope, repos) = self.begin().await?;
        let user = Self::account_of(&repos, &principal_id).await?;
        let mut outcomes = vec![
            Self::block_one(
                &repos,
                Some(principal_id.clone()),
                user.as_ref(),
                reason.clone(),
            )
            .await?,
        ];
        if cascade {
            let mut seen: HashSet<UserId> = user.iter().map(|u| u.id).collect();
            let mut next = 0;
            while next < outcomes.len() {
                let inviter = outcomes[next]
                    .account
                    .as_ref()
                    .map(|a| a.username.clone())
                    .unwrap_or_default();
                for invitee in outcomes[next].invitees.clone() {
                    if !seen.insert(invitee.user_id) {
                        continue;
                    }
                    let Some(user) = repos.user_repo.get_by_id(invitee.user_id).await? else {
                        continue;
                    };
                    let outcome = Self::block_one(
                        &repos,
                        user.principal_id.clone(),
                        Some(&user),
                        format!("{reason} (invited by {inviter})"),
                    )
                    .await?;
                    outcomes.push(outcome);
                }
                next += 1;
            }
        }
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
            for outcome in &mut outcomes {
                if let Some(account) = &outcome.account {
                    // A login that passed its block check before the commit may have added a
                    // session after the sweep.
                    outcome.ended_sessions += self
                        .repos
                        .session_repo
                        .revoke_all_for_user(account.user_id)
                        .await?;
                }
            }
        }
        for account in outcomes.iter().filter_map(|o| o.account.as_ref()) {
            self.session_revocations.revoke_user(account.user_id);
        }
        for (index, outcome) in outcomes.iter().enumerate() {
            info!(
                target: "audit",
                event = "relay.principal_blocked",
                scope = "server",
                actor_id = %by,
                principal_id = ?outcome.principal_id,
                user_id = ?outcome.account.as_ref().map(|a| a.user_id),
                newly_blocked = outcome.newly_blocked,
                ended_sessions = outcome.ended_sessions,
                ignored_requests = outcome.ignored_requests,
                revoked_invites = outcome.revoked_invites,
                email_blocked = outcome.blocked_email.is_some(),
                cascade = index > 0,
                "an operator blocked a principal or account on this server"
            );
        }
        Ok(outcomes)
    }

    #[instrument(name = "relay_abuse.unblock_principal", skip_all)]
    async fn unblock_principal(
        &self,
        by: OperatorCaller,
        principal_id: String,
    ) -> ServiceResult<PrincipalUnblockOutcome> {
        self.check(by).await?;
        check_principal(&principal_id)?;
        let (scope, repos) = self.begin().await?;
        let lifted = repos
            .relay_abuse_repo
            .unblock_principal(&principal_id)
            .await?;
        let email_unblocked = match Self::account_of(&repos, &principal_id)
            .await?
            .and_then(|u| u.email)
        {
            Some(email) => repos.user_repo.unblock_email(&email).await?,
            None => false,
        };
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        if lifted || email_unblocked {
            info!(
                target: "audit",
                event = "relay.principal_unblocked",
                scope = "server",
                actor_id = %by,
                principal_id = %principal_id,
                email_unblocked,
                "an operator unblocked a principal on this server"
            );
        }
        Ok(PrincipalUnblockOutcome {
            lifted,
            email_unblocked,
        })
    }

    async fn principal_record(
        &self,
        by: OperatorCaller,
        principal_id: String,
    ) -> ServiceResult<PrincipalRecordDto> {
        self.check(by).await?;
        check_principal(&principal_id)?;
        let repos = &self.repos;
        let block = match repos
            .relay_abuse_repo
            .list_principal_blocks()
            .await?
            .into_iter()
            .find(|b| b.principal_id == principal_id)
        {
            Some(block) => Some(Self::named_block(repos, block).await?),
            None => None,
        };
        let open_reports = repos
            .relay_abuse_repo
            .list_reports(true)
            .await?
            .into_iter()
            .filter(|r| r.subject_principal_id.as_deref() == Some(principal_id.as_str()))
            .collect();
        let open_reports = Self::named_reports(repos, open_reports).await?;
        let account = match Self::account_of(repos, &principal_id).await? {
            Some(user) => {
                let chain = repos.user_repo.invite_chain(user.id).await?;
                let inviter = match chain.and_then(|c| c.invited_by_user_id) {
                    Some(id) => repos.user_repo.get_by_id(id).await?.as_ref().map(summary),
                    None => None,
                };
                let email_blocked = match &user.email {
                    Some(email) => repos.user_repo.is_email_blocked(email).await?,
                    None => false,
                };
                let invites = repos
                    .user_repo
                    .invites_created_by(user.id)
                    .await?
                    .into_iter()
                    .map(|i| CreatedInviteDto {
                        id: i.id,
                        instance: i.kind == InviteKind::Instance,
                        created_at: i.created_at,
                        max_uses: i.max_uses,
                        use_count: i.use_count,
                        revoked_at: i.revoked_at,
                    })
                    .collect();
                Some(AccountRecordDto {
                    account: summary(&user),
                    email: user.email.clone(),
                    email_blocked,
                    invite_id: chain.map(|c| c.invite_id),
                    inviter,
                    invitees: Self::invite_tree(repos, user.id).await?,
                    invites,
                })
            }
            None => None,
        };
        Ok(PrincipalRecordDto {
            principal_id,
            account,
            block,
            open_reports,
        })
    }

    async fn principal_blocks(&self, by: OperatorCaller) -> ServiceResult<Vec<PrincipalBlockDto>> {
        self.check(by).await?;
        let mut named = Vec::new();
        for block in self.repos.relay_abuse_repo.list_principal_blocks().await? {
            named.push(Self::named_block(&self.repos, block).await?);
        }
        Ok(named)
    }

    #[instrument(name = "relay_abuse.file_report", skip_all)]
    async fn file_report(
        &self,
        by: OperatorCaller,
        report: FileAbuseReport,
    ) -> ServiceResult<AbuseReportDto> {
        self.check(by).await?;
        let reason = check_reason(&report.reason)?;
        if report.subject_principal_id.is_none() && report.subject_server_id.is_none() {
            return Err(ServiceError::Validation(
                "a report names a principal, a server or both".to_string(),
            ));
        }
        if let Some(principal_id) = &report.subject_principal_id {
            check_principal(principal_id)?;
        }
        if let Some(server_id) = &report.subject_server_id
            && !rim_s2s::is_server_id(server_id)
        {
            return Err(ServiceError::Validation(format!(
                "{server_id} is not a server id"
            )));
        }
        let stored = self
            .repos
            .relay_abuse_repo
            .insert_report(NewAbuseReport {
                reporter_user_id: None,
                subject_principal_id: report.subject_principal_id,
                subject_server_id: report.subject_server_id,
                request_id: None,
                reason,
            })
            .await?;
        info!(
            target: "audit",
            event = "relay.abuse_report_filed",
            actor_id = %by,
            report_id = %stored.id,
            principal_id = ?stored.subject_principal_id,
            peer_server_id = ?stored.subject_server_id,
            "an operator filed an abuse report"
        );
        Self::named_reports(&self.repos, vec![stored])
            .await?
            .pop()
            .ok_or_else(|| ServiceError::internal(anyhow::anyhow!("the filed report was lost")))
    }

    async fn reports(
        &self,
        by: OperatorCaller,
        open_only: bool,
    ) -> ServiceResult<Vec<AbuseReportDto>> {
        self.check(by).await?;
        let reports = self.repos.relay_abuse_repo.list_reports(open_only).await?;
        Self::named_reports(&self.repos, reports).await
    }

    #[instrument(name = "relay_abuse.resolve_report", skip_all)]
    async fn resolve_report(
        &self,
        by: OperatorCaller,
        id: AbuseReportId,
    ) -> ServiceResult<Option<AbuseReportDto>> {
        self.check(by).await?;
        let Some(resolved) = self.repos.relay_abuse_repo.resolve_report(id).await? else {
            return Ok(None);
        };
        info!(
            target: "audit",
            event = "relay.abuse_report_resolved",
            actor_id = %by,
            report_id = %id,
            "an operator resolved an abuse report"
        );
        Ok(Self::named_reports(&self.repos, vec![resolved])
            .await?
            .pop())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use rim_core::auth::model::SessionId;
    use rim_core::auth::repository::MockSessionRepository;
    use rim_core::key_identity::repository::MockKeyIdentityRepository;
    use rim_core::principal::model::{Principal, PrincipalId};
    use rim_core::relay::repository::MockRelayAbuseRepository;
    use rim_core::user::model::{CreatedInvite, InviteChain};
    use rim_core::user::repository::MockUserRepository;
    use rim_services::Actor;
    use rim_services::session_revocation::WatchedSession;

    use super::*;
    use crate::audit_capture::{capture_audit_lines, drain_buf};
    use crate::session_revocation::InProcessSessionRevocations;

    const CLI: OperatorCaller = OperatorCaller::CommandLine;

    const ADA: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
    const BOB: &str = "rimp1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";

    /// Calls the repositories received, in order.
    type Log = Arc<Mutex<Vec<String>>>;

    fn account(username: &str, principal: Option<&str>, email: Option<&str>) -> User {
        let now = Utc::now();
        User {
            id: UserId::new(),
            username: username.to_string(),
            password_hash: email.map(|_| "hash".to_string()),
            created_at: now,
            updated_at: now,
            last_login_at: None,
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: email.map(str::to_string),
            timezone: None,
            principal_id: principal.map(str::to_string),
            deletion_scheduled_for: None,
            handle: rim_core::user::model::UserHandle::Local,
        }
    }

    /// Ada (key and password) invited Bob (key only), who invited Cy (password only).
    struct World {
        ada: User,
        bob: User,
        cy: User,
        log: Log,
    }

    impl World {
        fn new() -> Self {
            Self {
                ada: account("ada", Some(ADA), Some("ada@example.com")),
                bob: account("bob", Some(BOB), None),
                cy: account("cy", None, Some("cy@example.com")),
                log: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        fn service(&self) -> DefaultRelayAbuseService {
            let users: HashMap<UserId, User> = [&self.ada, &self.bob, &self.cy]
                .into_iter()
                .map(|u| (u.id, u.clone()))
                .collect();
            let names: HashMap<UserId, String> = users
                .iter()
                .map(|(id, u)| (*id, u.username.clone()))
                .collect();
            let invitees: HashMap<UserId, Vec<UserId>> = HashMap::from([
                (self.ada.id, vec![self.bob.id]),
                (self.bob.id, vec![self.cy.id]),
            ]);
            let inviters: HashMap<UserId, UserId> =
                HashMap::from([(self.bob.id, self.ada.id), (self.cy.id, self.bob.id)]);
            let by_principal: HashMap<String, UserId> = users
                .values()
                .filter_map(|u| u.principal_id.clone().map(|p| (p, u.id)))
                .collect();
            let name = move |id: UserId| names[&id].clone();

            let mut keys = MockKeyIdentityRepository::new();
            keys.expect_find_user_by_principal_id().returning(move |p| {
                let found = by_principal.get(p).copied();
                Box::pin(async move { Ok(found) })
            });

            let mut sessions = MockSessionRepository::new();
            let log = Arc::clone(&self.log);
            let n = name.clone();
            sessions.expect_revoke_all_for_user().returning(move |id| {
                log.lock().unwrap().push(format!("end sessions {}", n(id)));
                Ok(2)
            });

            let mut abuse = MockRelayAbuseRepository::new();
            let log = Arc::clone(&self.log);
            abuse.expect_block_principal().returning(move |p, r| {
                log.lock().unwrap().push(format!("block {p}: {r}"));
                Box::pin(async { Ok(true) })
            });
            let log = Arc::clone(&self.log);
            abuse.expect_ignore_requests_from().returning(move |p, _| {
                log.lock().unwrap().push(format!("ignore requests {p}"));
                Box::pin(async { Ok(3) })
            });
            let log = Arc::clone(&self.log);
            abuse.expect_insert_report().returning(move |r| {
                log.lock().unwrap().push(format!(
                    "report {}: {}",
                    r.subject_principal_id.clone().unwrap_or_default(),
                    r.reason
                ));
                let stored = AbuseReport {
                    id: AbuseReportId::new(),
                    reporter_user_id: r.reporter_user_id,
                    subject_principal_id: r.subject_principal_id,
                    subject_server_id: r.subject_server_id,
                    request_id: r.request_id,
                    reason: r.reason,
                    created_at: Utc::now(),
                    resolved_at: None,
                };
                Box::pin(async move { Ok(stored) })
            });
            let log = Arc::clone(&self.log);
            abuse.expect_resolve_report().returning(move |_| {
                log.lock().unwrap().push("resolve report".to_string());
                Box::pin(async { Ok(None) })
            });
            let log = Arc::clone(&self.log);
            abuse.expect_unblock_principal().returning(move |p| {
                log.lock().unwrap().push(format!("unblock {p}"));
                Box::pin(async { Ok(true) })
            });
            abuse.expect_is_principal_blocked().returning(|p| {
                let blocked = p == BOB;
                Box::pin(async move { Ok(blocked) })
            });
            abuse.expect_list_principal_blocks().returning(|| {
                Box::pin(async {
                    Ok(vec![PrincipalBlock {
                        principal_id: ADA.to_string(),
                        reason: "spam".to_string(),
                        created_at: Utc::now(),
                    }])
                })
            });
            abuse.expect_list_reports().returning(|_| {
                let report = |principal: &str| AbuseReport {
                    id: AbuseReportId::new(),
                    reporter_user_id: None,
                    subject_principal_id: Some(principal.to_string()),
                    subject_server_id: None,
                    request_id: None,
                    reason: format!("mail about {principal}"),
                    created_at: Utc::now(),
                    resolved_at: None,
                };
                let reports = vec![report(ADA), report(BOB)];
                Box::pin(async move { Ok(reports) })
            });

            let mut user_repo = MockUserRepository::new();
            let all = users.clone();
            user_repo
                .expect_get_by_id()
                .returning(move |id| Ok(all.get(&id).cloned()));
            let all = users.clone();
            user_repo.expect_get_by_ids().returning(move |ids| {
                Ok(ids.iter().filter_map(|id| all.get(id).cloned()).collect())
            });
            let tree = invitees.clone();
            user_repo
                .expect_invited_users()
                .returning(move |id| Ok(tree.get(&id).cloned().unwrap_or_default()));
            let log = Arc::clone(&self.log);
            let n = name.clone();
            user_repo.expect_lock_user().returning(move |id| {
                log.lock().unwrap().push(format!("lock {}", n(id)));
                Ok(true)
            });
            let log = Arc::clone(&self.log);
            let n = name.clone();
            user_repo
                .expect_revoke_invites_created_by()
                .returning(move |id| {
                    log.lock()
                        .unwrap()
                        .push(format!("revoke invites {}", n(id)));
                    Ok(1)
                });
            let log = Arc::clone(&self.log);
            user_repo.expect_block_email().returning(move |email, r| {
                log.lock()
                    .unwrap()
                    .push(format!("block email {email}: {r}"));
                Ok(true)
            });
            let log = Arc::clone(&self.log);
            user_repo.expect_unblock_email().returning(move |email| {
                log.lock().unwrap().push(format!("unblock email {email}"));
                Ok(true)
            });
            user_repo
                .expect_is_email_blocked()
                .returning(|email| Ok(email == "cy@example.com"));
            // Ada is the instance operator.
            let operator = self.ada.id;
            user_repo
                .expect_is_instance_operator()
                .returning(move |id| Ok(id == operator));
            user_repo.expect_invite_chain().returning(move |id| {
                Ok(inviters.get(&id).map(|by| InviteChain {
                    invite_id: uuid::Uuid::nil(),
                    invited_by_user_id: Some(*by),
                }))
            });
            user_repo.expect_invites_created_by().returning(|_| {
                Ok(vec![CreatedInvite {
                    id: uuid::Uuid::nil(),
                    kind: InviteKind::Instance,
                    created_at: Utc::now(),
                    expires_at: None,
                    max_uses: 5,
                    use_count: 1,
                    revoked_at: None,
                }])
            });

            DefaultRelayAbuseService::new(RelayAbuseRepos {
                relay_abuse_repo: Arc::new(abuse),
                key_identity_repo: Arc::new(keys),
                session_repo: Arc::new(sessions),
                user_repo: Arc::new(user_repo),
            })
        }
    }

    #[tokio::test]
    async fn a_ban_blocks_ends_sessions_revokes_invites_blocks_the_email_and_files_a_report() {
        let w = World::new();
        let registry = Arc::new(InProcessSessionRevocations::default());
        let connection = |user_id| {
            registry.watch(WatchedSession {
                session_id: SessionId::new(),
                user_id,
                device_key_id: None,
            })
        };
        let (mut ada, mut bob) = (connection(w.ada.id), connection(w.bob.id));
        let outcomes = w
            .service()
            .with_session_revocations(Arc::clone(&registry) as Arc<dyn SessionRevocations>)
            .block_principal(CLI, ADA.to_string(), "  spam wave ".to_string(), false)
            .await
            .expect("blocked");
        assert!(ada.has_ended(), "the blocked account's connections close");
        assert!(!bob.has_ended());
        assert_eq!(
            w.calls(),
            vec![
                format!("block {ADA}: spam wave"),
                "lock ada".to_string(),
                "end sessions ada".to_string(),
                "revoke invites ada".to_string(),
                "block email ada@example.com: spam wave".to_string(),
                format!("ignore requests {ADA}"),
                format!("report {ADA}: blocked: spam wave"),
                "resolve report".to_string(),
            ]
        );
        assert_eq!(
            outcomes,
            vec![PrincipalBlockOutcome {
                principal_id: Some(ADA.to_string()),
                newly_blocked: true,
                account: Some(summary(&w.ada)),
                reason: "spam wave".to_string(),
                ended_sessions: 2,
                ignored_requests: 3,
                revoked_invites: 1,
                blocked_email: Some("ada@example.com".to_string()),
                invitees: vec![summary(&w.bob)],
            }]
        );
    }

    #[tokio::test]
    async fn a_cascade_bans_every_invitee_breadth_first_naming_its_inviter() {
        let w = World::new();
        let outcomes = w
            .service()
            .block_principal(CLI, ADA.to_string(), "spam ring".to_string(), true)
            .await
            .expect("blocked");
        let summary_of = |o: &PrincipalBlockOutcome| {
            (
                o.account.as_ref().map(|a| a.username.clone()),
                o.principal_id.clone(),
                o.reason.clone(),
                o.blocked_email.clone(),
            )
        };
        assert_eq!(
            outcomes.iter().map(summary_of).collect::<Vec<_>>(),
            vec![
                (
                    Some("ada".to_string()),
                    Some(ADA.to_string()),
                    "spam ring".to_string(),
                    Some("ada@example.com".to_string())
                ),
                (
                    Some("bob".to_string()),
                    Some(BOB.to_string()),
                    "spam ring (invited by ada)".to_string(),
                    None
                ),
                (
                    Some("cy".to_string()),
                    None,
                    "spam ring (invited by bob)".to_string(),
                    Some("cy@example.com".to_string())
                ),
            ]
        );
        // Cy has no key identity: no principal block, no request sweep, no report.
        let calls = w.calls();
        let cy_calls: Vec<&String> = calls
            .iter()
            .skip_while(|c| *c != "end sessions cy")
            .collect();
        assert_eq!(
            cy_calls,
            vec![
                "end sessions cy",
                "revoke invites cy",
                "block email cy@example.com: spam ring (invited by bob)",
            ]
        );
        assert!(outcomes[2].newly_blocked);
    }

    #[tokio::test]
    async fn a_principal_with_no_account_here_is_blocked_too() {
        let w = World::new();
        let stranger = "rimp1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c";
        let outcomes = w
            .service()
            .block_principal(CLI, stranger.to_string(), "again".to_string(), true)
            .await
            .expect("blocked");
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].account, None);
        assert_eq!(outcomes[0].ended_sessions, 0);
        assert_eq!(
            w.calls(),
            vec![
                format!("block {stranger}: again"),
                format!("ignore requests {stranger}"),
                format!("report {stranger}: blocked: again"),
                "resolve report".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn unblocking_lifts_the_principal_and_the_email_but_nothing_else() {
        let w = World::new();
        let outcome = w
            .service()
            .unblock_principal(CLI, ADA.to_string())
            .await
            .expect("unblocked");
        assert_eq!(
            outcome,
            PrincipalUnblockOutcome {
                lifted: true,
                email_unblocked: true,
            }
        );
        assert_eq!(
            w.calls(),
            vec![
                format!("unblock {ADA}"),
                "unblock email ada@example.com".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn a_record_names_the_inviter_the_invite_tree_and_the_open_reports() {
        let w = World::new();
        let record = w
            .service()
            .principal_record(CLI, BOB.to_string())
            .await
            .expect("record");
        let account = record.account.expect("bob's account");
        assert_eq!(account.inviter, Some(summary(&w.ada)));
        assert_eq!(account.invite_id, Some(uuid::Uuid::nil()));
        assert_eq!(
            account.invitees,
            vec![InviteeDto {
                depth: 1,
                account: summary(&w.cy),
                blocked: true,
            }]
        );
        assert_eq!(account.invites.len(), 1);
        assert!(account.invites[0].instance);
        assert_eq!(record.block, None);
        assert_eq!(
            record
                .open_reports
                .iter()
                .map(|r| r.reason.as_str())
                .collect::<Vec<_>>(),
            vec![format!("mail about {BOB}")]
        );

        let record = w
            .service()
            .principal_record(CLI, ADA.to_string())
            .await
            .expect("record");
        let account = record.account.expect("ada's account");
        assert_eq!(account.inviter, None);
        assert_eq!(
            account
                .invitees
                .iter()
                .map(|i| (i.depth, i.account.username.as_str(), i.blocked))
                .collect::<Vec<_>>(),
            vec![(1, "bob", true), (2, "cy", true)]
        );
        assert!(record.block.is_some());
    }

    #[tokio::test]
    async fn bad_principals_reasons_and_reports_are_refused() {
        let svc = World::new().service();
        for (principal, reason) in [("bob", "spam"), (ADA, "  "), (ADA, &"x".repeat(1001))] {
            assert!(matches!(
                svc.block_principal(CLI, principal.to_string(), reason.to_string(), false)
                    .await,
                Err(ServiceError::Validation(_))
            ));
        }
        for bad in ["bob", ""] {
            assert!(matches!(
                svc.principal_record(CLI, bad.to_string()).await,
                Err(ServiceError::Validation(_))
            ));
        }
        for report in [
            FileAbuseReport {
                subject_principal_id: None,
                subject_server_id: None,
                reason: "spam".to_string(),
            },
            FileAbuseReport {
                subject_principal_id: None,
                subject_server_id: Some("relay.example".to_string()),
                reason: "spam".to_string(),
            },
        ] {
            assert!(matches!(
                svc.file_report(CLI, report).await,
                Err(ServiceError::Validation(_))
            ));
        }
    }

    /// A signed-in caller acting as `user` directly.
    fn as_user(user: &User) -> OperatorCaller {
        OperatorCaller::User(Actor::new(user.id))
    }

    #[tokio::test]
    async fn a_user_who_is_not_an_operator_is_refused_every_tool_before_any_change() {
        let w = World::new();
        let svc = w.service();
        let bob = as_user(&w.bob);
        let report = FileAbuseReport {
            subject_principal_id: Some(ADA.to_string()),
            subject_server_id: None,
            reason: "spam".to_string(),
        };
        let refusals = [
            svc.block_principal(bob, ADA.to_string(), "spam".to_string(), true)
                .await
                .err(),
            svc.unblock_principal(bob, ADA.to_string()).await.err(),
            svc.principal_record(bob, ADA.to_string()).await.err(),
            svc.principal_blocks(bob).await.err(),
            svc.file_report(bob, report).await.err(),
            svc.reports(bob, false).await.err(),
            svc.resolve_report(bob, AbuseReportId::new()).await.err(),
        ];
        for refusal in refusals {
            assert!(
                matches!(refusal, Some(ServiceError::Forbidden(_))),
                "{refusal:?}"
            );
        }
        assert!(w.calls().is_empty(), "{:?}", w.calls());
    }

    #[tokio::test]
    async fn an_app_acting_for_an_operator_is_refused() {
        let w = World::new();
        let app = OperatorCaller::User(Actor {
            principal: Principal::app(PrincipalId::new()),
            user_id: w.ada.id,
        });
        assert!(matches!(
            w.service().reports(app, true).await,
            Err(ServiceError::Forbidden(_))
        ));
    }

    #[tokio::test]
    async fn an_operator_bans_as_the_command_line_does() {
        let w = World::new();
        let outcomes = w
            .service()
            .block_principal(as_user(&w.ada), BOB.to_string(), "spam".to_string(), false)
            .await
            .expect("blocked");
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].principal_id.as_deref(), Some(BOB));
        assert_eq!(w.calls()[0], format!("block {BOB}: spam"));
    }

    #[tokio::test]
    async fn reports_and_blocks_name_the_accounts_holding_their_principals() {
        let w = World::new();
        let svc = w.service();
        let reports = svc.reports(as_user(&w.ada), true).await.expect("reports");
        assert_eq!(
            reports
                .iter()
                .map(|r| (
                    r.subject_username.as_deref(),
                    r.reporter_username.as_deref()
                ))
                .collect::<Vec<_>>(),
            vec![(Some("ada"), None), (Some("bob"), None)]
        );
        let blocks = svc.principal_blocks(CLI).await.expect("blocks");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].username.as_deref(), Some("ada"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_audit_line_names_the_operator_or_the_command_line() {
        let (_guard, buf) = capture_audit_lines();
        let w = World::new();
        let svc = w.service();

        svc.unblock_principal(as_user(&w.ada), ADA.to_string())
            .await
            .expect("unblocked");
        svc.unblock_principal(CLI, ADA.to_string())
            .await
            .expect("unblocked");
        let lines = drain_buf(&buf);
        assert!(lines.contains(&format!("actor_id={}", w.ada.id)), "{lines}");
        assert!(lines.contains("actor_id=cli"), "{lines}");
    }
}
