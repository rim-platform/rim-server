//! GraphQL type describing the running server build and its features.

use std::sync::Arc;

use async_graphql::*;
use rim_core::server_feature::model::{FeatureState, ServerFeature};
use rim_services::auth::service::AuthService;
use rim_services::dm_policy::dto::DmOperatorPolicy;
use rim_services::feature::service::FeatureGate;
use rim_services::limits::InstanceLimits;

/// The major version of this GraphQL API. Raised when a field or argument is removed or
/// changes meaning, so a client can tell it is older than the server it talks to.
pub const API_VERSION: i32 = 1;

/// The running server build, the API version it speaks and the optional features it runs.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct ServerInfo {
    /// The server's release version, e.g. `0.1.0`.
    pub version: String,
    /// The major version of this GraphQL API.
    pub api_version: i32,
    /// Every optional feature with its state on this instance.
    pub features: Vec<ServerFeatureInfo>,
    /// Days a command audit row is kept before it is exported and deleted; null keeps rows
    /// forever.
    pub audit_retention_days: Option<u32>,
    /// A short text the operator shows people on this instance, at most 500 characters; null
    /// when there is none.
    pub instance_notice: Option<String>,
    /// The terms of service a signup accepts, an absolute http(s) URL; null when the instance
    /// has none. A signup sends it back as `acceptedTermsUrl`.
    pub terms_url: Option<String>,
    /// The instance-wide caps, so a client can show "5 of 5" before it tries.
    pub limits: ServerLimits,
    /// Whether new accounts sign up with a key only: `signupWithInvite` is refused with
    /// `FORBIDDEN` and `signupWithKey` is the signup. Password sign-in of existing accounts
    /// works either way.
    pub signup_requires_keys: bool,
    /// DMs from people on other servers, and from people here outside the recipient's
    /// workspaces (`RIM_DM_INBOUND`): a floor for every account here.
    pub dm_inbound: crate::types::DmInbound,
    /// DMs people here start with people on other servers (`RIM_DM_OUTBOUND`).
    pub dm_outbound: crate::types::DmOutbound,
    /// The server ids the `PARTNERS` settings name (`RIM_DM_PARTNERS`).
    pub dm_partners: Vec<String>,
    /// The workspace every new account joins as a Member, so a client can open it after a
    /// signup: the one `RIM_DEFAULT_WORKSPACE` names, or the one an owner claimed from the
    /// definition file; null when there is none.
    pub default_workspace: Option<ID>,
    /// Where to report abuse on this server; served after `registrationFull`.
    #[graphql(skip)]
    pub abuse_contact: Option<String>,
    /// The age every signup confirms; served after `registrationFull`.
    #[graphql(skip)]
    pub minimum_age: Option<i32>,
}

/// The instance-wide caps; each is null when this instance sets none, but devices and DM
/// requests always have one. Creating one more past a cap answers `VALIDATION` naming it.
#[derive(SimpleObject, Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerLimits {
    /// Workspaces one user owns.
    pub workspaces_per_user: Option<u32>,
    /// Members of one workspace; the direct space has no cap.
    pub members_per_workspace: Option<u32>,
    /// Channels of one workspace, public and private.
    pub channels_per_workspace: Option<u32>,
    /// Devices on one account: the stricter of the instance cap and the relay quota.
    pub devices_per_user: Option<u32>,
    /// DM requests one user sends to strangers per day: the stricter of the instance cap and
    /// the relay quota.
    pub dm_requests_per_day: Option<u32>,
    /// Macros of one workspace.
    pub macros_per_workspace: Option<u32>,
    /// Pipelines of one workspace.
    pub pipelines_per_workspace: Option<u32>,
    /// Plugins of one workspace's registry.
    pub plugins_per_workspace: Option<u32>,
    /// Invites one user creates per day.
    pub invites_per_user_per_day: Option<u32>,
    /// Uses one invite may have.
    pub invite_max_uses: Option<u32>,
    /// Bytes of one message's text.
    pub message_bytes: Option<u32>,
    /// Hours after its creation during which an account cannot create a workspace or send a
    /// DM request to a stranger, and sends messages at a lower rate; `me.probationEndsAt` says
    /// when the caller's ends.
    pub probation_hours: Option<u32>,
    /// Pending invitations to encrypted conversations one user holds; past it, adding them to
    /// one more answers `VALIDATION`.
    pub pending_invites_per_user: Option<u32>,
    /// Local accounts the instance holds, those waiting out a deletion grace period included;
    /// at it every signup answers `REGISTRATION_FULL` and `serverInfo.registrationFull` is true.
    pub accounts: Option<u32>,
}

impl From<&InstanceLimits> for ServerLimits {
    fn from(limits: &InstanceLimits) -> Self {
        Self {
            workspaces_per_user: limits.workspaces_per_user,
            members_per_workspace: limits.members_per_workspace,
            channels_per_workspace: limits.channels_per_workspace,
            devices_per_user: limits.devices_per_user,
            dm_requests_per_day: limits.dm_requests_per_day,
            macros_per_workspace: limits.macros_per_workspace,
            pipelines_per_workspace: limits.pipelines_per_workspace,
            plugins_per_workspace: limits.plugins_per_workspace,
            invites_per_user_per_day: limits.invites_per_user_per_day,
            invite_max_uses: limits.invite_max_uses,
            message_bytes: limits.message_bytes,
            probation_hours: limits.probation_hours,
            pending_invites_per_user: limits.pending_invites_per_user,
            accounts: limits.accounts,
        }
    }
}

/// The configured command audit retention in days, held in the schema data; `None` keeps rows
/// forever.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuditRetention(pub Option<u32>);

/// What the operator publishes about the instance, held in the schema data: the notice shown
/// to people, the terms URL every signup accepts, whether signups need a key, the workspace
/// every new account joins, the abuse contact and the minimum age signups confirm.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstancePolicy {
    /// The instance notice; `None` when there is none.
    pub notice: Option<String>,
    /// The terms of service URL; `None` when signups accept no terms.
    pub terms_url: Option<String>,
    /// Whether password signups are refused.
    pub signup_requires_keys: bool,
    /// The workspace every new account joins as a Member, when configured by id; `None` leaves
    /// `serverInfo` to the workspace an owner claimed from the definition file, if any.
    pub default_workspace: Option<rim_core::server::model::ServerId>,
    /// Where to report abuse; `None` when the operator names none.
    pub abuse_contact: Option<String>,
    /// The age every signup confirms; `None` when signups confirm none.
    pub minimum_age: Option<u8>,
}

#[ComplexObject]
impl ServerInfo {
    /// Whether the instance holds as many local accounts as `limits.accounts` allows, so every
    /// signup answers `REGISTRATION_FULL`; false when there is no cap. The answer may be up to half
    /// a minute old. A client says the server is full before anyone tries, and points
    /// to another server or to running one.
    async fn registration_full(&self, ctx: &Context<'_>) -> bool {
        let Ok(auth) = ctx.data::<Arc<dyn AuthService>>() else {
            return false;
        };
        auth.registration_full().await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not tell whether registration is full");
            false
        })
    }

    /// Where to report abuse on this server: an email address or an https URL; null when the
    /// operator names none (`RIM_ABUSE_CONTACT`).
    async fn abuse_contact(&self) -> Option<&str> {
        self.abuse_contact.as_deref()
    }

    /// The age every signup confirms the person has reached, self-declared; null when the
    /// server asks for none (`RIM_MINIMUM_AGE`). A signup sends it back as
    /// `confirmedMinimumAge`.
    async fn minimum_age(&self) -> Option<i32> {
        self.minimum_age
    }
}

impl ServerInfo {
    /// This build's version and API version, with the features `gate` reports, the audit
    /// `retention`, the instance `policy`, its `limits` and its DM settings.
    pub fn current(
        gate: &dyn FeatureGate,
        retention: AuditRetention,
        policy: &InstancePolicy,
        limits: &InstanceLimits,
        dm_policy: &DmOperatorPolicy,
    ) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            api_version: API_VERSION,
            features: gate
                .states()
                .into_iter()
                .map(|(feature, state)| ServerFeatureInfo::new(feature, &state))
                .collect(),
            audit_retention_days: retention.0,
            instance_notice: policy.notice.clone(),
            terms_url: policy.terms_url.clone(),
            limits: limits.into(),
            signup_requires_keys: policy.signup_requires_keys,
            dm_inbound: dm_policy.inbound.into(),
            dm_outbound: dm_policy.outbound.into(),
            dm_partners: dm_policy.partners.clone(),
            default_workspace: policy
                .default_workspace
                .map(|workspace| ID(workspace.to_string())),
            abuse_contact: policy.abuse_contact.clone(),
            minimum_age: policy.minimum_age.map(i32::from),
        }
    }
}

/// One optional feature and whether it works here.
#[derive(SimpleObject, Clone, Debug)]
pub struct ServerFeatureInfo {
    /// The feature's name, as `RIM_FEATURES_DISABLED` takes it: `attachments`, `link_previews`,
    /// `email`, `custom_emoji`, `workspace_plugins` or `ai_commands`. A client ignores a name
    /// it does not know.
    pub key: String,
    pub state: ServerFeatureState,
    /// Why an unavailable feature is unavailable; null otherwise.
    pub reason: Option<String>,
}

impl ServerFeatureInfo {
    fn new(feature: ServerFeature, state: &FeatureState) -> Self {
        Self {
            key: feature.as_str().to_string(),
            state: state.into(),
            reason: state.reason().map(str::to_string),
        }
    }
}

/// Whether an optional feature works on this instance.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ServerFeatureState {
    /// The feature works.
    Enabled,
    /// The operator switched it off.
    Disabled,
    /// The configuration lacks something the feature needs; `reason` says what.
    Unavailable,
}

impl From<&FeatureState> for ServerFeatureState {
    fn from(state: &FeatureState) -> Self {
        match state {
            FeatureState::Enabled => Self::Enabled,
            FeatureState::Disabled => Self::Disabled,
            FeatureState::Unavailable { .. } => Self::Unavailable,
        }
    }
}
