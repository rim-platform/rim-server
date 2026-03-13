use async_graphql::{Enum, InputObject, SimpleObject};

use rim_core::repository::pagination::Page;

#[derive(SimpleObject, Clone, Debug)]
pub struct User {
    pub id: async_graphql::ID,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Command {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub source_kind: String,
    pub execution_target: String,
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
            source_kind: format!("{:?}", value.source_kind),
            execution_target: format!("{:?}", value.execution_target),
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
pub struct CommandResult {
    pub status: String,
    pub message: String,
    pub payload: Option<async_graphql::Json<serde_json::Value>>,
    pub follow_up_commands: Vec<String>,
}

impl From<rim_services::command::dto::CommandResultDto> for CommandResult {
    fn from(value: rim_services::command::dto::CommandResultDto) -> Self {
        Self {
            status: format!("{:?}", value.status),
            message: value.message,
            payload: value.payload.map(async_graphql::Json),
            follow_up_commands: value.follow_up_commands,
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
    pub kind: String,
    pub title: String,
    pub enabled: bool,
    pub principal_id: Option<async_graphql::ID>,
}

impl From<rim_services::integration::dto::IntegrationDto> for Integration {
    fn from(value: rim_services::integration::dto::IntegrationDto) -> Self {
        Self {
            id: value.id,
            kind: format!("{:?}", value.kind),
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
    pub scope: String,
    pub title: String,
    pub description: String,
    pub value_type: String,
    pub is_secret: bool,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Keybind {
    pub id: async_graphql::ID,
    pub source: String,
    pub scope: String,
    pub workspace_id: Option<async_graphql::ID>,
    pub keystroke: String,
    pub command_id: String,
    pub context: String,
    pub preset_args: Option<async_graphql::Json<serde_json::Value>>,
    pub is_enabled: bool,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct DefaultKeybind {
    pub source: String,
    pub keystroke: String,
    pub command_id: String,
    pub context: String,
    pub preset_args: Option<async_graphql::Json<serde_json::Value>>,
}

impl From<rim_services::keybind::dto::DefaultKeybindDto> for DefaultKeybind {
    fn from(value: rim_services::keybind::dto::DefaultKeybindDto) -> Self {
        Self {
            source: format!("{:?}", value.source),
            keystroke: value.keystroke,
            command_id: value.command_id,
            context: format!("{:?}", value.context),
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
            source: format!("{:?}", value.source),
            scope: format!("{:?}", value.scope),
            workspace_id: value
                .workspace_id
                .map(|id| async_graphql::ID(id.to_string())),
            keystroke: value.keystroke,
            command_id: value.command_id,
            context: format!("{:?}", value.context),
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

impl From<rim_services::setting::dto::SettingDto> for Setting {
    fn from(value: rim_services::setting::dto::SettingDto) -> Self {
        Self {
            id: value.id,
            scope: format!("{:?}", value.scope),
            title: value.title,
            description: value.description,
            value_type: value.value_type,
            is_secret: value.is_secret,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Conversation {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_type: String,
    pub title: Option<String>,
    pub member_ids: Vec<async_graphql::ID>,
}

impl From<rim_services::conversation::dto::ConversationDto> for Conversation {
    fn from(value: rim_services::conversation::dto::ConversationDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_type: format!("{:?}", value.conversation_type),
            title: value.title,
            member_ids: value
                .member_ids
                .into_iter()
                .map(|id| async_graphql::ID(id.to_string()))
                .collect(),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
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
            text: value.text,
            has_attachments: value.has_attachments,
            attachments: value.attachments.into_iter().map(Into::into).collect(),
            parent_message_id: value
                .parent_message_id
                .map(|id| async_graphql::ID(id.to_string())),
            is_deleted: value.is_deleted,
            sent_at: value.sent_at,
        }
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
    pub status: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
}

impl From<rim_services::message_attachment::dto::AttachmentDto> for Attachment {
    fn from(value: rim_services::message_attachment::dto::AttachmentDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            status: format!("{:?}", value.status),
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
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
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
pub struct ThreadCursorOutput {
    pub last_message_at: chrono::DateTime<chrono::Utc>,
    pub thread_id: async_graphql::ID,
}
