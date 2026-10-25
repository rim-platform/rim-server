//! The server's default workspace: the workspace every account created here joins as a Member.
//! It is a workspace made in the app and named by its id, or the workspace a definition file
//! describes, which the first account to sign up through the owner claim link creates and owns.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use rim_core::auth::repository::InstanceInviteRepository;
use rim_core::conversation::model::{ConversationType, NewConversation};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::default_workspace::repository::DefaultWorkspaceRepository;
use rim_core::dm_policy::repository::DmPolicyRepository;
use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_services::default_workspace::DefaultWorkspaceService;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::{ServiceError, ServiceResult};

use crate::instance_invite::{InviteTerms, MAX_TTL_DAYS, issue};
use crate::secret_file::write_secret;
use crate::signup::{CheckedToken, SignupRepos};
use crate::util::add_workspace_member;

#[cfg(test)]
mod claim_tests;
mod definition;

pub use definition::{ChannelDefinition, DmDefinition, WorkspaceDefinition};

/// The name of the file in the data directory that holds the owner claim link.
pub const CLAIM_LINK_FILE: &str = "owner-claim.txt";

/// Where the default workspace comes from.
#[derive(Clone, Debug, Default)]
pub enum DefaultWorkspace {
    /// No default workspace: a new account joins only the workspace its invite names.
    #[default]
    None,
    /// A workspace made in the app, named by its id.
    Fixed(ServerId),
    /// The workspace a definition file describes, once an owner claimed it.
    Defined(Arc<DefinedWorkspace>),
}

impl From<Option<ServerId>> for DefaultWorkspace {
    fn from(workspace: Option<ServerId>) -> Self {
        workspace.map_or(Self::None, Self::Fixed)
    }
}

/// A definition file and where its owner claim link is written.
#[derive(Clone, Debug)]
pub struct DefinedWorkspace {
    pub definition: WorkspaceDefinition,
    /// The file holding the owner claim link; the claim deletes it.
    pub claim_link_file: PathBuf,
}

/// The repositories the start reads and applies the default workspace with.
#[derive(Clone)]
pub struct StartRepos {
    pub default_workspace_repo: Arc<dyn DefaultWorkspaceRepository>,
    pub instance_invite_repo: Arc<dyn InstanceInviteRepository>,
    pub server_repo: Arc<dyn ServerRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub dm_policy_repo: Arc<dyn DmPolicyRepository>,
    /// Tells whether the creator of a channel the definition names is the workspace's Owner or
    /// a Moderator.
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

/// What the start found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    /// No default workspace.
    None,
    /// The configured workspace exists.
    Fixed(ServerId),
    /// The claimed workspace, with what applying the definition changed.
    Applied {
        workspace: ServerId,
        changes: AppliedChanges,
    },
    /// Nothing claimed yet: a new owner claim link, valid until `expires_at`, is in `file`.
    /// `workspace_deleted` when a workspace was claimed before and has been deleted since.
    ClaimWaiting {
        file: PathBuf,
        expires_at: DateTime<Utc>,
        workspace_deleted: bool,
    },
}

/// What applying a definition to the claimed workspace changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppliedChanges {
    /// The new name, when the workspace was renamed.
    pub renamed_to: Option<String>,
    /// The titles of the channels created.
    pub channels_added: Vec<String>,
    /// The titles of the channels whose topic was set.
    pub topics_set: Vec<String>,
    /// The titles of the channels left alone because a channel of that title exists that
    /// neither the definition nor an Owner or Moderator created.
    pub channels_taken: Vec<String>,
    /// Whether the DM policy was set.
    pub dm_policy_set: bool,
}

/// What the start is told about the server.
#[derive(Debug, Clone, Copy)]
pub struct StartContext<'a> {
    /// The relay's direct space, which can never be the default workspace.
    pub direct_space_id: ServerId,
    /// Whether signups take an invite token; an owner claim link needs it.
    pub signups_take_tokens: bool,
    /// The origin a claim link points at, without a trailing slash.
    pub link_base: &'a str,
    pub now: DateTime<Utc>,
}

/// Check and apply the default workspace at start, before any signup runs.
///
/// - [`DefaultWorkspace::Fixed`]: the workspace must exist and not be the direct space.
/// - [`DefaultWorkspace::Defined`], claimed: the definition is applied to the workspace (see
///   [`apply`]).
/// - [`DefaultWorkspace::Defined`], not claimed: the previous claim invite is revoked, a new
///   single-use one valid for the longest invite lifetime is issued, and its link
///   (`<link base>/join/<token>`) is written to the claim link file, readable by its owner
///   only.
/// - Otherwise a claim invite left from an earlier start is revoked.
///
/// # Errors
/// A message naming the problem: a missing workspace, a claim waiting on a server whose signups
/// take no token, a storage error, or a claim link file that cannot be written.
pub async fn start(
    repos: &StartRepos,
    setting: &DefaultWorkspace,
    ctx: StartContext<'_>,
) -> Result<Started, String> {
    let record = repos
        .default_workspace_repo
        .record()
        .await
        .map_err(|e| format!("could not read the default workspace record: {e}"))?;
    let defined = match setting {
        DefaultWorkspace::Defined(defined) => defined,
        other => {
            if let Some(invite) = record.claim_invite_id {
                repos
                    .instance_invite_repo
                    .revoke(invite)
                    .await
                    .map_err(|e| format!("could not revoke the owner claim invite: {e}"))?;
            }
            return match other {
                DefaultWorkspace::Fixed(workspace) => {
                    check(repos.server_repo.as_ref(), ctx.direct_space_id, *workspace).await?;
                    Ok(Started::Fixed(*workspace))
                }
                _ => Ok(Started::None),
            };
        }
    };
    if let Some(workspace) = record.workspace_id {
        let changes = apply(repos, workspace, &defined.definition)
            .await
            .map_err(|e| format!("could not apply the definition to workspace {workspace}: {e}"))?;
        return Ok(Started::Applied { workspace, changes });
    }
    if !ctx.signups_take_tokens {
        return Err(
            "the owner claim link is an invite, and open registration takes none; \
                    turn RIM_OPEN_REGISTRATION off until the workspace is claimed"
                .to_string(),
        );
    }
    if let Some(invite) = record.claim_invite_id {
        repos
            .instance_invite_repo
            .revoke(invite)
            .await
            .map_err(|e| format!("could not revoke the previous owner claim invite: {e}"))?;
    }
    let issued = issue(
        repos.instance_invite_repo.as_ref(),
        None,
        InviteTerms {
            expires_at: Some(ctx.now + Duration::days(MAX_TTL_DAYS)),
            max_uses: Some(1),
            email: None,
        },
        ctx.now,
    )
    .await
    .map_err(|e| format!("could not issue the owner claim invite: {e}"))?;
    repos
        .default_workspace_repo
        .set_claim_invite(issued.invite.id)
        .await
        .map_err(|e| format!("could not record the owner claim invite: {e}"))?;
    let file = &defined.claim_link_file;
    remove_claim_link(file).map_err(|e| format!("cannot replace {}: {e}", file.display()))?;
    write_secret(file, &format!("{}/join/{}\n", ctx.link_base, issued.token))
        .map_err(|e| format!("{e:#}"))?;
    tracing::info!(
        target: "audit",
        event = "default_workspace_claim_issued",
        instance_invite_id = %issued.invite.id,
        expires_at = %issued.invite.expires_at,
        "owner claim invite issued for the default workspace"
    );
    Ok(Started::ClaimWaiting {
        file: file.clone(),
        expires_at: issued.invite.expires_at,
        workspace_deleted: record.claimed_at.is_some(),
    })
}

/// Remove the claim link file; a file that is not there is not an error.
fn remove_claim_link(file: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Apply `definition` to the claimed `workspace`: rename it when the name differs, create the
/// channels missing by title (with no members; new members join the public ones), set the
/// topics the definition gives where they differ, and set the DM policy when it differs.
/// Nothing is deleted, no channel's privacy changes and nobody's membership or role changes.
/// A channel of a definition's title is taken as the definition's only when the definition, the
/// Owner or a Moderator created it; any other is left alone and named in `channels_taken`.
///
/// # Errors
/// Storage errors; a workspace that no longer exists is `NotFound`.
pub async fn apply(
    repos: &StartRepos,
    workspace: ServerId,
    definition: &WorkspaceDefinition,
) -> ServiceResult<AppliedChanges> {
    let server = repos
        .server_repo
        .get_by_id(workspace)
        .await?
        .ok_or(rim_services::ServiceError::NotFound)?;
    let mut changes = AppliedChanges::default();
    if server.name != definition.name {
        repos
            .server_repo
            .rename(workspace, &definition.name)
            .await?;
        changes.renamed_to = Some(definition.name.clone());
    }
    for channel in &definition.channels {
        let existing = repos
            .conversation_repo
            .get_channel_by_title(workspace, &channel.title)
            .await?;
        let (id, topic) = match existing {
            Some(conversation)
                if !created_by_staff(repos, workspace, conversation.created_by).await? =>
            {
                changes.channels_taken.push(channel.title.clone());
                continue;
            }
            Some(conversation) => (conversation.id, conversation.topic),
            None => {
                let created = repos
                    .conversation_repo
                    .create_conversation_with_members(
                        new_channel(workspace, channel, None),
                        &[],
                        &[],
                    )
                    .await?;
                changes.channels_added.push(channel.title.clone());
                (created.id, None)
            }
        };
        if channel.topic.is_some() && channel.topic != topic {
            repos
                .conversation_repo
                .update_topic(id, channel.topic.clone())
                .await?;
            changes.topics_set.push(channel.title.clone());
        }
    }
    if let Some(dm) = definition.dm {
        let current = repos.dm_policy_repo.workspace_policy(workspace).await?;
        if current.is_none_or(|p| {
            p.outside_floor != dm.outside_floor || p.member_default != dm.member_default
        }) {
            repos
                .dm_policy_repo
                .set_workspace_policy(workspace, dm.outside_floor, dm.member_default)
                .await?;
            changes.dm_policy_set = true;
        }
    }
    Ok(changes)
}

/// Whether a channel of `workspace` created by `creator` counts as the workspace's own: one the
/// definition created (no creator), or one its Owner or a Moderator created.
async fn created_by_staff(
    repos: &StartRepos,
    workspace: ServerId,
    creator: Option<UserId>,
) -> ServiceResult<bool> {
    let Some(creator) = creator else {
        return Ok(true);
    };
    Ok(repos
        .server_member_repo
        .get_member(workspace, creator)
        .await?
        .is_some_and(|m| m.member_role.is_moderator()))
}

fn new_channel(
    workspace: ServerId,
    channel: &ChannelDefinition,
    created_by: Option<UserId>,
) -> NewConversation {
    NewConversation {
        server_id: workspace,
        conversation_type: ConversationType::Channel,
        title: Some(channel.title.clone()),
        is_private: channel.private,
        created_by,
        encrypted: false,
    }
}

/// Check that `workspace` can be the default workspace: it exists on this server and is not the
/// direct space `direct_space_id`.
///
/// # Errors
/// A message naming the problem, or the storage error the lookup failed with.
pub async fn check(
    server_repo: &dyn ServerRepository,
    direct_space_id: ServerId,
    workspace: ServerId,
) -> Result<(), String> {
    if workspace == direct_space_id {
        return Err(format!("{workspace} is the direct space, not a workspace"));
    }
    match server_repo.get_by_id(workspace).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(format!("no workspace {workspace} exists on this server")),
        Err(e) => Err(format!("could not look workspace {workspace} up: {e}")),
    }
}

/// The default workspace now: the configured one, or the claimed one read through
/// `default_workspace_repo` (the signup's transaction, when it runs in one).
///
/// # Errors
/// Storage errors reading the claimed workspace.
async fn current(
    default_workspace_repo: &dyn DefaultWorkspaceRepository,
    setting: &DefaultWorkspace,
) -> ServiceResult<Option<ServerId>> {
    Ok(match setting {
        DefaultWorkspace::None => None,
        DefaultWorkspace::Fixed(workspace) => Some(*workspace),
        DefaultWorkspace::Defined(_) => default_workspace_repo.record().await?.workspace_id,
    })
}

/// The workspace created by an owner claim, for what the signup announces once it committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Claimed {
    pub workspace_id: ServerId,
    /// The existing local accounts added as Members, with their usernames.
    pub members: Vec<(UserId, String)>,
    pub operator_granted: bool,
    pub claim_link_file: PathBuf,
}

/// Whether `token` is the owner claim invite of a defined workspace nobody claimed yet: the
/// signup that redeems it becomes the workspace's Owner.
///
/// # Errors
/// Storage errors reading the default workspace record.
pub(crate) async fn is_claim(
    repos: &SignupRepos,
    setting: &DefaultWorkspace,
    token: Option<&CheckedToken>,
) -> ServiceResult<bool> {
    let (DefaultWorkspace::Defined(_), Some(CheckedToken::InstanceInvite(invite))) =
        (setting, token)
    else {
        return Ok(false);
    };
    let record = repos.default_workspace_repo.record().await?;
    Ok(record.workspace_id.is_none() && record.claim_invite_id == Some(invite.id))
}

/// Claim the defined workspace for the new account `owner`, inside the signup's transaction,
/// when `token` is the owner claim invite (already consumed by the signup): create the
/// workspace with `owner` as Owner, its channels with their topics and its DM policy; record it
/// as the default workspace; add every other local account as a Member; and make `owner` an
/// instance operator when the definition says so. Returns `None` for every other signup.
///
/// # Errors
/// Storage errors; the caller's transaction rolls back and the invite stays unconsumed.
pub(crate) async fn claim(
    repos: &SignupRepos,
    setting: &DefaultWorkspace,
    token: Option<&CheckedToken>,
    owner: UserId,
) -> ServiceResult<Option<Claimed>> {
    let (DefaultWorkspace::Defined(defined), Some(CheckedToken::InstanceInvite(invite))) =
        (setting, token)
    else {
        return Ok(None);
    };
    if !repos.default_workspace_repo.take_claim(invite.id).await? {
        return Ok(None);
    }
    let definition = &defined.definition;
    let server = repos.server_repo.create_server(&definition.name).await?;
    add_workspace_member(
        repos.server_member_repo.as_ref(),
        repos.conversation_repo.as_ref(),
        repos.conversation_member_repo.as_ref(),
        server.id,
        owner,
        ServerMemberRole::Owner,
        None,
    )
    .await?;
    for channel in &definition.channels {
        let created = repos
            .conversation_repo
            .create_conversation_with_members(
                new_channel(server.id, channel, Some(owner)),
                &[owner],
                &[],
            )
            .await?;
        if channel.topic.is_some() {
            repos
                .conversation_repo
                .update_topic(created.id, channel.topic.clone())
                .await?;
        }
    }
    if let Some(dm) = definition.dm {
        repos
            .dm_policy_repo
            .set_workspace_policy(server.id, dm.outside_floor, dm.member_default)
            .await?;
    }
    repos
        .default_workspace_repo
        .record_claim(server.id, owner)
        .await?;
    let others = repos
        .default_workspace_repo
        .other_local_accounts(owner)
        .await?;
    for user_id in &others {
        add_workspace_member(
            repos.server_member_repo.as_ref(),
            repos.conversation_repo.as_ref(),
            repos.conversation_member_repo.as_ref(),
            server.id,
            *user_id,
            ServerMemberRole::Member,
            None,
        )
        .await?;
    }
    let members = if others.is_empty() {
        Vec::new()
    } else {
        repos
            .user_repo
            .get_by_ids(&others)
            .await?
            .into_iter()
            .map(|user| (user.id, user.username))
            .collect()
    };
    if definition.claim_grants_operator {
        repos.user_repo.set_instance_operator(owner, true).await?;
    }
    Ok(Some(Claimed {
        workspace_id: server.id,
        members,
        operator_granted: definition.claim_grants_operator,
        claim_link_file: defined.claim_link_file.clone(),
    }))
}

/// After the claiming signup committed: delete the claim link file, tell the workspace about
/// the accounts added as Members, and write the audit line.
pub(crate) fn announce_claim(claimed: Option<Claimed>, owner: UserId, observers: &Observers) {
    let Some(claimed) = claimed else {
        return;
    };
    if let Err(e) = remove_claim_link(&claimed.claim_link_file) {
        tracing::warn!(
            file = %claimed.claim_link_file.display(),
            error = %e,
            "the default workspace is claimed, but its claim link file could not be deleted"
        );
    }
    for (user_id, username) in &claimed.members {
        observers.emit(WorkspaceEvent::MemberJoined {
            workspace_id: claimed.workspace_id,
            user_id: *user_id,
            username: username.clone(),
            source: None,
        });
    }
    tracing::info!(
        target: "audit",
        event = "default_workspace_claimed",
        user_id = %owner,
        server_id = %claimed.workspace_id,
        members_added = claimed.members.len(),
        operator_granted = claimed.operator_granted,
        "the default workspace was created and claimed by its owner"
    );
}

/// Make the new account `user_id` a Member of the default workspace inside the signup's
/// transaction, with the public-channel auto-join every new member gets. The members-per-workspace
/// cap does not apply. Nothing happens without a default workspace, when the account already
/// belongs to it (it signed up with an invite to it, claimed it, or was a member from another
/// server), when it is banned there, or when the workspace was deleted since the start. Returns
/// the workspace when the account joined it.
///
/// # Errors
/// Storage errors; the caller's transaction rolls back.
pub(crate) async fn join(
    repos: &SignupRepos,
    setting: &DefaultWorkspace,
    user_id: UserId,
) -> ServiceResult<Option<ServerId>> {
    let Some(workspace) = current(repos.default_workspace_repo.as_ref(), setting).await? else {
        return Ok(None);
    };
    if repos
        .server_member_repo
        .is_member(workspace, user_id)
        .await?
    {
        return Ok(None);
    }
    if repos
        .moderation_repo
        .get_active_ban(workspace, user_id, Utc::now())
        .await?
        .is_some()
    {
        return Ok(None);
    }
    if repos.server_repo.get_by_id(workspace).await?.is_none() {
        tracing::warn!(
            server_id = %workspace,
            "the default workspace no longer exists; the new account joins no workspace"
        );
        return Ok(None);
    }
    add_workspace_member(
        repos.server_member_repo.as_ref(),
        repos.conversation_repo.as_ref(),
        repos.conversation_member_repo.as_ref(),
        workspace,
        user_id,
        ServerMemberRole::Member,
        None,
    )
    .await?;
    Ok(Some(workspace))
}

/// After the signup committed: when the account joined the default workspace `joined`, the
/// `MemberJoined` event other members see the newcomer by, and its audit line.
pub(crate) fn announce(
    joined: Option<ServerId>,
    user_id: UserId,
    username: &str,
    observers: &Observers,
) {
    let Some(workspace_id) = joined else {
        return;
    };
    observers.emit(WorkspaceEvent::MemberJoined {
        workspace_id,
        user_id,
        username: username.to_string(),
        source: None,
    });
    tracing::info!(
        target: "audit",
        event = "default_workspace_joined",
        user_id = %user_id,
        server_id = %workspace_id,
        "new account joined the default workspace"
    );
}

/// [`DefaultWorkspaceService`] over the instance's setting and its record.
pub struct DefaultDefaultWorkspaceService {
    setting: DefaultWorkspace,
    repo: Arc<dyn DefaultWorkspaceRepository>,
    join_repos: Option<SignupRepos>,
    db: Option<sea_orm::DatabaseConnection>,
    observers: Observers,
}

impl DefaultDefaultWorkspaceService {
    pub fn new(setting: DefaultWorkspace, repo: Arc<dyn DefaultWorkspaceRepository>) -> Self {
        Self {
            setting,
            repo,
            join_repos: None,
            db: None,
            observers: Observers::new(Arc::new(rim_services::events::NoopBroadcaster)),
        }
    }
    pub fn with_database(mut self, db: sea_orm::DatabaseConnection) -> Self {
        self.db = Some(db);
        self
    }

    pub fn with_join(mut self, repos: SignupRepos, observers: Observers) -> Self {
        self.join_repos = Some(repos);
        self.observers = observers;
        self
    }
}

#[async_trait]
impl DefaultWorkspaceService for DefaultDefaultWorkspaceService {
    async fn current(&self) -> ServiceResult<Option<ServerId>> {
        current(self.repo.as_ref(), &self.setting).await
    }
    async fn join(&self, user_id: UserId) -> ServiceResult<Option<ServerId>> {
        let scope = match &self.db {
            Some(db) => Some(
                rim_infra_db::begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let transaction_repos = scope.as_ref().map(|scope| {
            crate::repos::AppRepos::with_handle(&scope.handle())
                .for_auth()
                .signup_repos()
        });
        let repos = transaction_repos
            .as_ref()
            .or(self.join_repos.as_ref())
            .ok_or(ServiceError::Unavailable)?;
        let user = repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if !matches!(user.handle, rim_core::user::model::UserHandle::Local) {
            return Err(ServiceError::PermissionDenied);
        }
        let workspace = self.current().await?.ok_or(ServiceError::NotFound)?;
        crate::util::ensure_not_banned(repos.moderation_repo.as_ref(), workspace, user_id).await?;
        let joined = join(repos, &self.setting, user_id).await?;
        if joined.is_none()
            && !repos
                .server_member_repo
                .is_member(workspace, user_id)
                .await?
        {
            crate::util::ensure_not_banned(repos.moderation_repo.as_ref(), workspace, user_id)
                .await?;
            return Err(ServiceError::NotFound);
        }
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        announce(joined, user_id, &user.username, &self.observers);
        Ok(Some(workspace))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rim_core::auth::repository::{MockInstanceInviteRepository, MockInviteRepository};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::model::{ConversationMember, ConversationMemberId};
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::moderation::model::ServerBan;
    use rim_core::moderation::repository::MockModerationRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::server::model::Server;
    use rim_core::server::repository::MockServerRepository;
    use rim_core::server_member::model::{ServerMember, ServerMemberId};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::repository::MockUserRepository;
    use rim_services::events::Broadcaster;

    use super::*;

    fn server(id: ServerId) -> Server {
        Server {
            id,
            name: "Lobby".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn servers_with(id: ServerId) -> MockServerRepository {
        let mut repo = MockServerRepository::new();
        repo.expect_get_by_id()
            .returning(move |asked| Ok((asked == id).then(|| server(id))));
        repo
    }

    fn not_banned() -> MockModerationRepository {
        let mut repo = MockModerationRepository::new();
        repo.expect_get_active_ban()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        repo
    }

    fn repos(
        servers: MockServerRepository,
        members: MockServerMemberRepository,
        moderation: MockModerationRepository,
        conversations: MockConversationRepository,
        conversation_members: MockConversationMemberRepository,
    ) -> SignupRepos {
        SignupRepos {
            instance_invite_repo: Arc::new(MockInstanceInviteRepository::new()),
            invite_repo: Arc::new(MockInviteRepository::new()),
            server_repo: Arc::new(servers),
            server_member_repo: Arc::new(members),
            conversation_repo: Arc::new(conversations),
            conversation_member_repo: Arc::new(conversation_members),
            moderation_repo: Arc::new(moderation),
            user_repo: Arc::new(MockUserRepository::new()),
            dm_policy_repo: Arc::new(
                rim_core::dm_policy::repository::MockDmPolicyRepository::new(),
            ),
            default_workspace_repo: Arc::new(
                rim_core::default_workspace::repository::MockDefaultWorkspaceRepository::new(),
            ),
        }
    }

    /// Server membership that answers `is_member` with `member` and expects `adds` inserts.
    fn members(member: bool, adds: usize) -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        repo.expect_is_member().returning(move |_, _| Ok(member));
        repo.expect_add_member()
            .times(adds)
            .returning(|server_id, user_id, member_role| {
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: Utc::now(),
                })
            });
        repo
    }

    #[tokio::test]
    async fn check_accepts_an_ordinary_workspace_of_this_server() {
        let workspace = ServerId::new();
        let res = check(&servers_with(workspace), ServerId::new(), workspace).await;
        assert_eq!(res, Ok(()));
    }

    #[tokio::test]
    async fn check_refuses_an_unknown_workspace_naming_it() {
        let workspace = ServerId::new();
        let err = check(&servers_with(ServerId::new()), ServerId::new(), workspace)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            format!("no workspace {workspace} exists on this server")
        );
    }

    #[tokio::test]
    async fn check_refuses_the_direct_space_without_a_lookup() {
        let direct = ServerId::new();
        let mut servers = MockServerRepository::new();
        servers.expect_get_by_id().never();
        let err = check(&servers, direct, direct).await.unwrap_err();
        assert_eq!(
            err,
            format!("{direct} is the direct space, not a workspace")
        );
    }

    #[tokio::test]
    async fn check_reports_a_failed_lookup() {
        let mut servers = MockServerRepository::new();
        servers
            .expect_get_by_id()
            .returning(|_| Err(RepoError::new(RepoErrorKind::Internal)));
        let err = check(&servers, ServerId::new(), ServerId::new())
            .await
            .unwrap_err();
        assert!(err.starts_with("could not look workspace"), "{err}");
    }

    #[tokio::test]
    async fn self_join_refuses_remote_accounts_and_active_bans() {
        for remote in [true, false] {
            let workspace = ServerId::new();
            let id = UserId::new();
            let mut moderation = MockModerationRepository::new();
            moderation
                .expect_get_active_ban()
                .times(usize::from(!remote))
                .returning(move |_, _, _| {
                    Box::pin(async move {
                        Ok(Some(ServerBan {
                            server_id: workspace,
                            user_id: id,
                            reason: None,
                            banned_by: None,
                            created_at: Utc::now(),
                            expires_at: None,
                        }))
                    })
                });
            let mut r = repos(
                servers_with(workspace),
                members(false, 0),
                moderation,
                MockConversationRepository::new(),
                MockConversationMemberRepository::new(),
            );
            let mut account = crate::probation_fixtures::account(id, Utc::now(), None);
            if remote {
                account.handle = rim_core::user::model::UserHandle::Unknown;
            }
            r.user_repo = Arc::new(crate::probation_fixtures::users(vec![account]));
            let service = DefaultDefaultWorkspaceService::new(
                DefaultWorkspace::Fixed(workspace),
                Arc::clone(&r.default_workspace_repo),
            )
            .with_join(
                r,
                Observers::new(Arc::new(rim_services::events::NoopBroadcaster)),
            );
            let result = DefaultWorkspaceService::join(&service, id).await;
            assert!(matches!(
                result,
                Err(ServiceError::PermissionDenied) | Err(ServiceError::Forbidden(_))
            ));
        }
    }

    /// The account becomes a Member and joins the public channels it is not in yet.
    #[tokio::test]
    async fn join_adds_a_member_with_the_public_channels() {
        let workspace = ServerId::new();
        let user_id = UserId::new();
        let lobby = rim_core::conversation::model::ConversationId::new();

        let mut server_members = MockServerMemberRepository::new();
        server_members
            .expect_is_member()
            .withf(move |s, u| *s == workspace && *u == user_id)
            .returning(|_, _| Ok(false));
        server_members
            .expect_add_member()
            .withf(move |s, u, role| {
                *s == workspace && *u == user_id && *role == ServerMemberRole::Member
            })
            .times(1)
            .returning(|server_id, user_id, member_role| {
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: Utc::now(),
                })
            });
        let mut conversations = MockConversationRepository::new();
        conversations
            .expect_list_public_channel_ids()
            .withf(move |s| *s == workspace)
            .returning(move |_| Box::pin(async move { Ok(vec![lobby]) }));
        let mut conversation_members = MockConversationMemberRepository::new();
        conversation_members
            .expect_is_member()
            .returning(|_, _| Box::pin(async { Ok(false) }));
        conversation_members
            .expect_add_member()
            .withf(move |c, u| *c == lobby && *u == user_id)
            .times(1)
            .returning(|conversation_id, user_id| {
                Box::pin(async move {
                    Ok(ConversationMember {
                        id: ConversationMemberId::new(),
                        user_id,
                        conversation_id,
                        joined_at: Utc::now(),
                        pending_since: None,
                    })
                })
            });

        let r = repos(
            servers_with(workspace),
            server_members,
            not_banned(),
            conversations,
            conversation_members,
        );
        assert_eq!(
            join(&r, &DefaultWorkspace::Fixed(workspace), user_id)
                .await
                .unwrap(),
            Some(workspace)
        );
    }

    #[tokio::test]
    async fn join_without_a_default_workspace_touches_nothing() {
        let mut server_members = MockServerMemberRepository::new();
        server_members.expect_is_member().never();
        server_members.expect_add_member().never();
        let r = repos(
            MockServerRepository::new(),
            server_members,
            MockModerationRepository::new(),
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
        );
        assert_eq!(
            join(&r, &DefaultWorkspace::None, UserId::new())
                .await
                .unwrap(),
            None
        );
    }

    /// An account already in the workspace keeps its membership and role.
    #[tokio::test]
    async fn join_skips_an_account_that_is_already_a_member() {
        let workspace = ServerId::new();
        let r = repos(
            servers_with(workspace),
            members(true, 0),
            MockModerationRepository::new(),
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
        );
        assert_eq!(
            join(&r, &DefaultWorkspace::Fixed(workspace), UserId::new())
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn join_skips_an_account_banned_from_the_workspace() {
        let workspace = ServerId::new();
        let mut moderation = MockModerationRepository::new();
        moderation
            .expect_get_active_ban()
            .returning(move |server_id, user_id, _| {
                Box::pin(async move {
                    Ok(Some(ServerBan {
                        server_id,
                        user_id,
                        reason: None,
                        banned_by: None,
                        created_at: Utc::now(),
                        expires_at: None,
                    }))
                })
            });
        let r = repos(
            servers_with(workspace),
            members(false, 0),
            moderation,
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
        );
        assert_eq!(
            join(&r, &DefaultWorkspace::Fixed(workspace), UserId::new())
                .await
                .unwrap(),
            None
        );
    }

    /// A workspace deleted after the start does not fail the signup.
    #[tokio::test]
    async fn join_skips_a_workspace_deleted_since_the_start() {
        let r = repos(
            servers_with(ServerId::new()),
            members(false, 0),
            not_banned(),
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
        );
        assert_eq!(
            join(&r, &DefaultWorkspace::Fixed(ServerId::new()), UserId::new())
                .await
                .unwrap(),
            None
        );
    }

    /// A failed insert fails the signup, so its transaction rolls back.
    #[tokio::test]
    async fn join_propagates_a_failed_insert() {
        let workspace = ServerId::new();
        let mut server_members = MockServerMemberRepository::new();
        server_members
            .expect_is_member()
            .returning(|_, _| Ok(false));
        server_members
            .expect_add_member()
            .returning(|_, _, _| Err(RepoError::new(RepoErrorKind::Internal)));
        let r = repos(
            servers_with(workspace),
            server_members,
            not_banned(),
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
        );
        assert!(
            join(&r, &DefaultWorkspace::Fixed(workspace), UserId::new())
                .await
                .is_err()
        );
    }

    #[derive(Default)]
    struct Recording(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for Recording {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[test]
    fn announce_emits_member_joined_only_after_a_join() {
        let seen = Arc::new(Recording::default());
        let observers = Observers::new(Arc::clone(&seen) as Arc<dyn Broadcaster>);
        let user_id = UserId::new();
        announce(None, user_id, "alice", &observers);
        assert!(seen.0.lock().unwrap().is_empty());
        let workspace = ServerId::new();
        announce(Some(workspace), user_id, "alice", &observers);
        let seen = seen.0.lock().unwrap();
        assert!(matches!(
            seen.as_slice(),
            [WorkspaceEvent::MemberJoined { workspace_id, user_id: u, username, source: None }]
                if *workspace_id == workspace && *u == user_id && username == "alice"
        ));
    }
}
