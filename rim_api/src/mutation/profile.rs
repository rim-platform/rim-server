use async_graphql::*;
use rim_services::Actor;
use rim_services::user::command::UpdateMyProfileCmd;
use rim_services::user::service::UserService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::federation::ensure_may_set_display_name;
use crate::types::User;

#[derive(Default)]
pub(crate) struct ProfileMutations;

#[Object]
impl ProfileMutations {
    /// Update fields on the authenticated user's profile.
    ///
    /// Returns the full updated `User` so the client can refresh local cache
    /// without an extra `me { ... }` round-trip.
    ///
    /// # Errors
    ///
    /// - `UNAUTHENTICATED` — no session.
    /// - `FORBIDDEN` — `displayName` from a member whose relay is another server; their
    ///   display name comes from their profile card.
    /// - `VALIDATION` — `displayName` > 64 chars, `statusText` > 256 chars,
    ///   `bio` > 2000 chars, or `avatarUrl` not under `/api/files/`.
    /// - `NOT_FOUND` — caller's user row missing (should not happen in practice).
    /// - `INTERNAL` — repository failure.
    ///
    /// # Examples
    ///
    /// ```graphql
    /// mutation Update($input: UpdateMyProfileInput!) {
    ///   updateMyProfile(input: $input) {
    ///     id username displayName avatarUrl statusText bio
    ///   }
    /// }
    /// ```
    async fn update_my_profile(
        &self,
        ctx: &Context<'_>,
        input: UpdateMyProfileInput,
    ) -> Result<User> {
        let actor = require_actor(ctx)?;
        if input.display_name.is_some() {
            ensure_may_set_display_name(ctx, actor).await?;
        }
        let svc = ctx.data::<std::sync::Arc<dyn UserService>>()?;

        let dto = svc
            .update_my_profile(UpdateMyProfileCmd {
                actor: Actor::new(actor),
                display_name: input.display_name,
                avatar_url: input.avatar_url,
                status_text: input.status_text,
                bio: input.bio,
            })
            .await
            .map_err(map_service_error)?;

        Ok(dto.into())
    }
}

/// Patch fields on the authenticated user's profile.
///
/// Each field is independently optional. Omitting a field leaves the existing
/// value unchanged. Sending an explicit empty string clears the field
/// (after trim + control-char strip).
#[derive(InputObject)]
pub struct UpdateMyProfileInput {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
}
