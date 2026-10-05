use async_graphql::*;
use rim_services::Actor;
use rim_services::notification::command::{
    GetNotificationSettingsQuery, ListNotificationsQuery, UnreadNotificationCountQuery,
};
use rim_services::notification::service::NotificationService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::pagination::PageDir;
use crate::types::{NotificationCursorInput, NotificationPage, NotificationSettings};

/// `myNotifications` page size when `limit` is omitted.
const DEFAULT_LIMIT: i32 = 50;

#[derive(Default)]
pub(crate) struct NotificationQueries;

#[Object]
impl NotificationQueries {
    // ── Notification rules (Track 1.3) ────────────────────────────────

    /// The viewer's notification settings in the workspace (defaults until first saved).
    async fn my_notification_settings(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<NotificationSettings> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn NotificationService>>()?;
        let settings = svc
            .get_settings(GetNotificationSettingsQuery {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
            })
            .await
            .map_err(map_service_error)?;
        Ok(settings.into())
    }

    /// The viewer's notifications in the workspace, newest first (`createdAt DESC, id DESC`).
    /// `AFTER` pages go to older notifications. Defaults: `unreadOnly` false, `dir` `AFTER`,
    /// `limit` 50 (clamped to 1..=100).
    async fn my_notifications(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        unread_only: Option<bool>,
        dir: Option<PageDir>,
        cursor: Option<NotificationCursorInput>,
        limit: Option<i32>,
    ) -> Result<NotificationPage> {
        let unread_only = unread_only.unwrap_or(false);
        let dir = dir.unwrap_or(PageDir::After);
        let limit = limit.unwrap_or(DEFAULT_LIMIT);
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn NotificationService>>()?;
        let page = svc
            .list_notifications(ListNotificationsQuery {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                unread_only,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: u32::try_from(limit.max(1)).unwrap_or(1),
            })
            .await
            .map_err(map_service_error)?;
        Ok(NotificationPage::from_page(page))
    }

    /// Unread notifications of the viewer in the workspace (the bell badge).
    async fn my_unread_notification_count(&self, ctx: &Context<'_>, server_id: ID) -> Result<i64> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn NotificationService>>()?;
        let count = svc
            .unread_count(UnreadNotificationCountQuery {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
            })
            .await
            .map_err(map_service_error)?;
        Ok(i64::try_from(count).unwrap_or(i64::MAX))
    }
}
