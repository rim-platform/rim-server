use std::sync::Arc;

use async_trait::async_trait;

use rim_core::user::avatar::own_avatar_key;
use rim_core::user::model::{UpdateProfilePatch, UserHandle, UserId};
use rim_services::limits::InstanceLimits;
use rim_services::storage::service::StorageService;
use rim_services::user::command::{GetMeQuery, UpdateMyProfileCmd};
use rim_services::user::dto::UserDto;
use rim_services::user::service::UserService;
use rim_services::{ServiceError, ServiceResult};

use crate::UserRepos;
use crate::clock::Clock;
use crate::display_name;
use crate::reserved_names::ReservedNames;

/// Strip control characters (except newline/tab in bio) from a string.
fn strip_control_chars(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// Strip control characters for single-line fields (no newlines/tabs).
fn strip_control_chars_single_line(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

#[derive(Clone)]
pub struct DefaultUserService {
    repos: UserRepos,
    /// The probation of new accounts.
    limits: InstanceLimits,
    clock: Clock,
    /// Display names a profile edit may not take.
    reserved_names: ReservedNames,
    /// This server's handle domain; local accounts have no handle without one.
    handle_domain: Option<String>,
    /// Where avatar files are stored; without it a profile change deletes no file.
    storage: Option<Arc<dyn StorageService>>,
    profile_events: Option<(
        Arc<dyn rim_core::server::repository::ServerRepository>,
        rim_services::events::Observers,
    )>,
    direct_space_id: Option<rim_core::server::model::ServerId>,
}

impl DefaultUserService {
    pub fn new(repos: UserRepos) -> Self {
        Self {
            repos,
            limits: InstanceLimits::default(),
            clock: Clock::default(),
            reserved_names: ReservedNames::default(),
            handle_domain: None,
            storage: None,
            profile_events: None,
            direct_space_id: None,
        }
    }

    pub fn with_profile_events(
        mut self,
        servers: Arc<dyn rim_core::server::repository::ServerRepository>,
        observers: rim_services::events::Observers,
        direct_space_id: Option<rim_core::server::model::ServerId>,
    ) -> Self {
        self.profile_events = Some((servers, observers));
        self.direct_space_id = direct_space_id;
        self
    }

    /// Delete the user's unused avatar files from `storage` when a profile change clears or
    /// replaces the avatar.
    pub fn with_storage(mut self, storage: Arc<dyn StorageService>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Give local accounts handles at `handle_domain`.
    pub fn with_handle_domain(mut self, handle_domain: Option<String>) -> Self {
        self.handle_domain = handle_domain;
        self
    }

    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_reserved_names(mut self, reserved_names: ReservedNames) -> Self {
        self.reserved_names = reserved_names;
        self
    }

    /// Refuse a reserved display name, unless the account's own username is reserved: such an
    /// account belongs to the instance's maintainers, who hold the names.
    async fn check_display_name(&self, user_id: UserId, display_name: &str) -> ServiceResult<()> {
        if !self.reserved_names.is_reserved(display_name) {
            return Ok(());
        }
        let user = self
            .repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if self.reserved_names.is_reserved(&user.username) {
            return Ok(());
        }
        self.reserved_names.check(display_name)
    }

    /// The account's handle: a local account's username at this server's handle domain, a
    /// remote user's handle at their home relay, or `None`.
    fn handle(&self, user: &rim_core::user::model::User) -> Option<String> {
        match &user.handle {
            UserHandle::Local => self
                .handle_domain
                .as_deref()
                .map(|domain| rim_s2s::format_handle(&user.username.to_ascii_lowercase(), domain)),
            UserHandle::Home { username, domain } => Some(rim_s2s::format_handle(username, domain)),
            UserHandle::Unknown => None,
        }
    }

    fn convert_to_dto(&self, user: rim_core::user::model::User) -> UserDto {
        UserDto {
            handle: self.handle(&user),
            id: user.id,
            username: user.username,
            display_name: user.display_name,
            avatar_url: user.avatar_url,
            status_text: user.status_text,
            bio: user.bio,
            principal_id: user.principal_id,
            deletion_scheduled_for: None,
        }
    }
}

#[async_trait]
impl UserService for DefaultUserService {
    async fn get_me(&self, query: GetMeQuery) -> ServiceResult<UserDto> {
        let user = self
            .repos
            .user_repo
            .get_by_id(query.actor.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let deletion_scheduled_for = user.deletion_scheduled_for;

        Ok(UserDto {
            deletion_scheduled_for,
            ..self.convert_to_dto(user)
        })
    }

    async fn probation_ends_at(
        &self,
        query: GetMeQuery,
    ) -> ServiceResult<Option<chrono::DateTime<chrono::Utc>>> {
        if self.limits.probation_hours.is_none() {
            return Ok(None);
        }
        let user = self
            .repos
            .user_repo
            .get_by_id(query.actor.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        crate::probation::probation_ends_at(
            &self.limits,
            self.repos.user_repo.as_ref(),
            &user,
            self.clock.now(),
        )
        .await
    }

    async fn get_users_by_ids(
        &self,
        ids: &[rim_core::user::model::UserId],
    ) -> ServiceResult<Vec<UserDto>> {
        let users = self.repos.user_repo.get_by_ids(ids).await?;

        Ok(users.into_iter().map(|u| self.convert_to_dto(u)).collect())
    }

    async fn update_my_profile(&self, cmd: UpdateMyProfileCmd) -> ServiceResult<UserDto> {
        // The display name follows the rules every path shares; nothing visible left clears it.
        let display_name = cmd
            .display_name
            .map(|v| display_name::clean_local(&v).map(|v| (!v.is_empty()).then_some(v)))
            .transpose()?;
        if let Some(Some(ref dn)) = display_name {
            self.check_display_name(cmd.actor.user_id, dn).await?;
        }

        // Sanitize and validate status_text.
        let status_text = cmd.status_text.map(|v| {
            let v = strip_control_chars_single_line(v.trim());
            if v.is_empty() { None } else { Some(v) }
        });
        if let Some(Some(ref st)) = status_text
            && st.chars().count() > 256
        {
            return Err(ServiceError::Validation(
                "status_text must be at most 256 characters".to_string(),
            ));
        }

        // Sanitize and validate bio.
        let bio = cmd.bio.map(|v| {
            let v = strip_control_chars(v.trim());
            if v.is_empty() { None } else { Some(v) }
        });
        if let Some(Some(ref b)) = bio
            && b.chars().count() > 2000
        {
            return Err(ServiceError::Validation(
                "bio must be at most 2000 characters".to_string(),
            ));
        }

        // An avatar URL is empty or names one of the caller's own uploaded avatar files.
        let user_id = cmd.actor.user_id;
        let avatar_url = cmd.avatar_url.map(|v| {
            let v = v.trim().to_string();
            if v.is_empty() { None } else { Some(v) }
        });
        let avatar_key = match &avatar_url {
            Some(Some(url)) => Some(Some(own_avatar_key(user_id, url).ok_or_else(|| {
                ServiceError::Validation(
                    "avatar_url must be the URL of your own uploaded avatar".to_string(),
                )
            })?)),
            Some(None) => Some(None),
            None => None,
        };

        let mut workspaces = match &self.profile_events {
            Some((servers, _)) => servers
                .get_servers_for_user(user_id)
                .await?
                .into_iter()
                .map(|server| server.id)
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };
        if let Some(direct) = self.direct_space_id {
            workspaces.push(direct);
        }
        let user = self
            .repos
            .user_repo
            .update_profile(
                user_id,
                UpdateProfilePatch {
                    display_name,
                    avatar_url,
                    status_text,
                    bio,
                },
            )
            .await?;

        // The profile is saved; a file that cannot be deleted now goes with the next upload,
        // the next profile change or the account.
        if let (Some(keep), Some(storage)) = (avatar_key, &self.storage)
            && let Err(e) = storage.delete_avatar_files(user_id, keep).await
        {
            tracing::warn!(user_id = %user_id, error = %e, "could not delete unused avatar files");
        }

        if let Some((_, observers)) = &self.profile_events {
            use rim_services::events::EventObserver;
            for workspace_id in workspaces {
                observers.emit(rim_services::events::WorkspaceEvent::MemberProfileUpdated {
                    workspace_id,
                    user_id,
                });
            }
        }
        Ok(self.convert_to_dto(user))
    }
}
