#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{TimeZone, Utc};
    use mockall::predicate::eq;

    use rim_core::conversation::model::ConversationId;
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::incident::model::{Incident, IncidentId, IncidentSeverity, IncidentStatus};
    use rim_core::incident::repository::{
        IncidentListCursor, MockIncidentMessageTagRepository, MockIncidentPhaseRepository,
        MockIncidentRepository,
    };
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_services::incident::command::ListIncidentsQuery;
    use rim_services::incident::dto::IncidentListPageCursor;
    use rim_services::incident::service::IncidentService;
    use rim_services::{Actor, ServiceError};

    use crate::IncidentRepos;
    use crate::incident::DefaultIncidentService;

    fn make_incident(server_id: ServerId, status: IncidentStatus, n: i64) -> Incident {
        let ts = Utc.timestamp_opt(1_700_000_000 + n, 0).unwrap();
        Incident {
            id: IncidentId::new(),
            server_id,
            conversation_id: ConversationId::new(),
            started_by: UserId::new(),
            status,
            severity: Some(IncidentSeverity::Sev2),
            title: format!("incident #{n}"),
            original_topic: None,
            started_at: ts,
            mitigated_at: None,
            resolved_at: None,
            postmortem_draft: None,
            created_at: ts,
            updated_at: ts,
        }
    }

    fn build_service(
        incident_repo: MockIncidentRepository,
        server_member_repo: MockServerMemberRepository,
    ) -> DefaultIncidentService {
        DefaultIncidentService::new(
            IncidentRepos {
                incident_repo: Arc::new(incident_repo),
                incident_phase_repo: Arc::new(MockIncidentPhaseRepository::new()),
                incident_message_tag_repo: Arc::new(MockIncidentMessageTagRepository::new()),
                conversation_repo: Arc::new(MockConversationRepository::new()),
                server_member_repo: Arc::new(server_member_repo),
            },
            None,
        )
    }

    #[tokio::test]
    async fn list_incidents_denies_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .with(eq(server_id), eq(actor_id))
            .returning(|_, _| Ok(false));

        let mut incident_repo = MockIncidentRepository::new();
        incident_repo.expect_list_incidents().never();

        let svc = build_service(incident_repo, server_member_repo);

        let err = svc
            .list_incidents(ListIncidentsQuery {
                actor: Actor::new(actor_id),
                server_id,
                status_filter: None,
                severity_filter: None,
                dir: PageDir::After,
                cursor: None,
                limit: 20,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn list_incidents_passes_filters_and_maps_page() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let i1 = make_incident(server_id, IncidentStatus::Active, 2);
        let i2 = make_incident(server_id, IncidentStatus::Active, 1);
        let next_cursor = IncidentListCursor {
            created_at: i2.created_at,
            incident_id: i2.id,
        };

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut incident_repo = MockIncidentRepository::new();
        incident_repo
            .expect_list_incidents()
            .withf(move |sid, statuses, severities, limit, dir, cursor| {
                *sid == server_id
                    && *statuses == Some(&[IncidentStatus::Active][..])
                    && *severities == Some(&[IncidentSeverity::Sev1, IncidentSeverity::Sev2][..])
                    && *limit == 20
                    && matches!(dir, PageDir::After)
                    && cursor.is_none()
            })
            .returning(move |_, _, _, _, _, _| {
                let i1 = i1.clone();
                let i2 = i2.clone();
                Box::pin(async move { Ok(Page::new(vec![i1, i2], Some(next_cursor), None)) })
            });

        let svc = build_service(incident_repo, server_member_repo);

        let page = svc
            .list_incidents(ListIncidentsQuery {
                actor: Actor::new(actor_id),
                server_id,
                status_filter: Some(vec![IncidentStatus::Active]),
                severity_filter: Some(vec![IncidentSeverity::Sev1, IncidentSeverity::Sev2]),
                dir: PageDir::After,
                cursor: None,
                limit: 20,
            })
            .await
            .unwrap();

        assert_eq!(page.items.len(), 2);
        let next: IncidentListPageCursor = page.next.unwrap();
        assert_eq!(next.incident_id, next_cursor.incident_id);
        assert_eq!(next.created_at, next_cursor.created_at);
        assert!(page.prev.is_none());
    }

    #[tokio::test]
    async fn list_incidents_forwards_cursor_and_dir() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let cursor = IncidentListPageCursor {
            created_at: Utc.timestamp_opt(1_700_000_500, 0).unwrap(),
            incident_id: IncidentId::new(),
        };

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut incident_repo = MockIncidentRepository::new();
        let cursor_for_check = cursor;
        incident_repo
            .expect_list_incidents()
            .withf(move |_, statuses, severities, _, dir, c| {
                statuses.is_none()
                    && severities.is_none()
                    && matches!(dir, PageDir::Before)
                    && c.map(|c| {
                        c.incident_id == cursor_for_check.incident_id
                            && c.created_at == cursor_for_check.created_at
                    })
                    .unwrap_or(false)
            })
            .returning(|_, _, _, _, _, _| {
                Box::pin(async move { Ok(Page::new(vec![], None, None)) })
            });

        let svc = build_service(incident_repo, server_member_repo);

        let page = svc
            .list_incidents(ListIncidentsQuery {
                actor: Actor::new(actor_id),
                server_id,
                status_filter: None,
                severity_filter: None,
                dir: PageDir::Before,
                cursor: Some(cursor),
                limit: 20,
            })
            .await
            .unwrap();
        assert!(page.items.is_empty());
    }
}
