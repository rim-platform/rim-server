use async_graphql::*;

use crate::auth_context::require_actor;
use crate::pagination::PageDir;
use crate::types::{
    Action, Attachment, Capability, ChannelCursorInput, ChannelLens, ChannelPage, Command,
    CommandContextInput, CommandSurface, Conversation, ConversationPage, ConversationTypeEnum,
    CustomEmoji, DefaultKeybind, Event, InboxCursorInput, IncidentGql, IncidentListCursorInput,
    IncidentPage, IncidentSeverityGql, IncidentStatusGql, IncidentTimelineGql, InstanceInviteGql,
    Integration, Invite, InviteTokenPreviewGql, Keybind, KeybindContext, KeybindLayers,
    KeybindScope, MacroDef, Message, MessageCursorInput, MessagePage, MessageReference,
    MutedConversation, RegistrationModeGql, SearchResultMessage, Server, ServerMember, Setting,
    Thread, ThreadCursorInput, ThreadPage, UnreadCount, User, UserPresenceGql,
};
use rim_services::Actor;
use rim_services::action::service::ActionService;
use rim_services::auth::command::ListInvitesQuery;
use rim_services::auth::service::AuthService;
use rim_services::capability::service::CapabilityService;
use rim_services::command::request::ListCommandsQuery;
use rim_services::command::service::CommandService;
use rim_services::conversation::command::{
    GetConversationQuery, ListChannelsQuery, ListInboxConversationsQuery,
};
use rim_services::conversation::service::ConversationService;
use rim_services::event::service::EventService;
use rim_services::integration::service::IntegrationService;
use rim_services::keybind::request::{
    InspectKeybindLayersQuery, ListDefaultKeybindsQuery, ListKeybindsQuery, ResolveKeybindQuery,
    ResolveKeybindsQuery,
};
use rim_services::keybind::service::KeybindService;
use rim_services::message::command::ListThreadMessagesQuery;
use rim_services::message::command::{
    GetMessageQuery, GetMessageReferenceQuery, ListMessagesQuery, SearchMessagesQuery,
};
use rim_services::message::service::MessageService;
use rim_services::message_attachment::command::GetAttachmentQuery;
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::message_thread::command::{
    GetThreadByRootMessageQuery, GetThreadQuery, ListThreadsQuery,
};
use rim_services::message_thread::service::ThreadService;
use rim_services::server::command::{GetServerQuery, ListMyServersQuery};
use rim_services::server::service::ServerService;
use rim_services::server_member::command::ListServerMembersQuery;
use rim_services::server_member::service::ServerMemberService;
use rim_services::setting::service::SettingService;
use rim_services::user::command::GetMeQuery;
use rim_services::user::service::UserService;

use crate::ids::parse_uuid_id;

pub struct Query;

#[Object]
impl Query {
    async fn commands(
        &self,
        ctx: &Context<'_>,
        surface: Option<CommandSurface>,
        command_context: Option<CommandContextInput>,
    ) -> Result<Vec<Command>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;

        let commands = svc
            .list_commands(ListCommandsQuery {
                actor: Actor::new(actor),
                context: command_context.map(TryInto::try_into).transpose()?,
                surface: surface.unwrap_or(CommandSurface::Palette).into(),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(commands.into_iter().map(Into::into).collect())
    }

    /// P5-A7 — AI Agent command-discovery meta-tool (v2 §5 R1 #3, M6).
    ///
    /// Returns the actor's accessible AI-callable command surface, optionally
    /// ranked by `intent_phrase` relevance (literal substring + token-overlap
    /// against `intent_samples`, `title`, `nl_description`), truncated to
    /// `limit` (default 50, hard-capped at 100), and scoped to a workspace for
    /// macro synthesis.
    ///
    /// Rate-limited: 5 calls / 60-second sliding window per actor (M6).
    /// Returns `RATE_LIMITED` on overage.
    async fn command_discover(
        &self,
        ctx: &Context<'_>,
        intent_phrase: Option<String>,
        limit: Option<i32>,
        workspace_id: Option<ID>,
    ) -> Result<Vec<Command>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;

        let ws_id = workspace_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::server::model::ServerId>)
            .transpose()?;

        let commands = svc
            .command_discover(
                &Actor::new(actor),
                intent_phrase,
                limit.map(|l| l.max(1) as u32),
                ws_id,
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(commands.into_iter().map(Into::into).collect())
    }

    async fn command(&self, ctx: &Context<'_>, command_id: String) -> Result<Option<Command>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;

        let command = svc
            .get_command(rim_services::command::request::GetCommandQuery {
                actor: Actor::new(actor),
                command_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(command.map(Into::into))
    }

    async fn actions(&self, ctx: &Context<'_>) -> Result<Vec<Action>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ActionService>>()?;

        let actions = svc
            .list_actions(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(actions.into_iter().map(Into::into).collect())
    }

    async fn events(&self, ctx: &Context<'_>) -> Result<Vec<Event>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn EventService>>()?;

        let events = svc
            .list_events(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(events.into_iter().map(Into::into).collect())
    }

    async fn capabilities(&self, ctx: &Context<'_>) -> Result<Vec<Capability>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn CapabilityService>>()?;

        let capabilities = svc
            .list_capabilities(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(capabilities.into_iter().map(Into::into).collect())
    }

    async fn integrations(&self, ctx: &Context<'_>) -> Result<Vec<Integration>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn IntegrationService>>()?;

        let integrations = svc
            .list_integrations(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(integrations.into_iter().map(Into::into).collect())
    }

    async fn settings(&self, ctx: &Context<'_>) -> Result<Vec<Setting>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn SettingService>>()?;

        let settings = svc
            .list_settings(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(settings.into_iter().map(Into::into).collect())
    }

    async fn keybinds(
        &self,
        ctx: &Context<'_>,
        scope: KeybindScope,
        workspace_id: Option<ID>,
    ) -> Result<Vec<Keybind>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;

        let keybinds = svc
            .list_keybinds(ListKeybindsQuery {
                actor: Actor::new(actor),
                scope: scope.into(),
                workspace_id: workspace_id
                    .as_ref()
                    .map(parse_uuid_id::<rim_core::server::model::WorkspaceId>)
                    .transpose()?,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(keybinds.into_iter().map(Into::into).collect())
    }

    async fn default_keybinds(&self, ctx: &Context<'_>) -> Result<Vec<DefaultKeybind>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;

        let keybinds = svc
            .list_default_keybinds(ListDefaultKeybindsQuery {
                actor: Actor::new(actor),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(keybinds.into_iter().map(Into::into).collect())
    }

    async fn resolved_keybinds(
        &self,
        ctx: &Context<'_>,
        workspace_id: Option<ID>,
    ) -> Result<Vec<Keybind>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;

        let keybinds = svc
            .resolve_keybinds(ResolveKeybindsQuery {
                actor: Actor::new(actor),
                workspace_id: workspace_id
                    .as_ref()
                    .map(parse_uuid_id::<rim_core::server::model::WorkspaceId>)
                    .transpose()?,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(keybinds.into_iter().map(Into::into).collect())
    }

    async fn resolve_keybind(
        &self,
        ctx: &Context<'_>,
        keystroke: String,
        context: KeybindContext,
        workspace_id: Option<ID>,
    ) -> Result<Option<Keybind>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;

        let keybind = svc
            .resolve_keybind(ResolveKeybindQuery {
                actor: Actor::new(actor),
                workspace_id: workspace_id
                    .as_ref()
                    .map(parse_uuid_id::<rim_core::server::model::WorkspaceId>)
                    .transpose()?,
                keystroke,
                context: context.into(),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(keybind.map(Into::into))
    }

    async fn keybind_layers(
        &self,
        ctx: &Context<'_>,
        keystroke: String,
        context: KeybindContext,
        workspace_id: Option<ID>,
    ) -> Result<KeybindLayers> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn KeybindService>>()?;

        let layers = svc
            .inspect_keybind_layers(InspectKeybindLayersQuery {
                actor: Actor::new(actor),
                workspace_id: workspace_id
                    .as_ref()
                    .map(parse_uuid_id::<rim_core::server::model::WorkspaceId>)
                    .transpose()?,
                keystroke,
                context: context.into(),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(layers.into())
    }

    async fn conversation(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let conv = svc
            .get_conversation(GetConversationQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(conv.into())
    }

    async fn inbox_conversations(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        dir: PageDir,
        cursor: Option<InboxCursorInput>,
        limit: i32,
    ) -> Result<ConversationPage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let page = svc
            .list_inbox_conversations(ListInboxConversationsQuery {
                actor: Actor::new(actor),
                server_id,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(ConversationPage::from_inbox_page(page))
    }

    async fn channels(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        dir: PageDir,
        cursor: Option<ChannelCursorInput>,
        limit: i32,
    ) -> Result<ChannelPage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let page = svc
            .list_channels(ListChannelsQuery {
                actor: Actor::new(actor),
                server_id,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(ChannelPage::from_channel_page(page))
    }

    #[allow(clippy::too_many_arguments)]
    async fn messages(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        dir: PageDir,
        cursor: Option<MessageCursorInput>,
        limit: i32,
        include_deleted: bool,
    ) -> Result<MessagePage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let page = svc
            .list_messages(ListMessagesQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
                include_deleted,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(MessagePage::from_page(page))
    }

    async fn search_messages(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: Option<ID>,
        query: String,
        limit: i32,
        include_deleted: bool,
    ) -> Result<Vec<SearchResultMessage>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let conv_svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id = conversation_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::conversation::model::ConversationId>)
            .transpose()?;

        let dtos = svc
            .search_messages(SearchMessagesQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                query,
                limit: (limit.max(1) as u32).min(50),
                include_deleted,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        // Batch-load conversation metadata for results
        let unique_conv_ids: Vec<_> = {
            let mut seen = std::collections::HashSet::new();
            dtos.iter()
                .filter_map(|d| {
                    if seen.insert(d.conversation_id) {
                        Some(d.conversation_id)
                    } else {
                        None
                    }
                })
                .collect()
        };

        let conversations = conv_svc
            .get_conversations_by_ids(&unique_conv_ids)
            .await
            .map_err(crate::errors::map_service_error)?;
        let conv_map: std::collections::HashMap<_, _> =
            conversations.into_iter().map(|c| (c.id, c)).collect();

        Ok(dtos
            .into_iter()
            .map(|dto| {
                let conv = conv_map.get(&dto.conversation_id);
                SearchResultMessage {
                    id: ID(dto.id.to_string()),
                    server_id: ID(dto.server_id.to_string()),
                    conversation_id: ID(dto.conversation_id.to_string()),
                    conversation_title: conv.and_then(|c| c.title.clone()),
                    conversation_type: conv
                        .map(|c| ConversationTypeEnum::from(c.conversation_type))
                        .unwrap_or(ConversationTypeEnum::Channel),
                    thread_id: dto.thread_id.map(|id| ID(id.to_string())),
                    root_message_id: dto.thread_root_message_id.map(|id| ID(id.to_string())),
                    author_id: ID(dto.author_id.to_string()),
                    text: dto.text,
                    sent_at: dto.sent_at,
                }
            })
            .collect())
    }

    async fn message(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        message_id: ID,
    ) -> Result<Message> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&message_id)?;

        let dto = svc
            .get_message(GetMessageQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                message_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(dto.into())
    }

    async fn message_reference(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        message_id: ID,
    ) -> Result<MessageReference> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&message_id)?;

        let reference = svc
            .get_message_reference(GetMessageReferenceQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                message_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(reference.into())
    }

    async fn thread(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        thread_id: ID,
    ) -> Result<Thread> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ThreadService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let thread_id =
            parse_uuid_id::<rim_core::message_thread::model::MessageThreadId>(&thread_id)?;

        let t = svc
            .get_thread(GetThreadQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                thread_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(t.into())
    }

    async fn thread_by_root_message(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        root_message_id: ID,
    ) -> Result<Option<Thread>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ThreadService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let root_message_id =
            parse_uuid_id::<rim_core::message::model::MessageId>(&root_message_id)?;

        let t = svc
            .get_thread_by_root_message(GetThreadByRootMessageQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                root_message_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(t.map(Into::into))
    }

    async fn threads(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        dir: PageDir,
        cursor: Option<ThreadCursorInput>,
        limit: i32,
    ) -> Result<ThreadPage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ThreadService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let page = svc
            .list_threads(ListThreadsQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(ThreadPage::from_page(page))
    }

    #[allow(clippy::too_many_arguments)]
    async fn thread_messages(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        thread_id: ID,
        dir: PageDir,
        cursor: Option<MessageCursorInput>,
        limit: i32,
        include_deleted: bool,
    ) -> Result<MessagePage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let thread_id =
            parse_uuid_id::<rim_core::message_thread::model::MessageThreadId>(&thread_id)?;

        let page = svc
            .list_thread_messages(ListThreadMessagesQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                thread_id,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
                include_deleted,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(MessagePage::from_page(page))
    }

    async fn attachment(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        attachment_id: ID,
    ) -> Result<Attachment> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageAttachmentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let attachment_id = parse_uuid_id::<
            rim_core::message_attachment::model::MessageAttachmentId,
        >(&attachment_id)?;

        let attachment = svc
            .get_attachment(GetAttachmentQuery {
                actor: Actor::new(actor),
                server_id,
                attachment_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(attachment.into())
    }

    async fn me(&self, ctx: &Context<'_>) -> Result<User> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn UserService>>()?;

        let dto = svc
            .get_me(GetMeQuery {
                actor: Actor::new(actor),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(dto.into())
    }

    async fn my_servers(&self, ctx: &Context<'_>) -> Result<Vec<Server>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ServerService>>()?;

        let servers = svc
            .list_my_servers(ListMyServersQuery {
                actor: Actor::new(actor),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(servers.into_iter().map(Into::into).collect())
    }

    async fn server(&self, ctx: &Context<'_>, server_id: ID) -> Result<Server> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ServerService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let dto = svc
            .get_server(GetServerQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(dto.into())
    }

    async fn unread_counts(&self, ctx: &Context<'_>, workspace_id: ID) -> Result<Vec<UnreadCount>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::read_state::service::ReadStateService>>()?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&workspace_id)?;

        let counts = svc
            .get_unread_counts(rim_services::read_state::dto::UnreadCountsQuery {
                actor: Actor::new(actor),
                workspace_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(counts.into_iter().map(Into::into).collect())
    }

    async fn server_members(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<ServerMember>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ServerMemberService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let members = svc
            .list_members(ListServerMembersQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(members.into_iter().map(Into::into).collect())
    }

    async fn my_mentions(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        #[graphql(default = 50)] limit: u32,
    ) -> Result<Vec<Message>> {
        let actor = require_actor(ctx)?;

        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::mention::service::MentionService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let messages = svc
            .get_my_mentions(rim_services::mention::command::MyMentionsQuery {
                actor: rim_services::Actor::new(actor),
                server_id,
                limit,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(messages.into_iter().map(Into::into).collect())
    }

    async fn pinned_messages(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<Vec<Message>> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn rim_services::pin::service::PinService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let messages = svc
            .list_pinned_messages(rim_services::pin::command::ListPinnedMessagesQuery {
                actor: rim_services::Actor::new(actor),
                server_id,
                conversation_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(messages.into_iter().map(Into::into).collect())
    }

    async fn workspace_presence(
        &self,
        ctx: &Context<'_>,
        workspace_id: ID,
    ) -> Result<Vec<UserPresenceGql>> {
        let _actor = require_actor(ctx)?;
        let presence_mgr =
            ctx.data::<std::sync::Arc<dyn rim_services::presence::PresenceService>>()?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&workspace_id)?;

        let entries = presence_mgr.get_workspace_presence(workspace_id);
        Ok(entries.into_iter().map(Into::into).collect())
    }

    async fn my_invites(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<Invite>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn AuthService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let invites = svc
            .list_invites(ListInvitesQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(invites.into_iter().map(Into::into).collect())
    }

    async fn muted_conversations(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<Vec<MutedConversation>> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::notification_preference::service::NotificationPreferenceService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let items = svc
            .list_muted_conversations(
                rim_services::notification_preference::command::ListMutedConversationsQuery {
                    actor: rim_services::Actor::new(actor),
                    server_id,
                },
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(items.into_iter().map(Into::into).collect())
    }

    async fn custom_emojis(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<CustomEmoji>> {
        let actor = require_actor(ctx)?;

        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::custom_emoji::service::CustomEmojiService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let emojis = svc
            .list(rim_services::custom_emoji::command::ListCustomEmojisQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(emojis.into_iter().map(Into::into).collect())
    }

    // ── Macros ──────────────────────────────────────────────────────────

    async fn my_macros(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<MacroDef>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::macro_def::service::MacroService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let macros = svc
            .list_user_macros(rim_services::macro_def::request::ListUserMacrosQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(macros.into_iter().map(Into::into).collect())
    }

    async fn macro_by_name(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        name: String,
    ) -> Result<Option<MacroDef>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::macro_def::service::MacroService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let macro_def = svc
            .get_macro_by_name(rim_services::macro_def::request::GetMacroByNameQuery {
                actor: Actor::new(actor),
                server_id,
                name,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(macro_def.map(Into::into))
    }

    async fn server_macros(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<MacroDef>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::macro_def::service::MacroService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let macros = svc
            .list_server_macros(rim_services::macro_def::request::ListServerMacrosQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(macros.into_iter().map(Into::into).collect())
    }

    // ── Skill Profile ──────────────────────────────────────────────────

    async fn my_skill_profile(&self, ctx: &Context<'_>) -> Result<crate::types::SkillProfileGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::skill_profile::service::SkillProfileService>>(
            )?;

        let profile = svc
            .get_skill_profile(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(profile.into())
    }

    // ── Channel Lenses ────────────────────────────────────────────────────

    async fn channel_lenses(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<Vec<ChannelLens>> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::channel_lens::service::ChannelLensService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let lenses = svc
            .list_lenses(rim_services::channel_lens::command::ListLensesQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(lenses.into_iter().map(Into::into).collect())
    }

    async fn apply_channel_lens(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        lens_id: ID,
        #[graphql(default = 50)] limit: i32,
    ) -> Result<Vec<Message>> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::channel_lens::service::ChannelLensService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let lens_id = parse_uuid_id::<rim_core::channel_lens::model::ChannelLensId>(&lens_id)?;

        let messages = svc
            .apply_lens(rim_services::channel_lens::command::ApplyLensQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                lens_id,
                limit: (limit.max(1) as u32).min(50),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(messages.into_iter().map(Into::into).collect())
    }

    // ── Incidents ──────────────────────────────────────────────────────

    async fn active_incident(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<Option<IncidentGql>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let incident = svc
            .get_active_incident(rim_services::incident::command::GetActiveIncidentQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(incident.map(Into::into))
    }

    async fn incident_timeline(
        &self,
        ctx: &Context<'_>,
        incident_id: ID,
    ) -> Result<IncidentTimelineGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let incident_id = parse_uuid_id::<rim_core::incident::model::IncidentId>(&incident_id)?;

        let timeline = svc
            .get_incident_timeline(rim_services::incident::command::GetIncidentTimelineQuery {
                actor: Actor::new(actor),
                incident_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(timeline.into())
    }

    #[allow(clippy::too_many_arguments)]
    async fn list_incidents(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        status_filter: Option<Vec<IncidentStatusGql>>,
        severity_filter: Option<Vec<IncidentSeverityGql>>,
        dir: PageDir,
        cursor: Option<IncidentListCursorInput>,
        #[graphql(default = 20)] limit: i32,
    ) -> Result<IncidentPage> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::incident::service::IncidentService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let page = svc
            .list_incidents(rim_services::incident::command::ListIncidentsQuery {
                actor: Actor::new(actor),
                server_id,
                status_filter: status_filter
                    .map(|v| v.into_iter().map(Into::into).collect::<Vec<_>>()),
                severity_filter: severity_filter
                    .map(|v| v.into_iter().map(Into::into).collect::<Vec<_>>()),
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(IncidentPage::from_page(page))
    }

    async fn workspace_state(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        key: String,
        scope: crate::types::WorkspaceStateScopeGql,
    ) -> Result<Option<crate::types::WorkspaceStateEntryGql>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::workspace_state::service::WorkspaceStateService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let entry = svc
            .get(
                rim_services::workspace_state::request::GetWorkspaceStateQuery {
                    actor: Actor::new(actor),
                    server_id,
                    key,
                    scope: scope.into(),
                },
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(entry.map(Into::into))
    }

    async fn workspace_state_list(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        prefix: String,
        scope: crate::types::WorkspaceStateScopeGql,
        limit: Option<u64>,
    ) -> Result<Vec<crate::types::WorkspaceStateEntryGql>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::workspace_state::service::WorkspaceStateService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let entries = svc
            .list(
                rim_services::workspace_state::request::ListWorkspaceStateQuery {
                    actor: Actor::new(actor),
                    server_id,
                    prefix,
                    scope: scope.into(),
                    limit,
                },
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(entries.into_iter().map(Into::into).collect())
    }

    async fn pipeline(
        &self,
        ctx: &Context<'_>,
        pipeline_id: async_graphql::ID,
    ) -> Result<crate::types::PipelineGql> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let pipeline_id = parse_uuid_id::<rim_core::pipeline::model::PipelineId>(&pipeline_id)?;

        let pipeline = svc
            .get(Actor::new(actor), pipeline_id)
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(pipeline.into())
    }

    async fn pipelines(
        &self,
        ctx: &Context<'_>,
        server_id: async_graphql::ID,
        enabled: Option<bool>,
    ) -> Result<Vec<crate::types::PipelineGql>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::pipeline::service::PipelineService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let pipelines = svc
            .list(rim_services::pipeline::request::ListPipelinesCmd {
                actor: Actor::new(actor),
                server_id,
                enabled,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(pipelines.into_iter().map(Into::into).collect())
    }

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

    /// Return the deployment-wide registration mode.
    ///
    /// Used by the login page to decide whether to render a "Sign up" link.
    /// No authentication required — this is public configuration metadata.
    async fn registration_mode(&self, ctx: &Context<'_>) -> Result<RegistrationModeGql> {
        let mode = ctx.data::<std::sync::Arc<rim_services::auth::dto::RegistrationMode>>()?;
        Ok(RegistrationModeGql::from(mode.as_ref().clone()))
    }

    /// List all active (non-revoked) instance invites.
    ///
    /// Admin-only: requires the caller to be `Owner` on at least one workspace.
    /// Returns `FORBIDDEN` otherwise.
    async fn list_instance_invites(&self, ctx: &Context<'_>) -> Result<Vec<InstanceInviteGql>> {
        let actor = require_actor(ctx)?;
        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;

        let invites = auth
            .list_instance_invites(rim_services::Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(invites.into_iter().map(InstanceInviteGql::from).collect())
    }

    /// Preview a registration token without consuming it.
    ///
    /// Returns minimal metadata so the frontend can render the correct signup
    /// copy before the user submits credentials. Rate-limited at 30 req / 5 min
    /// per IP; no authentication required.
    ///
    /// See spec §4.7.
    async fn preview_invite_token(
        &self,
        ctx: &Context<'_>,
        token: String,
    ) -> Result<InviteTokenPreviewGql> {
        // Fail-closed: preview rate limiter MUST be wired into context (§4.7 boot invariant).
        let _ = ctx
            .data::<std::sync::Arc<rim_services::auth::dto::PreviewInviteRateLimiterToken>>()
            .map_err(|_| {
                async_graphql::Error::new("internal error: preview rate limiter not configured")
                    .extend_with(|_, e| e.set("code", "INTERNAL"))
            })?;

        let remote_ip = ctx
            .data::<crate::auth_context::RemoteIp>()
            .map(|r| r.0.clone())
            .unwrap_or_else(|_| "unknown".to_string());

        let auth = ctx.data::<std::sync::Arc<dyn AuthService>>()?;

        let preview = auth
            .preview_invite_token(rim_services::auth::command::PreviewInviteTokenQuery {
                token,
                remote_ip,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(InviteTokenPreviewGql::from(preview))
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
