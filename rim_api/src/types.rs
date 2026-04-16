use async_graphql::{ComplexObject, Context, Enum, InputObject, Object, Result, SimpleObject};

use rim_core::repository::pagination::Page;

use crate::resolve::ResolveExt;

// ── GraphQL output enums (stable wire contract) ──────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandSourceKind {
    BuiltIn,
    Plugin,
    Integration,
    User,
}

impl From<rim_services::command::dto::CommandSourceKindDto> for CommandSourceKind {
    fn from(value: rim_services::command::dto::CommandSourceKindDto) -> Self {
        use rim_services::command::dto::CommandSourceKindDto as D;
        match value {
            D::BuiltIn => Self::BuiltIn,
            D::Plugin => Self::Plugin,
            D::Integration => Self::Integration,
            D::User => Self::User,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandExecutionTarget {
    Backend,
    DesktopLocal,
}

impl From<rim_services::command::dto::CommandExecutionTargetDto> for CommandExecutionTarget {
    fn from(value: rim_services::command::dto::CommandExecutionTargetDto) -> Self {
        use rim_services::command::dto::CommandExecutionTargetDto as D;
        match value {
            D::Backend => Self::Backend,
            D::DesktopLocal => Self::DesktopLocal,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandResultStatus {
    Success,
    Error,
}

impl From<rim_services::command::dto::CommandResultStatusDto> for CommandResultStatus {
    fn from(value: rim_services::command::dto::CommandResultStatusDto) -> Self {
        use rim_services::command::dto::CommandResultStatusDto as D;
        match value {
            D::Success => Self::Success,
            D::Error => Self::Error,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IntegrationKind {
    BuiltIn,
    LocalPlugin,
    Bot,
    Webhook,
}

impl From<rim_services::integration::dto::IntegrationKindDto> for IntegrationKind {
    fn from(value: rim_services::integration::dto::IntegrationKindDto) -> Self {
        use rim_services::integration::dto::IntegrationKindDto as D;
        match value {
            D::BuiltIn => Self::BuiltIn,
            D::LocalPlugin => Self::LocalPlugin,
            D::Bot => Self::Bot,
            D::Webhook => Self::Webhook,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SettingScope {
    User,
    Workspace,
    Integration,
    DeviceLocal,
}

impl From<rim_services::setting::dto::SettingScopeDto> for SettingScope {
    fn from(value: rim_services::setting::dto::SettingScopeDto) -> Self {
        use rim_services::setting::dto::SettingScopeDto as D;
        match value {
            D::User => Self::User,
            D::Workspace => Self::Workspace,
            D::Integration => Self::Integration,
            D::DeviceLocal => Self::DeviceLocal,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ConversationTypeEnum {
    Direct,
    Group,
    Channel,
}

impl From<rim_core::conversation::model::ConversationType> for ConversationTypeEnum {
    fn from(value: rim_core::conversation::model::ConversationType) -> Self {
        use rim_core::conversation::model::ConversationType as D;
        match value {
            D::Direct => Self::Direct,
            D::Group => Self::Group,
            D::Channel => Self::Channel,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum AttachmentStatusEnum {
    PendingUpload,
    Uploaded,
    Ready,
    Failed,
}

impl From<rim_services::message_attachment::dto::AttachmentStatus> for AttachmentStatusEnum {
    fn from(value: rim_services::message_attachment::dto::AttachmentStatus) -> Self {
        use rim_services::message_attachment::dto::AttachmentStatus as D;
        match value {
            D::PendingUpload => Self::PendingUpload,
            D::Uploaded => Self::Uploaded,
            D::Ready => Self::Ready,
            D::Failed => Self::Failed,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum KeybindSource {
    Default,
    User,
    Workspace,
}

impl From<rim_services::keybind::dto::KeybindSourceDto> for KeybindSource {
    fn from(value: rim_services::keybind::dto::KeybindSourceDto) -> Self {
        use rim_services::keybind::dto::KeybindSourceDto as D;
        match value {
            D::Default => Self::Default,
            D::User => Self::User,
            D::Workspace => Self::Workspace,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct User {
    pub id: async_graphql::ID,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
}

impl From<rim_services::user::dto::UserDto> for User {
    fn from(value: rim_services::user::dto::UserDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            username: value.username,
            display_name: value.display_name,
            avatar_url: value.avatar_url,
            status_text: value.status_text,
            bio: value.bio,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Server {
    pub id: async_graphql::ID,
    pub name: String,
}

impl From<rim_services::server::dto::ServerDto> for Server {
    fn from(value: rim_services::server::dto::ServerDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            name: value.name,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ServerMember {
    pub server_id: async_graphql::ID,
    pub user_id: async_graphql::ID,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
    pub member_role: String,
    pub joined_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::server_member::dto::ServerMemberDto> for ServerMember {
    fn from(value: rim_services::server_member::dto::ServerMemberDto) -> Self {
        Self {
            server_id: async_graphql::ID(value.server_id.to_string()),
            user_id: async_graphql::ID(value.user_id.to_string()),
            username: value.username,
            display_name: value.display_name,
            avatar_url: value.avatar_url,
            status_text: value.status_text,
            bio: value.bio,
            member_role: value.member_role,
            joined_at: value.joined_at,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Command {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub source_kind: CommandSourceKind,
    pub execution_target: CommandExecutionTarget,
    pub required_context: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub visible_in_palette: bool,
    pub visible_in_context_menus: bool,
    pub keybind_capable: bool,
    pub dangerous: bool,
}

impl From<rim_services::command::dto::CommandDefinitionDto> for Command {
    fn from(value: rim_services::command::dto::CommandDefinitionDto) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
            category: value.category,
            source_kind: value.source_kind.into(),
            execution_target: value.execution_target.into(),
            required_context: value.required_context,
            required_capabilities: value.required_capabilities,
            visible_in_palette: value.visible_in_palette,
            visible_in_context_menus: value.visible_in_context_menus,
            keybind_capable: value.keybind_capable,
            dangerous: value.dangerous,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandSurface {
    Palette,
    ContextMenu,
    KeybindPicker,
}

impl From<CommandSurface> for rim_services::command::dto::CommandSurfaceDto {
    fn from(value: CommandSurface) -> Self {
        match value {
            CommandSurface::Palette => Self::Palette,
            CommandSurface::ContextMenu => Self::ContextMenu,
            CommandSurface::KeybindPicker => Self::KeybindPicker,
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct CommandContextInput {
    pub workspace_id: Option<async_graphql::ID>,
    pub conversation_id: Option<async_graphql::ID>,
    pub thread_id: Option<async_graphql::ID>,
    pub selected_text: Option<String>,
    pub selected_message_id: Option<async_graphql::ID>,
    pub selected_message_ids: Option<Vec<async_graphql::ID>>,
    pub focused_surface: Option<String>,
    pub active_panel: Option<String>,
}

impl TryFrom<CommandContextInput> for rim_services::command::dto::CommandContextDto {
    type Error = async_graphql::Error;

    fn try_from(value: CommandContextInput) -> Result<Self, Self::Error> {
        Ok(Self {
            workspace_id: value
                .workspace_id
                .as_ref()
                .map(crate::ids::parse_uuid_id::<rim_core::server::model::ServerId>)
                .transpose()?,
            conversation_id: value
                .conversation_id
                .as_ref()
                .map(crate::ids::parse_uuid_id::<rim_core::conversation::model::ConversationId>)
                .transpose()?,
            thread_id: value
                .thread_id
                .as_ref()
                .map(crate::ids::parse_uuid_id::<rim_core::message_thread::model::MessageThreadId>)
                .transpose()?,
            selected_text: value.selected_text,
            selected_message_id: value
                .selected_message_id
                .as_ref()
                .map(crate::ids::parse_uuid_id::<rim_core::message::model::MessageId>)
                .transpose()?,
            selected_message_ids: value
                .selected_message_ids
                .unwrap_or_default()
                .into_iter()
                .map(|id| crate::ids::parse_uuid_id::<rim_core::message::model::MessageId>(&id))
                .collect::<Result<Vec<_>, _>>()?,
            focused_surface: value.focused_surface,
            active_panel: value.active_panel,
        })
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipeStageResult {
    pub stage_index: u32,
    pub command_id: String,
    pub status: CommandResultStatus,
    pub message: String,
    pub payload: Option<async_graphql::Json<serde_json::Value>>,
}

impl From<rim_services::command::dto::PipeStageResultDto> for PipeStageResult {
    fn from(value: rim_services::command::dto::PipeStageResultDto) -> Self {
        Self {
            stage_index: value.stage_index,
            command_id: value.command_id,
            status: value.status.into(),
            message: value.message,
            payload: value.payload.map(async_graphql::Json),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct CommandResult {
    pub status: CommandResultStatus,
    pub message: String,
    pub payload: Option<async_graphql::Json<serde_json::Value>>,
    pub follow_up_commands: Vec<String>,
    pub pipeline_stages: Option<Vec<PipeStageResult>>,
}

impl From<rim_services::command::dto::CommandResultDto> for CommandResult {
    fn from(value: rim_services::command::dto::CommandResultDto) -> Self {
        Self {
            status: value.status.into(),
            message: value.message,
            payload: value.payload.map(async_graphql::Json),
            follow_up_commands: value.follow_up_commands,
            pipeline_stages: value
                .pipeline_stages
                .map(|stages| stages.into_iter().map(Into::into).collect()),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Action {
    pub id: String,
    pub title: String,
    pub description: String,
    pub capability_ids: Vec<String>,
}

impl From<rim_services::action::dto::ActionDto> for Action {
    fn from(value: rim_services::action::dto::ActionDto) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
            capability_ids: value.capability_ids,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Event {
    pub id: String,
    pub title: String,
    pub description: String,
}

impl From<rim_services::event::dto::EventDto> for Event {
    fn from(value: rim_services::event::dto::EventDto) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Capability {
    pub id: String,
    pub title: String,
    pub description: String,
}

impl From<rim_services::capability::dto::CapabilityDto> for Capability {
    fn from(value: rim_services::capability::dto::CapabilityDto) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Integration {
    pub id: String,
    pub kind: IntegrationKind,
    pub title: String,
    pub enabled: bool,
    pub principal_id: Option<async_graphql::ID>,
}

impl From<rim_services::integration::dto::IntegrationDto> for Integration {
    fn from(value: rim_services::integration::dto::IntegrationDto) -> Self {
        Self {
            id: value.id,
            kind: value.kind.into(),
            title: value.title,
            enabled: value.enabled,
            principal_id: value
                .principal_id
                .map(|id| async_graphql::ID(id.to_string())),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Setting {
    pub id: String,
    pub scope: SettingScope,
    pub title: String,
    pub description: String,
    pub value_type: String,
    pub is_secret: bool,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Keybind {
    pub id: async_graphql::ID,
    pub source: KeybindSource,
    pub scope: KeybindScope,
    pub workspace_id: Option<async_graphql::ID>,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<async_graphql::Json<serde_json::Value>>,
    pub is_enabled: bool,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct DefaultKeybind {
    pub source: KeybindSource,
    pub keystroke: String,
    pub command_id: String,
    pub context: KeybindContext,
    pub preset_args: Option<async_graphql::Json<serde_json::Value>>,
}

impl From<rim_services::keybind::dto::DefaultKeybindDto> for DefaultKeybind {
    fn from(value: rim_services::keybind::dto::DefaultKeybindDto) -> Self {
        Self {
            source: value.source.into(),
            keystroke: value.keystroke,
            command_id: value.command_id,
            context: value.context.into(),
            preset_args: value.preset_args.map(async_graphql::Json),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct KeybindLayers {
    pub default_keybind: Option<DefaultKeybind>,
    pub user_keybind: Option<Keybind>,
    pub workspace_keybind: Option<Keybind>,
    pub resolved_persisted_keybind: Option<Keybind>,
}

impl From<rim_services::keybind::dto::KeybindLayersDto> for KeybindLayers {
    fn from(value: rim_services::keybind::dto::KeybindLayersDto) -> Self {
        Self {
            default_keybind: value.default_keybind.map(Into::into),
            user_keybind: value.user_keybind.map(Into::into),
            workspace_keybind: value.workspace_keybind.map(Into::into),
            resolved_persisted_keybind: value.resolved_persisted_keybind.map(Into::into),
        }
    }
}

impl From<rim_services::keybind::dto::KeybindDto> for Keybind {
    fn from(value: rim_services::keybind::dto::KeybindDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            source: value.source.into(),
            scope: value.scope.into(),
            workspace_id: value
                .workspace_id
                .map(|id| async_graphql::ID(id.to_string())),
            keystroke: value.keystroke,
            command_id: value.command_id,
            context: value.context.into(),
            preset_args: value.preset_args.map(async_graphql::Json),
            is_enabled: value.is_enabled,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum KeybindScope {
    User,
    Workspace,
}

impl From<KeybindScope> for rim_services::keybind::dto::KeybindScopeDto {
    fn from(value: KeybindScope) -> Self {
        match value {
            KeybindScope::User => Self::User,
            KeybindScope::Workspace => Self::Workspace,
        }
    }
}

impl From<rim_services::keybind::dto::KeybindScopeDto> for KeybindScope {
    fn from(value: rim_services::keybind::dto::KeybindScopeDto) -> Self {
        use rim_services::keybind::dto::KeybindScopeDto as D;
        match value {
            D::User => Self::User,
            D::Workspace => Self::Workspace,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum KeybindContext {
    Global,
    Composer,
    MessageList,
    ThreadView,
    ConversationView,
    Panel,
    CommandPalette,
}

impl From<KeybindContext> for rim_services::keybind::dto::KeybindContextDto {
    fn from(value: KeybindContext) -> Self {
        match value {
            KeybindContext::Global => Self::Global,
            KeybindContext::Composer => Self::Composer,
            KeybindContext::MessageList => Self::MessageList,
            KeybindContext::ThreadView => Self::ThreadView,
            KeybindContext::ConversationView => Self::ConversationView,
            KeybindContext::Panel => Self::Panel,
            KeybindContext::CommandPalette => Self::CommandPalette,
        }
    }
}

impl From<rim_services::keybind::dto::KeybindContextDto> for KeybindContext {
    fn from(value: rim_services::keybind::dto::KeybindContextDto) -> Self {
        use rim_services::keybind::dto::KeybindContextDto as D;
        match value {
            D::Global => Self::Global,
            D::Composer => Self::Composer,
            D::MessageList => Self::MessageList,
            D::ThreadView => Self::ThreadView,
            D::ConversationView => Self::ConversationView,
            D::Panel => Self::Panel,
            D::CommandPalette => Self::CommandPalette,
        }
    }
}

impl From<rim_services::setting::dto::SettingDto> for Setting {
    fn from(value: rim_services::setting::dto::SettingDto) -> Self {
        Self {
            id: value.id,
            scope: value.scope.into(),
            title: value.title,
            description: value.description,
            value_type: value.value_type,
            is_secret: value.is_secret,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct Conversation {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_type: ConversationTypeEnum,
    pub title: Option<String>,
    pub topic: Option<String>,
    pub member_ids: Vec<async_graphql::ID>,
}

#[ComplexObject]
impl Conversation {
    async fn is_muted(&self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<bool> {
        let actor = crate::auth_context::require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::notification_preference::service::NotificationPreferenceService,
        >>()?;
        let conversation_id =
            crate::ids::parse_uuid_id::<rim_core::conversation::model::ConversationId>(&self.id)?;

        svc.is_muted(
            rim_services::notification_preference::command::IsMutedQuery {
                actor: rim_services::Actor::new(actor),
                conversation_id,
            },
        )
        .await
        .map_err(crate::errors::map_service_error)
    }
}

impl From<rim_services::conversation::dto::ConversationDto> for Conversation {
    fn from(value: rim_services::conversation::dto::ConversationDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_type: value.conversation_type.into(),
            title: value.title,
            topic: value.topic,
            member_ids: value
                .member_ids
                .into_iter()
                .map(|id| async_graphql::ID(id.to_string()))
                .collect(),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct SearchResultMessage {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub conversation_title: Option<String>,
    pub conversation_type: ConversationTypeEnum,
    pub thread_id: Option<async_graphql::ID>,
    pub root_message_id: Option<async_graphql::ID>,
    pub author_id: async_graphql::ID,
    pub text: Option<String>,
    pub sent_at: chrono::DateTime<chrono::Utc>,
}

#[ComplexObject]
impl SearchResultMessage {
    #[graphql(complexity = 5)]
    async fn author(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        ctx.load_user(&self.author_id).await
    }
}

#[derive(Clone, Debug)]
pub struct Message {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub thread_id: Option<async_graphql::ID>,
    pub root_thread_id: Option<async_graphql::ID>,
    pub author_id: async_graphql::ID,
    pub text: Option<String>,
    pub has_attachments: bool,
    pub attachments: Vec<Attachment>,
    pub parent_message_id: Option<async_graphql::ID>,
    pub forwarded_from_message_id: Option<async_graphql::ID>,
    pub is_deleted: bool,
    pub sent_at: chrono::DateTime<chrono::Utc>,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MessageReference {
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub message_id: async_graphql::ID,
    pub thread_id: Option<async_graphql::ID>,
    pub permalink_path: String,
    pub anchor: String,
}

impl From<rim_services::message::dto::MessageReferenceDto> for MessageReference {
    fn from(value: rim_services::message::dto::MessageReferenceDto) -> Self {
        Self {
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            message_id: async_graphql::ID(value.message_id.to_string()),
            thread_id: value.thread_id.map(|id| async_graphql::ID(id.to_string())),
            permalink_path: value.permalink_path,
            anchor: value.anchor,
        }
    }
}

impl From<rim_services::message::dto::MessageDto> for Message {
    fn from(value: rim_services::message::dto::MessageDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            thread_id: value.thread_id.map(|id| async_graphql::ID(id.to_string())),
            root_thread_id: value
                .root_thread_id
                .map(|id| async_graphql::ID(id.to_string())),
            author_id: async_graphql::ID(value.author_id.to_string()),
            text: if value.is_deleted { None } else { value.text },
            has_attachments: value.has_attachments,
            attachments: value.attachments.into_iter().map(Into::into).collect(),
            parent_message_id: value
                .parent_message_id
                .map(|id| async_graphql::ID(id.to_string())),
            forwarded_from_message_id: value
                .forwarded_from_message_id
                .map(|id| async_graphql::ID(id.to_string())),
            is_deleted: value.is_deleted,
            sent_at: value.sent_at,
        }
    }
}

#[Object]
impl Message {
    async fn id(&self) -> &async_graphql::ID {
        &self.id
    }

    async fn server_id(&self) -> &async_graphql::ID {
        &self.server_id
    }

    async fn conversation_id(&self) -> &async_graphql::ID {
        &self.conversation_id
    }

    async fn thread_id(&self) -> &Option<async_graphql::ID> {
        &self.thread_id
    }

    async fn root_thread_id(&self) -> &Option<async_graphql::ID> {
        &self.root_thread_id
    }

    async fn author_id(&self) -> &async_graphql::ID {
        &self.author_id
    }

    async fn text(&self) -> &Option<String> {
        &self.text
    }

    async fn has_attachments(&self) -> bool {
        self.has_attachments
    }

    async fn attachments(&self) -> &Vec<Attachment> {
        &self.attachments
    }

    async fn parent_message_id(&self) -> &Option<async_graphql::ID> {
        &self.parent_message_id
    }

    async fn forwarded_from_message_id(&self) -> &Option<async_graphql::ID> {
        &self.forwarded_from_message_id
    }

    #[graphql(complexity = 5)]
    async fn forwarded_from(&self, ctx: &Context<'_>) -> Result<Option<Message>> {
        let Some(ref fwd_id) = self.forwarded_from_message_id else {
            return Ok(None);
        };
        let msg_svc =
            ctx.data::<std::sync::Arc<dyn rim_services::message::service::MessageService>>()?;
        let message_id = crate::ids::parse_uuid_id::<rim_core::message::model::MessageId>(fwd_id)?;
        let messages = msg_svc
            .get_messages_by_ids(&[message_id])
            .await
            .map_err(crate::errors::map_service_error)?;
        Ok(messages.into_iter().next().map(Into::into))
    }

    async fn is_deleted(&self) -> bool {
        self.is_deleted
    }

    async fn sent_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.sent_at
    }

    #[graphql(complexity = 5)]
    async fn author(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        ctx.load_user(&self.author_id).await
    }

    #[graphql(complexity = "child_complexity * 10")]
    async fn reactions(&self, ctx: &Context<'_>) -> Result<Vec<ReactionGroup>> {
        ctx.load_reactions(&self.id).await
    }

    #[graphql(complexity = 5)]
    async fn is_pinned(&self, ctx: &Context<'_>) -> Result<bool> {
        ctx.load_is_pinned(&self.id).await
    }

    #[graphql(complexity = 5)]
    async fn mentioned_user_ids(&self, ctx: &Context<'_>) -> Result<Vec<async_graphql::ID>> {
        let user_ids = ctx.load_mentioned_user_ids(&self.id).await?;
        Ok(user_ids
            .into_iter()
            .map(|id| async_graphql::ID(id.to_string()))
            .collect())
    }

    #[graphql(complexity = 5)]
    async fn link_previews(&self, ctx: &Context<'_>) -> Result<Vec<LinkPreview>> {
        ctx.load_link_previews(&self.id).await
    }

    #[graphql(complexity = "child_complexity * 10")]
    async fn mentions(&self, ctx: &Context<'_>) -> Result<Vec<User>> {
        ctx.load_mentions(&self.id).await
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Thread {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub root_message_id: Option<async_graphql::ID>,
    pub root_author_id: Option<async_graphql::ID>,
    pub root_text: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_message_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::message_thread::dto::ThreadDto> for Thread {
    fn from(value: rim_services::message_thread::dto::ThreadDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            root_message_id: value
                .root_message_id
                .map(|id| async_graphql::ID(id.to_string())),
            root_author_id: value
                .root_author_id
                .map(|id| async_graphql::ID(id.to_string())),
            root_text: value.root_text,
            created_at: value.created_at,
            last_message_at: value.last_message_at,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Attachment {
    pub id: async_graphql::ID,
    pub status: AttachmentStatusEnum,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
}

impl From<rim_services::message_attachment::dto::AttachmentDto> for Attachment {
    fn from(value: rim_services::message_attachment::dto::AttachmentDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            status: value.status.into(),
            filename: value.filename,
            content_type: value.content_type,
            size_bytes: value.size_bytes,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct UploadTicket {
    pub attachment_id: async_graphql::ID,
    pub upload_token: String,
    pub max_size_bytes: Option<u64>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<rim_services::message_attachment::dto::UploadTicketDto> for UploadTicket {
    fn from(value: rim_services::message_attachment::dto::UploadTicketDto) -> Self {
        Self {
            attachment_id: async_graphql::ID(value.attachment_id.to_string()),
            upload_token: value.upload_token,
            max_size_bytes: value.max_size_bytes,
            expires_at: value.expires_at,
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct InboxCursorInput {
    pub last_activity_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: async_graphql::ID,
}

impl TryFrom<InboxCursorInput> for rim_services::conversation::dto::InboxPageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: InboxCursorInput) -> Result<Self, Self::Error> {
        Ok(Self {
            last_activity_at: value.last_activity_at,
            conversation_id: crate::ids::parse_uuid_id::<
                rim_core::conversation::model::ConversationId,
            >(&value.conversation_id)?,
        })
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct ChannelCursorInput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: async_graphql::ID,
}

impl TryFrom<ChannelCursorInput> for rim_services::conversation::dto::ChannelPageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: ChannelCursorInput) -> Result<Self, Self::Error> {
        Ok(Self {
            created_at: value.created_at,
            conversation_id: crate::ids::parse_uuid_id::<
                rim_core::conversation::model::ConversationId,
            >(&value.conversation_id)?,
        })
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct MessageCursorInput {
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub message_id: async_graphql::ID,
}

impl TryFrom<MessageCursorInput> for rim_services::message::dto::MessagePageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: MessageCursorInput) -> Result<Self, Self::Error> {
        Ok(Self {
            sent_at: value.sent_at,
            message_id: crate::ids::parse_uuid_id::<rim_core::message::model::MessageId>(
                &value.message_id,
            )?,
        })
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct ThreadCursorInput {
    pub last_message_at: chrono::DateTime<chrono::Utc>,
    pub thread_id: async_graphql::ID,
}

impl TryFrom<ThreadCursorInput> for rim_services::message_thread::dto::ThreadPageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: ThreadCursorInput) -> Result<Self, Self::Error> {
        Ok(Self {
            last_message_at: value.last_message_at,
            thread_id: crate::ids::parse_uuid_id::<rim_core::message_thread::model::MessageThreadId>(
                &value.thread_id,
            )?,
        })
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PageInfo {
    pub has_next: bool,
    pub has_prev: bool,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ConversationPage {
    pub items: Vec<Conversation>,
    pub next: Option<InboxCursorOutput>,
    pub prev: Option<InboxCursorOutput>,
    pub page_info: PageInfo,
}

impl ConversationPage {
    pub fn from_inbox_page(
        page: Page<
            rim_services::conversation::dto::ConversationDto,
            rim_services::conversation::dto::InboxPageCursor,
        >,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
            next: page.next.map(|c| InboxCursorOutput {
                last_activity_at: c.last_activity_at,
                conversation_id: async_graphql::ID(c.conversation_id.to_string()),
            }),
            prev: page.prev.map(|c| InboxCursorOutput {
                last_activity_at: c.last_activity_at,
                conversation_id: async_graphql::ID(c.conversation_id.to_string()),
            }),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ChannelPage {
    pub items: Vec<Conversation>,
    pub next: Option<ChannelCursorOutput>,
    pub prev: Option<ChannelCursorOutput>,
    pub page_info: PageInfo,
}

impl ChannelPage {
    pub fn from_channel_page(
        page: Page<
            rim_services::conversation::dto::ConversationDto,
            rim_services::conversation::dto::ChannelPageCursor,
        >,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
            next: page.next.map(|c| ChannelCursorOutput {
                created_at: c.created_at,
                conversation_id: async_graphql::ID(c.conversation_id.to_string()),
            }),
            prev: page.prev.map(|c| ChannelCursorOutput {
                created_at: c.created_at,
                conversation_id: async_graphql::ID(c.conversation_id.to_string()),
            }),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MessagePage {
    pub items: Vec<Message>,
    pub next: Option<MessageCursorOutput>,
    pub prev: Option<MessageCursorOutput>,
    pub page_info: PageInfo,
}

impl MessagePage {
    pub fn from_page(
        page: Page<
            rim_services::message::dto::MessageDto,
            rim_services::message::dto::MessagePageCursor,
        >,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        let items = page.items.into_iter().map(Into::into).collect();
        Self {
            items,
            next: page.next.map(|c| MessageCursorOutput {
                sent_at: c.sent_at,
                message_id: async_graphql::ID(c.message_id.to_string()),
            }),
            prev: page.prev.map(|c| MessageCursorOutput {
                sent_at: c.sent_at,
                message_id: async_graphql::ID(c.message_id.to_string()),
            }),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ThreadPage {
    pub items: Vec<Thread>,
    pub next: Option<ThreadCursorOutput>,
    pub prev: Option<ThreadCursorOutput>,
    pub page_info: PageInfo,
}

impl ThreadPage {
    pub fn from_page(
        page: Page<
            rim_services::message_thread::dto::ThreadDto,
            rim_services::message_thread::dto::ThreadPageCursor,
        >,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
            next: page.next.map(|c| ThreadCursorOutput {
                last_message_at: c.last_message_at,
                thread_id: async_graphql::ID(c.thread_id.to_string()),
            }),
            prev: page.prev.map(|c| ThreadCursorOutput {
                last_message_at: c.last_message_at,
                thread_id: async_graphql::ID(c.thread_id.to_string()),
            }),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct InboxCursorOutput {
    pub last_activity_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: async_graphql::ID,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ChannelCursorOutput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: async_graphql::ID,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MessageCursorOutput {
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub message_id: async_graphql::ID,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ReactionGroup {
    pub emoji: String,
    pub count: u32,
    pub reacted_by_me: bool,
    pub user_ids: Vec<async_graphql::ID>,
}

impl From<rim_services::message_reaction::dto::ReactionGroupDto> for ReactionGroup {
    fn from(value: rim_services::message_reaction::dto::ReactionGroupDto) -> Self {
        Self {
            emoji: value.emoji,
            count: value.count,
            reacted_by_me: value.reacted_by_me,
            user_ids: value
                .user_ids
                .into_iter()
                .map(|id| async_graphql::ID(id.to_string()))
                .collect(),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ThreadCursorOutput {
    pub last_message_at: chrono::DateTime<chrono::Utc>,
    pub thread_id: async_graphql::ID,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct UnreadCount {
    pub conversation_id: async_graphql::ID,
    pub count: i64,
}

impl From<rim_services::read_state::dto::UnreadCountDto> for UnreadCount {
    fn from(value: rim_services::read_state::dto::UnreadCountDto) -> Self {
        Self {
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            count: value.count,
        }
    }
}

// ── Presence ──────────────────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PresenceStatus {
    Online,
    Away,
    Offline,
}

impl From<PresenceStatus> for rim_services::presence::PresenceStatus {
    fn from(value: PresenceStatus) -> Self {
        match value {
            PresenceStatus::Online => Self::Online,
            PresenceStatus::Away => Self::Away,
            PresenceStatus::Offline => Self::Offline,
        }
    }
}

impl From<rim_services::presence::PresenceStatus> for PresenceStatus {
    fn from(value: rim_services::presence::PresenceStatus) -> Self {
        match value {
            rim_services::presence::PresenceStatus::Online => Self::Online,
            rim_services::presence::PresenceStatus::Away => Self::Away,
            rim_services::presence::PresenceStatus::Offline => Self::Offline,
        }
    }
}

#[derive(SimpleObject)]
pub struct UserPresenceGql {
    pub user_id: async_graphql::ID,
    pub status: PresenceStatus,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::presence::UserPresence> for UserPresenceGql {
    fn from(value: rim_services::presence::UserPresence) -> Self {
        Self {
            user_id: async_graphql::ID(value.user_id.to_string()),
            status: value.status.into(),
            last_seen_at: value.last_seen_at,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MutedConversation {
    pub conversation_id: async_graphql::ID,
    pub muted_until: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<rim_services::notification_preference::dto::MutedConversationDto> for MutedConversation {
    fn from(value: rim_services::notification_preference::dto::MutedConversationDto) -> Self {
        Self {
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            muted_until: value.muted_until,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Invite {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub created_by_user_id: async_graphql::ID,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<rim_services::auth::dto::InviteDto> for Invite {
    fn from(value: rim_services::auth::dto::InviteDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            created_by_user_id: async_graphql::ID(value.created_by_user_id.to_string()),
            expires_at: value.expires_at,
            max_uses: value.max_uses,
            use_count: value.use_count,
            revoked_at: value.revoked_at,
        }
    }
}

// ── Link Preview ──────────────────────────────────────────────────────────

#[derive(SimpleObject, Clone, Debug)]
pub struct LinkPreview {
    pub id: async_graphql::ID,
    pub message_id: async_graphql::ID,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub site_name: Option<String>,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::link_preview::dto::LinkPreviewDto> for LinkPreview {
    fn from(value: rim_services::link_preview::dto::LinkPreviewDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            message_id: async_graphql::ID(value.message_id.to_string()),
            url: value.url,
            title: value.title,
            description: value.description,
            image_url: value.image_url,
            site_name: value.site_name,
            fetched_at: value.fetched_at,
        }
    }
}

// ── Custom Emoji ──────────────────────────────────────────────────────────

#[derive(SimpleObject, Clone, Debug)]
pub struct CustomEmoji {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub name: String,
    pub image_url: String,
    pub uploaded_by: async_graphql::ID,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::custom_emoji::dto::CustomEmojiDto> for CustomEmoji {
    fn from(value: rim_services::custom_emoji::dto::CustomEmojiDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            name: value.name,
            image_url: value.image_url,
            uploaded_by: async_graphql::ID(value.uploaded_by.to_string()),
            created_at: value.created_at,
        }
    }
}

// ── Macro ────────────────────────────────────────────────────────────────

#[derive(SimpleObject, Clone, Debug)]
pub struct MacroStep {
    pub command_id: String,
    pub args: Option<async_graphql::Json<serde_json::Value>>,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MacroDef {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub user_id: async_graphql::ID,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<MacroStep>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::macro_def::dto::MacroDto> for MacroDef {
    fn from(value: rim_services::macro_def::dto::MacroDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            user_id: async_graphql::ID(value.user_id.to_string()),
            name: value.name,
            description: value.description,
            steps: value
                .steps
                .into_iter()
                .map(|s| MacroStep {
                    command_id: s.command_id,
                    args: s.args.map(async_graphql::Json),
                })
                .collect(),
            created_at: value.created_at,
        }
    }
}

// ── Skill Profile ────────────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SkillTier {
    Beginner,
    Intermediate,
    Advanced,
}

impl From<rim_core::skill_profile::model::SkillTier> for SkillTier {
    fn from(value: rim_core::skill_profile::model::SkillTier) -> Self {
        use rim_core::skill_profile::model::SkillTier as D;
        match value {
            D::Beginner => Self::Beginner,
            D::Intermediate => Self::Intermediate,
            D::Advanced => Self::Advanced,
        }
    }
}

impl From<SkillTier> for rim_core::skill_profile::model::SkillTier {
    fn from(value: SkillTier) -> Self {
        match value {
            SkillTier::Beginner => Self::Beginner,
            SkillTier::Intermediate => Self::Intermediate,
            SkillTier::Advanced => Self::Advanced,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct FeatureUsage {
    pub feature_id: String,
    pub first_used_at: chrono::DateTime<chrono::Utc>,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct SkillProfileGql {
    pub user_id: async_graphql::ID,
    pub features_used: Vec<FeatureUsage>,
    pub skill_tier: SkillTier,
}

impl From<rim_services::skill_profile::dto::SkillProfileDto> for SkillProfileGql {
    fn from(value: rim_services::skill_profile::dto::SkillProfileDto) -> Self {
        let mut features_used: Vec<FeatureUsage> = value
            .features_used
            .into_iter()
            .map(|(feature_id, first_used_at)| FeatureUsage {
                feature_id,
                first_used_at,
            })
            .collect();
        features_used.sort_by_key(|f| f.first_used_at);

        Self {
            user_id: async_graphql::ID(value.user_id.to_string()),
            features_used,
            skill_tier: value.skill_tier.into(),
        }
    }
}

// ── Channel Lens ────────────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FilterField {
    HasLinks,
    AuthorId,
    HasAttachments,
    TextContains,
    IsPinned,
}

impl From<rim_services::channel_lens::dto::FilterFieldDto> for FilterField {
    fn from(value: rim_services::channel_lens::dto::FilterFieldDto) -> Self {
        use rim_services::channel_lens::dto::FilterFieldDto as D;
        match value {
            D::HasLinks => Self::HasLinks,
            D::AuthorId => Self::AuthorId,
            D::HasAttachments => Self::HasAttachments,
            D::TextContains => Self::TextContains,
            D::IsPinned => Self::IsPinned,
        }
    }
}

impl From<FilterField> for rim_services::channel_lens::dto::FilterFieldDto {
    fn from(value: FilterField) -> Self {
        match value {
            FilterField::HasLinks => Self::HasLinks,
            FilterField::AuthorId => Self::AuthorId,
            FilterField::HasAttachments => Self::HasAttachments,
            FilterField::TextContains => Self::TextContains,
            FilterField::IsPinned => Self::IsPinned,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FilterOp {
    Eq,
    In,
    Contains,
}

impl From<rim_services::channel_lens::dto::FilterOpDto> for FilterOp {
    fn from(value: rim_services::channel_lens::dto::FilterOpDto) -> Self {
        use rim_services::channel_lens::dto::FilterOpDto as D;
        match value {
            D::Eq => Self::Eq,
            D::In => Self::In,
            D::Contains => Self::Contains,
        }
    }
}

impl From<FilterOp> for rim_services::channel_lens::dto::FilterOpDto {
    fn from(value: FilterOp) -> Self {
        match value {
            FilterOp::Eq => Self::Eq,
            FilterOp::In => Self::In,
            FilterOp::Contains => Self::Contains,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FilterLogic {
    And,
    Or,
}

impl From<rim_services::channel_lens::dto::FilterLogicDto> for FilterLogic {
    fn from(value: rim_services::channel_lens::dto::FilterLogicDto) -> Self {
        use rim_services::channel_lens::dto::FilterLogicDto as D;
        match value {
            D::And => Self::And,
            D::Or => Self::Or,
        }
    }
}

impl From<FilterLogic> for rim_services::channel_lens::dto::FilterLogicDto {
    fn from(value: FilterLogic) -> Self {
        match value {
            FilterLogic::And => Self::And,
            FilterLogic::Or => Self::Or,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct FilterConditionGql {
    pub field: FilterField,
    pub op: FilterOp,
    pub value: async_graphql::Json<serde_json::Value>,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct FilterSpecGql {
    pub conditions: Vec<FilterConditionGql>,
    pub logic: FilterLogic,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ChannelLens {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub created_by: async_graphql::ID,
    pub name: String,
    pub filter_spec: FilterSpecGql,
    pub is_shared: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::channel_lens::dto::ChannelLensDto> for ChannelLens {
    fn from(value: rim_services::channel_lens::dto::ChannelLensDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            created_by: async_graphql::ID(value.created_by.to_string()),
            name: value.name,
            filter_spec: FilterSpecGql {
                conditions: value
                    .filter_spec
                    .conditions
                    .into_iter()
                    .map(|c| FilterConditionGql {
                        field: c.field.into(),
                        op: c.op.into(),
                        value: async_graphql::Json(c.value),
                    })
                    .collect(),
                logic: value.filter_spec.logic.into(),
            },
            is_shared: value.is_shared,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct FilterConditionInput {
    pub field: FilterField,
    pub op: FilterOp,
    pub value: async_graphql::Json<serde_json::Value>,
}

#[derive(InputObject, Clone, Debug)]
pub struct FilterSpecInput {
    pub conditions: Vec<FilterConditionInput>,
    pub logic: FilterLogic,
}

impl From<FilterSpecInput> for rim_services::channel_lens::dto::FilterSpecDto {
    fn from(value: FilterSpecInput) -> Self {
        Self {
            conditions: value
                .conditions
                .into_iter()
                .map(|c| rim_services::channel_lens::dto::FilterConditionDto {
                    field: c.field.into(),
                    op: c.op.into(),
                    value: c.value.0,
                })
                .collect(),
            logic: value.logic.into(),
        }
    }
}

// ── Incident Mode ──────────────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IncidentStatusGql {
    Active,
    Mitigated,
    Resolved,
}

impl From<rim_core::incident::model::IncidentStatus> for IncidentStatusGql {
    fn from(value: rim_core::incident::model::IncidentStatus) -> Self {
        use rim_core::incident::model::IncidentStatus as D;
        match value {
            D::Active => Self::Active,
            D::Mitigated => Self::Mitigated,
            D::Resolved => Self::Resolved,
        }
    }
}

impl From<IncidentStatusGql> for rim_core::incident::model::IncidentStatus {
    fn from(value: IncidentStatusGql) -> Self {
        match value {
            IncidentStatusGql::Active => Self::Active,
            IncidentStatusGql::Mitigated => Self::Mitigated,
            IncidentStatusGql::Resolved => Self::Resolved,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IncidentSeverityGql {
    Sev1,
    Sev2,
    Sev3,
    Sev4,
}

impl From<rim_core::incident::model::IncidentSeverity> for IncidentSeverityGql {
    fn from(value: rim_core::incident::model::IncidentSeverity) -> Self {
        use rim_core::incident::model::IncidentSeverity as D;
        match value {
            D::Sev1 => Self::Sev1,
            D::Sev2 => Self::Sev2,
            D::Sev3 => Self::Sev3,
            D::Sev4 => Self::Sev4,
        }
    }
}

impl From<IncidentSeverityGql> for rim_core::incident::model::IncidentSeverity {
    fn from(value: IncidentSeverityGql) -> Self {
        match value {
            IncidentSeverityGql::Sev1 => Self::Sev1,
            IncidentSeverityGql::Sev2 => Self::Sev2,
            IncidentSeverityGql::Sev3 => Self::Sev3,
            IncidentSeverityGql::Sev4 => Self::Sev4,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IncidentPhaseNameGql {
    Detection,
    Triage,
    Mitigation,
    Resolution,
    Postmortem,
}

impl From<rim_core::incident::model::IncidentPhaseName> for IncidentPhaseNameGql {
    fn from(value: rim_core::incident::model::IncidentPhaseName) -> Self {
        use rim_core::incident::model::IncidentPhaseName as D;
        match value {
            D::Detection => Self::Detection,
            D::Triage => Self::Triage,
            D::Mitigation => Self::Mitigation,
            D::Resolution => Self::Resolution,
            D::Postmortem => Self::Postmortem,
        }
    }
}

impl From<IncidentPhaseNameGql> for rim_core::incident::model::IncidentPhaseName {
    fn from(value: IncidentPhaseNameGql) -> Self {
        match value {
            IncidentPhaseNameGql::Detection => Self::Detection,
            IncidentPhaseNameGql::Triage => Self::Triage,
            IncidentPhaseNameGql::Mitigation => Self::Mitigation,
            IncidentPhaseNameGql::Resolution => Self::Resolution,
            IncidentPhaseNameGql::Postmortem => Self::Postmortem,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum MessageTagTypeGql {
    Decision,
    ActionItem,
    RootCause,
    TimelineEvent,
}

impl From<rim_core::incident::model::MessageTagType> for MessageTagTypeGql {
    fn from(value: rim_core::incident::model::MessageTagType) -> Self {
        use rim_core::incident::model::MessageTagType as D;
        match value {
            D::Decision => Self::Decision,
            D::ActionItem => Self::ActionItem,
            D::RootCause => Self::RootCause,
            D::TimelineEvent => Self::TimelineEvent,
        }
    }
}

impl From<MessageTagTypeGql> for rim_core::incident::model::MessageTagType {
    fn from(value: MessageTagTypeGql) -> Self {
        match value {
            MessageTagTypeGql::Decision => Self::Decision,
            MessageTagTypeGql::ActionItem => Self::ActionItem,
            MessageTagTypeGql::RootCause => Self::RootCause,
            MessageTagTypeGql::TimelineEvent => Self::TimelineEvent,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct IncidentGql {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub started_by: async_graphql::ID,
    pub status: IncidentStatusGql,
    pub severity: Option<IncidentSeverityGql>,
    pub title: String,
    pub original_topic: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub mitigated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub postmortem_draft: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::incident::dto::IncidentDto> for IncidentGql {
    fn from(value: rim_services::incident::dto::IncidentDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            started_by: async_graphql::ID(value.started_by.to_string()),
            status: value.status.into(),
            severity: value.severity.map(Into::into),
            title: value.title,
            original_topic: value.original_topic,
            started_at: value.started_at,
            mitigated_at: value.mitigated_at,
            resolved_at: value.resolved_at,
            postmortem_draft: value.postmortem_draft,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct IncidentPhaseGql {
    pub id: async_graphql::ID,
    pub incident_id: async_graphql::ID,
    pub phase: IncidentPhaseNameGql,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub note: Option<String>,
}

impl From<rim_services::incident::dto::IncidentPhaseDto> for IncidentPhaseGql {
    fn from(value: rim_services::incident::dto::IncidentPhaseDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            incident_id: async_graphql::ID(value.incident_id.to_string()),
            phase: value.phase.into(),
            started_at: value.started_at,
            ended_at: value.ended_at,
            note: value.note,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct IncidentMessageTagGql {
    pub message_id: async_graphql::ID,
    pub incident_id: async_graphql::ID,
    pub phase: IncidentPhaseNameGql,
    pub tag_type: Option<MessageTagTypeGql>,
}

impl From<rim_services::incident::dto::IncidentMessageTagDto> for IncidentMessageTagGql {
    fn from(value: rim_services::incident::dto::IncidentMessageTagDto) -> Self {
        Self {
            message_id: async_graphql::ID(value.message_id.to_string()),
            incident_id: async_graphql::ID(value.incident_id.to_string()),
            phase: value.phase.into(),
            tag_type: value.tag_type.map(Into::into),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct IncidentTimelineGql {
    pub incident: IncidentGql,
    pub phases: Vec<IncidentPhaseGql>,
    pub tagged_messages: Vec<IncidentMessageTagGql>,
}

impl From<rim_services::incident::dto::IncidentTimelineDto> for IncidentTimelineGql {
    fn from(value: rim_services::incident::dto::IncidentTimelineDto) -> Self {
        Self {
            incident: value.incident.into(),
            phases: value.phases.into_iter().map(Into::into).collect(),
            tagged_messages: value.tagged_messages.into_iter().map(Into::into).collect(),
        }
    }
}

// ── Workspace State ──────────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum WorkspaceStateScopeGql {
    Server,
    User,
}

impl From<rim_services::workspace_state::dto::WorkspaceStateScopeDto> for WorkspaceStateScopeGql {
    fn from(value: rim_services::workspace_state::dto::WorkspaceStateScopeDto) -> Self {
        match value {
            rim_services::workspace_state::dto::WorkspaceStateScopeDto::Server => Self::Server,
            rim_services::workspace_state::dto::WorkspaceStateScopeDto::User => Self::User,
        }
    }
}

impl From<WorkspaceStateScopeGql> for rim_services::workspace_state::dto::WorkspaceStateScopeDto {
    fn from(value: WorkspaceStateScopeGql) -> Self {
        match value {
            WorkspaceStateScopeGql::Server => Self::Server,
            WorkspaceStateScopeGql::User => Self::User,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct WorkspaceStateEntryGql {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub key: String,
    pub value: async_graphql::Json<serde_json::Value>,
    pub scope: WorkspaceStateScopeGql,
    pub user_id: Option<async_graphql::ID>,
    pub version: i64,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::workspace_state::dto::WorkspaceStateDto> for WorkspaceStateEntryGql {
    fn from(value: rim_services::workspace_state::dto::WorkspaceStateDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            key: value.key,
            value: async_graphql::Json(value.value),
            scope: value.scope.into(),
            user_id: value.user_id.map(|id| async_graphql::ID(id.to_string())),
            version: value.version,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct SetWorkspaceStateInput {
    pub server_id: async_graphql::ID,
    pub key: String,
    pub value: async_graphql::Json<serde_json::Value>,
    pub scope: WorkspaceStateScopeGql,
    pub ttl_seconds: Option<i64>,
}

#[derive(InputObject, Clone, Debug)]
pub struct DeleteWorkspaceStateInput {
    pub server_id: async_graphql::ID,
    pub key: String,
    pub scope: WorkspaceStateScopeGql,
}

#[derive(InputObject, Clone, Debug)]
pub struct IncrementWorkspaceStateInput {
    pub server_id: async_graphql::ID,
    pub key: String,
    pub scope: WorkspaceStateScopeGql,
    pub delta: i64,
}

// ── Pipeline GraphQL types ──────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PipelineTriggerKindGql {
    NewMessage,
    MessageUpdated,
    MessageDeleted,
    ReactionsUpdated,
    MemberJoined,
    WorkspaceStateChanged,
}

impl From<rim_services::pipeline::dto::PipelineTriggerKindDto> for PipelineTriggerKindGql {
    fn from(value: rim_services::pipeline::dto::PipelineTriggerKindDto) -> Self {
        use rim_services::pipeline::dto::PipelineTriggerKindDto as D;
        match value {
            D::NewMessage => Self::NewMessage,
            D::MessageUpdated => Self::MessageUpdated,
            D::MessageDeleted => Self::MessageDeleted,
            D::ReactionsUpdated => Self::ReactionsUpdated,
            D::MemberJoined => Self::MemberJoined,
            D::WorkspaceStateChanged => Self::WorkspaceStateChanged,
        }
    }
}

impl From<PipelineTriggerKindGql> for rim_services::pipeline::dto::PipelineTriggerKindDto {
    fn from(value: PipelineTriggerKindGql) -> Self {
        match value {
            PipelineTriggerKindGql::NewMessage => Self::NewMessage,
            PipelineTriggerKindGql::MessageUpdated => Self::MessageUpdated,
            PipelineTriggerKindGql::MessageDeleted => Self::MessageDeleted,
            PipelineTriggerKindGql::ReactionsUpdated => Self::ReactionsUpdated,
            PipelineTriggerKindGql::MemberJoined => Self::MemberJoined,
            PipelineTriggerKindGql::WorkspaceStateChanged => Self::WorkspaceStateChanged,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipelineTriggerGql {
    pub kind: PipelineTriggerKindGql,
    pub conversation_id: Option<async_graphql::ID>,
}

impl From<rim_services::pipeline::dto::PipelineTriggerDto> for PipelineTriggerGql {
    fn from(value: rim_services::pipeline::dto::PipelineTriggerDto) -> Self {
        Self {
            kind: value.kind.into(),
            conversation_id: value
                .conversation_id
                .map(|id| async_graphql::ID(id.to_string())),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipelineFilterConditionGql {
    pub condition_type: String,
    pub pattern: Option<String>,
    pub user_id: Option<async_graphql::ID>,
    pub conversation_id: Option<async_graphql::ID>,
}

impl From<rim_services::pipeline::dto::FilterConditionDto> for PipelineFilterConditionGql {
    fn from(value: rim_services::pipeline::dto::FilterConditionDto) -> Self {
        use rim_services::pipeline::dto::FilterConditionDto as D;
        match value {
            D::MessageContains { pattern } => Self {
                condition_type: "MessageContains".to_string(),
                pattern: Some(pattern),
                user_id: None,
                conversation_id: None,
            },
            D::MessageFromUser { user_id } => Self {
                condition_type: "MessageFromUser".to_string(),
                pattern: None,
                user_id: Some(async_graphql::ID(user_id.to_string())),
                conversation_id: None,
            },
            D::ConversationIs { conversation_id } => Self {
                condition_type: "ConversationIs".to_string(),
                pattern: None,
                user_id: None,
                conversation_id: Some(async_graphql::ID(conversation_id.to_string())),
            },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipelineFilterGql {
    pub conditions: Vec<PipelineFilterConditionGql>,
}

impl From<rim_services::pipeline::dto::PipelineFilterDto> for PipelineFilterGql {
    fn from(value: rim_services::pipeline::dto::PipelineFilterDto) -> Self {
        Self {
            conditions: value.conditions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipelineActionGql {
    pub action_type: String,
    pub command_id: Option<String>,
    pub args: Option<async_graphql::Json<serde_json::Value>>,
}

impl From<rim_services::pipeline::dto::PipelineActionDto> for PipelineActionGql {
    fn from(value: rim_services::pipeline::dto::PipelineActionDto) -> Self {
        use rim_services::pipeline::dto::PipelineActionDto as D;
        match value {
            D::ExecuteCommand { command_id, args } => Self {
                action_type: "ExecuteCommand".to_string(),
                command_id: Some(command_id),
                args: args.map(async_graphql::Json),
            },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipelineGql {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTriggerGql,
    pub filter: Option<PipelineFilterGql>,
    pub actions: Vec<PipelineActionGql>,
    pub enabled: bool,
    pub cooldown_secs: Option<i32>,
    pub created_by: async_graphql::ID,
    pub schema_version: i32,
    pub last_fired_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::pipeline::dto::PipelineDto> for PipelineGql {
    fn from(value: rim_services::pipeline::dto::PipelineDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            name: value.name,
            description: value.description,
            trigger: value.trigger.into(),
            filter: value.filter.map(Into::into),
            actions: value.actions.into_iter().map(Into::into).collect(),
            enabled: value.enabled,
            cooldown_secs: value.cooldown_secs,
            created_by: async_graphql::ID(value.created_by.to_string()),
            schema_version: value.schema_version,
            last_fired_at: value.last_fired_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

// Input types for pipeline mutations

#[derive(InputObject, Clone, Debug)]
pub struct PipelineTriggerInput {
    pub kind: PipelineTriggerKindGql,
    pub conversation_id: Option<async_graphql::ID>,
}

#[derive(InputObject, Clone, Debug)]
pub struct PipelineFilterConditionInput {
    pub condition_type: String,
    pub pattern: Option<String>,
    pub user_id: Option<async_graphql::ID>,
    pub conversation_id: Option<async_graphql::ID>,
}

#[derive(InputObject, Clone, Debug)]
pub struct PipelineFilterInput {
    pub conditions: Vec<PipelineFilterConditionInput>,
}

#[derive(InputObject, Clone, Debug)]
pub struct PipelineActionInput {
    pub command_id: String,
    pub args: Option<async_graphql::Json<serde_json::Value>>,
}

#[derive(InputObject, Clone, Debug)]
pub struct CreatePipelineInput {
    pub server_id: async_graphql::ID,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTriggerInput,
    pub filter: Option<PipelineFilterInput>,
    pub actions: Vec<PipelineActionInput>,
    pub cooldown_secs: Option<i32>,
}

#[derive(InputObject, Clone, Debug)]
pub struct UpdatePipelineInput {
    pub pipeline_id: async_graphql::ID,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTriggerInput,
    pub filter: Option<PipelineFilterInput>,
    pub actions: Vec<PipelineActionInput>,
    pub cooldown_secs: Option<i32>,
}
