use async_graphql::*;

use crate::auth_context::require_actor;
use crate::pagination::PageDir;
use crate::types::{
    Action, Attachment, Capability, ChannelCursorInput, ChannelPage, Command, CommandContextInput,
    CommandSurface, Conversation, ConversationPage, DefaultKeybind, Event, InboxCursorInput,
    Integration, Keybind, KeybindContext, KeybindLayers, KeybindScope, Message, MessageCursorInput,
    MessagePage, MessageReference, Server, ServerMember, Setting, Thread, ThreadCursorInput,
    ThreadPage, User,
};
use rim_services::Actor;
use rim_services::action::service::ActionService;
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
use rim_services::message_reaction::command::GetReactionsForMessagesQuery;
use rim_services::message_reaction::service::MessageReactionService;
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
        let reaction_svc = ctx.data::<std::sync::Arc<dyn MessageReactionService>>()?;
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

        let message_ids: Vec<_> = page.items.iter().map(|m| m.id).collect();
        let reaction_map = reaction_svc
            .get_reactions_for_messages(GetReactionsForMessagesQuery {
                actor: Actor::new(actor),
                message_ids,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(MessagePage::from_page(page, &reaction_map))
    }

    async fn search_messages(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        query: String,
        limit: i32,
        include_deleted: bool,
    ) -> Result<Vec<Message>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let reaction_svc = ctx.data::<std::sync::Arc<dyn MessageReactionService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let dtos = svc
            .search_messages(SearchMessagesQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                query,
                limit: (limit.max(1) as u32).min(100),
                include_deleted,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        let message_ids: Vec<_> = dtos.iter().map(|m| m.id).collect();
        let reaction_map = reaction_svc
            .get_reactions_for_messages(GetReactionsForMessagesQuery {
                actor: Actor::new(actor),
                message_ids,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(dtos
            .into_iter()
            .map(|dto| {
                let msg_id = dto.id;
                let mut msg: Message = dto.into();
                if let Some(groups) = reaction_map.get(&msg_id) {
                    msg.reactions = groups.iter().cloned().map(Into::into).collect();
                }
                msg
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
        let reaction_svc = ctx.data::<std::sync::Arc<dyn MessageReactionService>>()?;
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

        let reaction_map = reaction_svc
            .get_reactions_for_messages(GetReactionsForMessagesQuery {
                actor: Actor::new(actor),
                message_ids: vec![dto.id],
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        let msg_id = dto.id;
        let mut msg: Message = dto.into();
        if let Some(groups) = reaction_map.get(&msg_id) {
            msg.reactions = groups.iter().cloned().map(Into::into).collect();
        }

        Ok(msg)
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
        let reaction_svc = ctx.data::<std::sync::Arc<dyn MessageReactionService>>()?;
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

        let message_ids: Vec<_> = page.items.iter().map(|m| m.id).collect();
        let reaction_map = reaction_svc
            .get_reactions_for_messages(GetReactionsForMessagesQuery {
                actor: Actor::new(actor),
                message_ids,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(MessagePage::from_page(page, &reaction_map))
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
}
