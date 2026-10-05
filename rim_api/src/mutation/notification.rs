use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_core::notification::model::DndPause;
use rim_services::Actor;
use rim_services::notification::command::{
    MarkNotificationsReadCmd, MarkNotificationsReadTarget, SetConversationNotificationLevelCmd,
    SetDndPauseCmd, UpdateNotificationSettingsCmd,
};
use rim_services::notification::service::NotificationService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{
    ConversationNotificationLevel, NotificationLevel, NotificationSettings,
    UpdateNotificationSettingsInput,
};

#[derive(Default)]
pub(crate) struct NotificationMutations;

fn notifications(ctx: &Context<'_>) -> Result<std::sync::Arc<dyn NotificationService>> {
    Ok(std::sync::Arc::clone(
        ctx.data::<std::sync::Arc<dyn NotificationService>>()?,
    ))
}

fn validation(message: &str) -> Error {
    Error::new(message).extend_with(|_, e| e.set("code", "VALIDATION"))
}

#[Object]
impl NotificationMutations {
    // ── Notification rules (Track 1.3) ────────────────────────────────

    /// Replace the viewer's notification settings in the workspace (default level, keywords,
    /// daily do-not-disturb window, digest). Keywords: at most 20, each 2 to 40 characters.
    async fn update_notification_settings(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        input: UpdateNotificationSettingsInput,
    ) -> Result<NotificationSettings> {
        let actor = require_actor(ctx)?;
        let settings = notifications(ctx)?
            .update_settings(UpdateNotificationSettingsCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                default_level: input.default_level.into(),
                keywords: input.keywords,
                dnd: input.dnd.map(TryFrom::try_from).transpose()?,
                digest: input.digest.into(),
            })
            .await
            .map_err(map_service_error)?;
        Ok(settings.into())
    }

    /// Set the viewer's level for one conversation they can read; `level: null` goes back to
    /// the workspace default. `muteConversation`/`unmuteConversation` keep working alongside.
    async fn set_conversation_notification_level(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        level: Option<NotificationLevel>,
    ) -> Result<ConversationNotificationLevel> {
        let actor = require_actor(ctx)?;
        let dto = notifications(ctx)?
            .set_conversation_level(SetConversationNotificationLevelCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                conversation_id: parse_uuid_id(&conversation_id)?,
                level: level.map(Into::into),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// Mark the viewer's notifications read: the given `ids`, or every notification created at
    /// or before `allBefore` (exactly one of the two). Returns how many changed.
    async fn mark_notifications_read(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        ids: Option<Vec<ID>>,
        all_before: Option<DateTime<Utc>>,
    ) -> Result<i64> {
        let actor = require_actor(ctx)?;
        let target = match (ids, all_before) {
            (Some(ids), None) => MarkNotificationsReadTarget::Ids(
                ids.iter().map(parse_uuid_id).collect::<Result<Vec<_>>>()?,
            ),
            (None, Some(before)) => MarkNotificationsReadTarget::AllBefore(before),
            _ => return Err(validation("provide exactly one of `ids` or `allBefore`")),
        };
        let changed = notifications(ctx)?
            .mark_read(MarkNotificationsReadCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                target,
            })
            .await
            .map_err(map_service_error)?;
        Ok(i64::try_from(changed).unwrap_or(i64::MAX))
    }

    /// Turn the viewer's manual do-not-disturb pause on (until turned off, or until `until`)
    /// or off. While paused, notifications are still recorded but arrive `quiet`.
    async fn set_do_not_disturb(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        paused: bool,
        until: Option<DateTime<Utc>>,
    ) -> Result<NotificationSettings> {
        let actor = require_actor(ctx)?;
        let pause = match (paused, until) {
            (false, None) => DndPause::Off,
            (false, Some(_)) => return Err(validation("`until` needs `paused: true`")),
            (true, None) => DndPause::Indefinite,
            (true, Some(until)) => DndPause::Until(until),
        };
        let settings = notifications(ctx)?
            .set_dnd_pause(SetDndPauseCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&server_id)?,
                pause,
            })
            .await
            .map_err(map_service_error)?;
        Ok(settings.into())
    }
}
