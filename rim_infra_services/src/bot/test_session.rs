//! Webhook test sessions: who may start and read one, and what a delivery to one is held as. The
//! sessions themselves are in the [`WebhookTestSessionStore`]; nothing here reaches the database
//! but the audit entries of a session starting and ending, which hold ids alone.

use chrono::Utc;

use rim_auth::session_token::{generate_webhook_token, hash_token, is_webhook_token_shape};
use rim_core::bot::model::{IncomingWebhookId, WebhookTestSessionId};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_services::bot::dto::{StartedTestSessionDto, TestDeliveriesDto};
use rim_services::bot::test_session::{
    ArrivedDelivery, MAX_TEST_BODY_BYTES, NewTestSession, TestBody, TestSessionInfo,
};
use rim_services::command::verb::TypedVerb;
use rim_services::{Actor, ServiceError, ServiceResult};

use super::DefaultBotService;
use super::webhook_format::is_refused_header;

pub(super) const AUDIT_TEST_SESSION_START: &str = "bot.webhook.test_session.start";
pub(super) const AUDIT_TEST_SESSION_END: &str = "bot.webhook.test_session.end";

/// How long a client waits before starting another session once the limit is reached.
const FULL_RETRY_SECONDS: u64 = 60;

/// The refusal of a session someone else started, or one of another workspace, on creation.
const NOT_YOURS: &str = "That test session is not yours, or not for this workspace.";

/// What `body` is held as: its text exactly, or why its bytes are not held.
fn held_body(body: Option<&[u8]>) -> TestBody {
    match body {
        None => TestBody::TooLarge,
        Some(bytes) if bytes.len() > MAX_TEST_BODY_BYTES => TestBody::TooLarge,
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => TestBody::Text(text.to_string()),
            Err(_) => TestBody::NotText,
        },
    }
}

/// A delivery as a session holds it, with the headers a format refuses dropped.
fn arrived(
    content_type: Option<&str>,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> ArrivedDelivery {
    ArrivedDelivery {
        content_type: content_type.map(str::to_string),
        headers: headers
            .iter()
            .filter(|(name, _)| !is_refused_header(name))
            .cloned()
            .collect(),
        body: held_body(body),
    }
}

impl DefaultBotService {
    /// Whether `actor` may start a session like `session`: Owner or Moderator of its workspace,
    /// and for a bound one, its webhook not revoked and its private channel one they are in.
    async fn ensure_session_standing(
        &self,
        actor: UserId,
        server_id: ServerId,
        webhook_id: Option<IncomingWebhookId>,
    ) -> ServiceResult<()> {
        self.ensure_moderator(server_id, actor).await?;
        let Some(webhook_id) = webhook_id else {
            return Ok(());
        };
        let webhook = self
            .repos
            .bot_repo
            .find_webhook(webhook_id)
            .await?
            .filter(|webhook| webhook.server_id == server_id)
            .ok_or(ServiceError::NotFound)?;
        if webhook.revoked_at.is_some() {
            return Err(ServiceError::Validation(
                "A revoked webhook gets no deliveries.".to_string(),
            ));
        }
        let conversation = self
            .repos
            .conversation_repo
            .get_by_id(webhook.conversation_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if conversation.is_private
            && !self
                .repos
                .conversation_member_repo
                .is_member(webhook.conversation_id, actor)
                .await?
        {
            return Err(ServiceError::PermissionDenied);
        }
        Ok(())
    }

    /// The session of `actor` named `id`, or `NotFound`.
    fn owned_session(
        &self,
        actor: UserId,
        id: WebhookTestSessionId,
    ) -> ServiceResult<TestSessionInfo> {
        self.test_sessions
            .get(id, Utc::now())
            .filter(|session| session.owner == actor)
            .ok_or(ServiceError::NotFound)
    }

    async fn audit_session(
        &self,
        actor: UserId,
        command_id: &'static str,
        session: &TestSessionInfo,
    ) {
        let mut args = serde_json::json!({ "session_id": session.id.to_string() });
        if let Some(webhook_id) = session.webhook_id {
            args["webhook_id"] = serde_json::json!(webhook_id.to_string());
        }
        let verb = if command_id == AUDIT_TEST_SESSION_START {
            TypedVerb::Write
        } else {
            TypedVerb::Delete
        };
        self.audit(actor, session.server_id, command_id, args, verb)
            .await;
    }

    pub(super) async fn start_webhook_test_session(
        &self,
        actor: Actor,
        server_id: ServerId,
        webhook_id: Option<IncomingWebhookId>,
    ) -> ServiceResult<StartedTestSessionDto> {
        self.require_feature()?;
        self.ensure_session_standing(actor.user_id, server_id, webhook_id)
            .await?;
        let token = webhook_id.is_none().then(generate_webhook_token);
        let session = self
            .test_sessions
            .open(
                NewTestSession {
                    server_id,
                    owner: actor.user_id,
                    webhook_id,
                    token_hash: token.as_deref().map(hash_token),
                },
                Utc::now(),
            )
            .map_err(|_| ServiceError::RateLimited {
                retry_after_seconds: FULL_RETRY_SECONDS,
            })?;
        self.audit_session(actor.user_id, AUDIT_TEST_SESSION_START, &session)
            .await;
        Ok(StartedTestSessionDto {
            id: session.id,
            token,
            expires_at: session.expires_at,
        })
    }

    pub(super) async fn end_webhook_test_session(
        &self,
        actor: Actor,
        id: WebhookTestSessionId,
    ) -> ServiceResult<()> {
        self.require_feature()?;
        self.owned_session(actor.user_id, id)?;
        if let Some(session) = self.test_sessions.end(id) {
            self.audit_session(actor.user_id, AUDIT_TEST_SESSION_END, &session)
                .await;
        }
        Ok(())
    }

    pub(super) async fn webhook_test_deliveries(
        &self,
        actor: Actor,
        id: WebhookTestSessionId,
        after: u64,
    ) -> ServiceResult<TestDeliveriesDto> {
        self.require_feature()?;
        let session = self.owned_session(actor.user_id, id)?;
        // The same answer as a session that is gone, once the starter lost the standing.
        self.ensure_session_standing(actor.user_id, session.server_id, session.webhook_id)
            .await
            .map_err(|e| match e {
                ServiceError::Internal(_) => e,
                _ => ServiceError::NotFound,
            })?;
        let (deliveries, expires_at) = self
            .test_sessions
            .read(id, after, Utc::now())
            .ok_or(ServiceError::NotFound)?;
        Ok(TestDeliveriesDto {
            deliveries,
            expires_at,
        })
    }

    pub(super) fn resolve_webhook_test_session(&self, raw: &str) -> Option<WebhookTestSessionId> {
        if !is_webhook_token_shape(raw) || self.require_feature().is_err() {
            return None;
        }
        self.test_sessions
            .unbound_by_hash(&hash_token(raw), Utc::now())
            .map(|session| session.id)
    }

    pub(super) fn record_webhook_test_delivery(
        &self,
        id: WebhookTestSessionId,
        content_type: Option<&str>,
        headers: &[(String, String)],
        body: Option<&[u8]>,
    ) {
        self.test_sessions
            .record(id, arrived(content_type, headers, body), Utc::now());
    }

    pub(super) fn copy_to_webhook_test_sessions(
        &self,
        webhook_id: IncomingWebhookId,
        content_type: Option<&str>,
        headers: &[(String, String)],
        body: Option<&[u8]>,
    ) {
        let now = Utc::now();
        for id in self.test_sessions.bound_to(webhook_id, now) {
            self.test_sessions
                .record(id, arrived(content_type, headers, body), now);
        }
    }

    /// The token hash a new webhook keeps from `actor`'s open unbound session `id` of
    /// `server_id`, which then ends. `None` when there is no session to keep, or it is gone.
    ///
    /// # Errors
    /// `Validation` for a session that is open but someone else's, another workspace's or bound.
    pub(super) fn session_to_keep(
        &self,
        actor: UserId,
        server_id: ServerId,
        id: Option<WebhookTestSessionId>,
    ) -> ServiceResult<Option<WebhookTestSessionId>> {
        let Some(id) = id else {
            return Ok(None);
        };
        match self.test_sessions.get(id, Utc::now()) {
            None => Ok(None),
            Some(session)
                if session.owner == actor
                    && session.server_id == server_id
                    && session.webhook_id.is_none() =>
            {
                Ok(Some(id))
            }
            Some(_) => Err(ServiceError::Validation(NOT_YOURS.to_string())),
        }
    }

    /// End session `id` for a new webhook to keep its token: its hash and the session, or `None`
    /// when it ended meanwhile.
    pub(super) fn take_session(
        &self,
        id: Option<WebhookTestSessionId>,
    ) -> Option<(TestSessionInfo, String)> {
        self.test_sessions.take_unbound(id?, Utc::now())
    }

    /// End the sessions bound to a webhook that was revoked.
    pub(super) async fn end_sessions_of(&self, actor: UserId, webhook_id: IncomingWebhookId) {
        for session in self.test_sessions.end_bound_to(webhook_id) {
            self.audit_session(actor, AUDIT_TEST_SESSION_END, &session)
                .await;
        }
    }

    pub(super) fn sweep_webhook_test_sessions(&self) {
        self.test_sessions.sweep(Utc::now());
    }

    /// The audit entry of a session that ended because a new webhook kept its token.
    pub(super) async fn audit_kept_session(&self, actor: UserId, session: &TestSessionInfo) {
        self.audit_session(actor, AUDIT_TEST_SESSION_END, session)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::arrived;

    #[test]
    fn a_delivery_keeps_no_header_that_carries_the_senders_address() {
        let headers: Vec<(String, String)> = [
            ("x-forwarded-for", "203.0.113.7"),
            ("X-Real-IP", "203.0.113.7"),
            ("x-github-event", "ping"),
        ]
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .to_vec();
        let held = arrived(None, &headers, Some(b"{}"));
        assert_eq!(
            held.headers,
            [("x-github-event".to_string(), "ping".to_string())]
        );
    }
}
