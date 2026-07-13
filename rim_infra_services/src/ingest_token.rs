use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;

use rim_auth::session_token::{generate_ingest_token, hash_token};
use rim_core::conversation::model::ConversationType;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::ingest_token::model::NewIngestToken;
use rim_core::ingest_token::repository::IngestTokenRepository;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_services::ingest_token::command::{
    CreateIngestTokenCmd, ListIngestTokensQuery, RevokeIngestTokenCmd,
};
use rim_services::ingest_token::dto::{CreateIngestTokenResultDto, IngestTokenDto};
use rim_services::ingest_token::service::IngestTokenService;
use rim_services::{ServiceError, ServiceResult};

use crate::util::{ensure_conversation_access, ensure_conversation_in_server, ensure_server_owner};

#[derive(Clone)]
pub struct IngestTokenRepos {
    pub ingest_token_repo: Arc<dyn IngestTokenRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

pub struct DefaultIngestTokenService {
    repos: IngestTokenRepos,
    /// Instance-wide ingest-token TTL (`RIM_INGEST_TOKEN_TTL_SECONDS`). `None` means tokens
    /// never expire (revocation only) — see `rim_server::config::RIMConfig::ingest_token_ttl_seconds`.
    /// Computed against `token.created_at` at authenticate time; not a persisted per-token
    /// expiry (`workspace_ingest_token` has no `expires_at` column).
    ttl_seconds: Option<u64>,
}

impl DefaultIngestTokenService {
    pub fn new(repos: IngestTokenRepos, ttl_seconds: Option<u64>) -> Self {
        Self { repos, ttl_seconds }
    }
}

#[async_trait]
impl IngestTokenService for DefaultIngestTokenService {
    async fn create_token(
        &self,
        cmd: CreateIngestTokenCmd,
    ) -> ServiceResult<CreateIngestTokenResultDto> {
        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;
        if conv.conversation_type != ConversationType::Channel {
            return Err(ServiceError::Validation(
                "ingest tokens may only target channels".to_string(),
            ));
        }
        // Channels never require explicit membership (see `ensure_conversation_access`), but
        // calling it keeps this call site consistent with every other conversation-scoped
        // write path in case that invariant ever changes.
        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            cmd.actor.user_id,
        )
        .await?;

        if cmd.label.trim().is_empty() || cmd.label.chars().count() > 128 {
            return Err(ServiceError::Validation(
                "label must be between 1 and 128 characters".to_string(),
            ));
        }

        let raw_token = generate_ingest_token();
        let token_hash = hash_token(&raw_token);

        let token = self
            .repos
            .ingest_token_repo
            .create(NewIngestToken {
                server_id: cmd.server_id,
                conversation_id: cmd.conversation_id,
                created_by: cmd.actor.user_id,
                label: cmd.label,
                token_hash,
                severity_default: cmd.severity_default,
            })
            .await?;

        Ok(CreateIngestTokenResultDto {
            token: IngestTokenDto::from(token),
            raw_token,
        })
    }

    async fn list_tokens(
        &self,
        query: ListIngestTokensQuery,
    ) -> ServiceResult<Vec<IngestTokenDto>> {
        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let tokens = self
            .repos
            .ingest_token_repo
            .list_for_server(query.server_id)
            .await?;
        Ok(tokens.into_iter().map(IngestTokenDto::from).collect())
    }

    async fn revoke_token(&self, cmd: RevokeIngestTokenCmd) -> ServiceResult<()> {
        let token = self
            .repos
            .ingest_token_repo
            .find_by_id(cmd.token_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            token.server_id,
            cmd.actor.user_id,
        )
        .await?;

        self.repos.ingest_token_repo.revoke(cmd.token_id).await?;
        Ok(())
    }

    async fn authenticate_token(&self, raw_token: &str) -> ServiceResult<IngestTokenDto> {
        let token_hash = hash_token(raw_token);
        let token = self
            .repos
            .ingest_token_repo
            .find_by_token_hash(&token_hash)
            .await?
            .ok_or(ServiceError::Unauthenticated)?;

        if token.revoked_at.is_some() {
            return Err(ServiceError::Unauthenticated);
        }

        if let Some(ttl) = self.ttl_seconds {
            let age_seconds = (Utc::now() - token.created_at).num_seconds().max(0) as u64;
            if age_seconds > ttl {
                return Err(ServiceError::Unauthenticated);
            }
        }

        // Best-effort: a failure to record last-used must never fail authentication itself.
        if let Err(e) = self
            .repos
            .ingest_token_repo
            .touch_last_used(token.id, Utc::now())
            .await
        {
            tracing::warn!(
                error = %e,
                token_id = %token.id,
                "failed to update ingest token last_used_at"
            );
        }

        Ok(IngestTokenDto::from(token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::conversation::model::{Conversation, ConversationId};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::incident::model::IncidentSeverity;
    use rim_core::ingest_token::model::{IngestToken, IngestTokenId};
    use rim_core::ingest_token::repository::MockIngestTokenRepository;
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_services::Actor;

    fn owner_member(server_id: ServerId, user_id: UserId) -> ServerMember {
        ServerMember {
            id: ServerMemberId::new(),
            server_id,
            user_id,
            member_role: ServerMemberRole::Owner,
            joined_at: chrono::Utc::now(),
        }
    }

    fn channel(server_id: ServerId, id: ConversationId) -> Conversation {
        Conversation {
            id,
            server_id,
            conversation_type: ConversationType::Channel,
            title: Some("alerts".to_string()),
            topic: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
        }
    }

    fn repos(
        server_member_repo: MockServerMemberRepository,
        conversation_repo: MockConversationRepository,
        ingest_token_repo: MockIngestTokenRepository,
    ) -> IngestTokenRepos {
        IngestTokenRepos {
            ingest_token_repo: Arc::new(ingest_token_repo),
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            server_member_repo: Arc::new(server_member_repo),
        }
    }

    #[tokio::test]
    async fn create_token_requires_server_owner() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(|_, _| Ok(None));

        let svc = DefaultIngestTokenService::new(
            repos(
                server_member_repo,
                MockConversationRepository::new(),
                MockIngestTokenRepository::new(),
            ),
            None,
        );

        let err = svc
            .create_token(CreateIngestTokenCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: ConversationId::new(),
                label: "prom".to_string(),
                severity_default: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn create_token_rejects_non_channel_target() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo.expect_get_by_id().returning(move |_| {
            Box::pin(async move {
                Ok(Some(Conversation {
                    id: conv_id,
                    server_id,
                    conversation_type: ConversationType::Group,
                    title: None,
                    topic: None,
                    created_at: chrono::Utc::now(),
                    last_activity_at: chrono::Utc::now(),
                }))
            })
        });

        let svc = DefaultIngestTokenService::new(
            repos(
                server_member_repo,
                conversation_repo,
                MockIngestTokenRepository::new(),
            ),
            None,
        );

        let err = svc
            .create_token(CreateIngestTokenCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                label: "prom".to_string(),
                severity_default: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_token_success_returns_raw_token_once() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv_id = ConversationId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_| Box::pin(async move { Ok(Some(channel(server_id, conv_id))) }));

        let mut ingest_token_repo = MockIngestTokenRepository::new();
        ingest_token_repo.expect_create().returning(move |new| {
            Box::pin(async move {
                Ok(IngestToken {
                    id: IngestTokenId::new(),
                    server_id: new.server_id,
                    conversation_id: new.conversation_id,
                    created_by: new.created_by,
                    label: new.label,
                    token_hash: new.token_hash,
                    severity_default: new.severity_default,
                    created_at: chrono::Utc::now(),
                    revoked_at: None,
                    last_used_at: None,
                })
            })
        });

        let svc = DefaultIngestTokenService::new(
            repos(server_member_repo, conversation_repo, ingest_token_repo),
            None,
        );

        let result = svc
            .create_token(CreateIngestTokenCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                label: "prom".to_string(),
                severity_default: Some(IncidentSeverity::Sev2),
            })
            .await
            .unwrap();

        assert!(result.raw_token.starts_with("rim_ingest_"));
        assert_eq!(result.token.severity_default, Some(IncidentSeverity::Sev2));
    }

    #[tokio::test]
    async fn authenticate_token_rejects_unknown() {
        let mut ingest_token_repo = MockIngestTokenRepository::new();
        ingest_token_repo
            .expect_find_by_token_hash()
            .returning(|_| Box::pin(async { Ok(None) }));

        let svc = DefaultIngestTokenService::new(
            repos(
                MockServerMemberRepository::new(),
                MockConversationRepository::new(),
                ingest_token_repo,
            ),
            None,
        );

        let err = svc
            .authenticate_token("rim_ingest_bogus")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
    }

    #[tokio::test]
    async fn authenticate_token_rejects_revoked() {
        let token_id = IngestTokenId::new();
        let mut ingest_token_repo = MockIngestTokenRepository::new();
        ingest_token_repo
            .expect_find_by_token_hash()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(Some(IngestToken {
                        id: token_id,
                        server_id: ServerId::new(),
                        conversation_id: ConversationId::new(),
                        created_by: UserId::new(),
                        label: "x".to_string(),
                        token_hash: "hash".to_string(),
                        severity_default: None,
                        created_at: chrono::Utc::now(),
                        revoked_at: Some(chrono::Utc::now()),
                        last_used_at: None,
                    }))
                })
            });

        let svc = DefaultIngestTokenService::new(
            repos(
                MockServerMemberRepository::new(),
                MockConversationRepository::new(),
                ingest_token_repo,
            ),
            None,
        );

        let err = svc.authenticate_token("raw").await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
    }

    #[tokio::test]
    async fn authenticate_token_rejects_expired_when_ttl_configured() {
        let token_id = IngestTokenId::new();
        let mut ingest_token_repo = MockIngestTokenRepository::new();
        ingest_token_repo
            .expect_find_by_token_hash()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(Some(IngestToken {
                        id: token_id,
                        server_id: ServerId::new(),
                        conversation_id: ConversationId::new(),
                        created_by: UserId::new(),
                        label: "x".to_string(),
                        token_hash: "hash".to_string(),
                        severity_default: None,
                        created_at: chrono::Utc::now() - chrono::Duration::seconds(120),
                        revoked_at: None,
                        last_used_at: None,
                    }))
                })
            });

        let svc = DefaultIngestTokenService::new(
            repos(
                MockServerMemberRepository::new(),
                MockConversationRepository::new(),
                ingest_token_repo,
            ),
            Some(60), // TTL shorter than the token's 120s age
        );

        let err = svc.authenticate_token("raw").await.unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
    }

    #[tokio::test]
    async fn authenticate_token_allows_unexpired_when_ttl_configured() {
        let token_id = IngestTokenId::new();
        let mut ingest_token_repo = MockIngestTokenRepository::new();
        ingest_token_repo
            .expect_find_by_token_hash()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(Some(IngestToken {
                        id: token_id,
                        server_id: ServerId::new(),
                        conversation_id: ConversationId::new(),
                        created_by: UserId::new(),
                        label: "x".to_string(),
                        token_hash: "hash".to_string(),
                        severity_default: None,
                        created_at: chrono::Utc::now() - chrono::Duration::seconds(10),
                        revoked_at: None,
                        last_used_at: None,
                    }))
                })
            });
        ingest_token_repo
            .expect_touch_last_used()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let svc = DefaultIngestTokenService::new(
            repos(
                MockServerMemberRepository::new(),
                MockConversationRepository::new(),
                ingest_token_repo,
            ),
            Some(3600), // TTL far longer than the token's 10s age
        );

        let dto = svc.authenticate_token("raw").await.unwrap();
        assert_eq!(dto.id, token_id);
    }

    #[tokio::test]
    async fn authenticate_token_success_touches_last_used() {
        let token_id = IngestTokenId::new();
        let mut ingest_token_repo = MockIngestTokenRepository::new();
        ingest_token_repo
            .expect_find_by_token_hash()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(Some(IngestToken {
                        id: token_id,
                        server_id: ServerId::new(),
                        conversation_id: ConversationId::new(),
                        created_by: UserId::new(),
                        label: "x".to_string(),
                        token_hash: "hash".to_string(),
                        severity_default: None,
                        created_at: chrono::Utc::now(),
                        revoked_at: None,
                        last_used_at: None,
                    }))
                })
            });
        ingest_token_repo
            .expect_touch_last_used()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let svc = DefaultIngestTokenService::new(
            repos(
                MockServerMemberRepository::new(),
                MockConversationRepository::new(),
                ingest_token_repo,
            ),
            None,
        );

        let dto = svc.authenticate_token("raw").await.unwrap();
        assert_eq!(dto.id, token_id);
    }

    #[tokio::test]
    async fn revoke_token_requires_owner_of_tokens_server() {
        let token_id = IngestTokenId::new();
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let mut ingest_token_repo = MockIngestTokenRepository::new();
        ingest_token_repo.expect_find_by_id().returning(move |_| {
            Box::pin(async move {
                Ok(Some(IngestToken {
                    id: token_id,
                    server_id,
                    conversation_id: ConversationId::new(),
                    created_by: UserId::new(),
                    label: "x".to_string(),
                    token_hash: "hash".to_string(),
                    severity_default: None,
                    created_at: chrono::Utc::now(),
                    revoked_at: None,
                    last_used_at: None,
                }))
            })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(|_, _| Ok(None));

        let svc = DefaultIngestTokenService::new(
            repos(
                server_member_repo,
                MockConversationRepository::new(),
                ingest_token_repo,
            ),
            None,
        );

        let err = svc
            .revoke_token(RevokeIngestTokenCmd {
                actor: Actor::new(actor_id),
                token_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }
}
