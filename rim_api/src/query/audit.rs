use async_graphql::*;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct AuditLogQueries;

#[Object]
impl AuditLogQueries {
    /// Query the command audit log. Requires `audit.read` capability.
    async fn list_audit_log(
        &self,
        ctx: &Context<'_>,
        filter: Option<crate::types::AuditLogFilterInput>,
        pagination: Option<crate::types::AuditLogPaginationInput>,
    ) -> Result<crate::types::AuditLogPage> {
        use rim_services::audit::dto::ListAuditLogQuery;
        use rim_services::audit::service::AuditLogService;

        let caller_id = require_actor(ctx)?;
        let caller = rim_services::Actor::new(caller_id);
        let svc = ctx.data::<std::sync::Arc<dyn AuditLogService>>()?;

        let filter = filter.unwrap_or_default();
        let pagination = pagination.unwrap_or_default();

        // P6-H2: pass actor_user_id as None when the filter doesn't specify it.
        // The service layer now enforces the full access-control matrix on
        // (actor_user_id, server_id) — defaulting to caller_id here would
        // hide the unscoped-workspace cases and prevent workspace-audit reads
        // (actor_user_id=None, server_id=Some(W)) from reaching the service.
        let actor_user_id = filter
            .actor_user_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::user::model::UserId>)
            .transpose()?
            .map(|id| id.into());

        let server_id = filter
            .server_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::server::model::ServerId>)
            .transpose()?
            .map(|id| id.into());

        let cursor = pagination
            .after
            .as_ref()
            .and_then(|s| decode_audit_cursor(s));

        let limit = pagination.limit.unwrap_or(50).clamp(1, 200) as u32;

        let query = ListAuditLogQuery {
            actor_user_id,
            server_id,
            command_id: filter.command_id,
            typed_verb: filter.typed_verb.map(|v| {
                rim_services::command::verb::TypedVerb::from(v)
                    .as_str()
                    .to_string()
            }),
            since: filter.since.as_deref().and_then(parse_datetime),
            until: filter.until.as_deref().and_then(parse_datetime),
            cursor,
            limit,
        };

        let page = svc
            .list_entries(caller, query)
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(crate::types::AuditLogPage {
            items: page.items.into_iter().map(Into::into).collect(),
            next_cursor: page.next_cursor.map(|(dt, id)| encode_audit_cursor(dt, id)),
            page_info: crate::types::AuditLogPageInfo {
                has_next_page: page.page_info.has_next_page,
                has_prev_page: page.page_info.has_prev_page,
            },
        })
    }
}

fn encode_audit_cursor(dt: chrono::DateTime<chrono::Utc>, id: uuid::Uuid) -> String {
    use base64::Engine;
    let raw = format!("{}:{}", dt.timestamp_micros(), id);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes())
}

fn decode_audit_cursor(s: &str) -> Option<(chrono::DateTime<chrono::Utc>, uuid::Uuid)> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .ok()?;
    let s = std::str::from_utf8(&decoded).ok()?;
    let (ts_part, id_part) = s.split_once(':')?;
    let ts_micros: i64 = ts_part.parse().ok()?;
    let dt = chrono::DateTime::from_timestamp_micros(ts_micros)?;
    let id = uuid::Uuid::parse_str(id_part).ok()?;
    Some((dt, id))
}

fn parse_datetime(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}
