use std::sync::Arc;

use rim_core::auth::repository::{InstanceInviteRepository, InviteRepository, SessionRepository};
use rim_core::capability::repository::CapabilityRegistryRepository;
use rim_core::channel_lens::repository::ChannelLensRepository;
use rim_core::command::repository::UserCommandRepository;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::conversation_read_state::repository::ConversationReadStateRepository;
use rim_core::custom_emoji::repository::CustomEmojiRepository;
use rim_core::device_link::repository::DeviceLinkRepository;
use rim_core::e2ee::repository::E2eeRepository;
use rim_core::host::repository::{DelegationRepository, HostRepository};
use rim_core::key_identity::repository::{KeyChallengeRepository, KeyIdentityRepository};
use rim_core::keybind::repository::KeybindRepository;
use rim_core::link_preview::repository::LinkPreviewRepository;
use rim_core::macro_def::repository::MacroRepository;
use rim_core::message::repository::MessageRepository;
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::message_mention::repository::MessageMentionRepository;
use rim_core::message_pin::repository::MessagePinRepository;
use rim_core::message_reaction::repository::MessageReactionRepository;
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::moderation::repository::ModerationRepository;
use rim_core::notification::repository::NotificationRepository;
use rim_core::notification_preference::repository::NotificationPreferenceRepository;
use rim_core::password_reset::repository::PasswordResetTokenRepository;
use rim_core::pipeline::repository::PipelineRepository;
use rim_core::relay::repository::{RelayAbuseRepository, RelayRepository, SubscriptionRepository};
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::repository::UserRepository;
use rim_core::workspace_plugin::repository::WorkspacePluginRepository;
use rim_core::workspace_state::repository::WorkspaceStateRepository;
use rim_infra_db::DbHandle;

#[derive(Clone)]
pub struct AppRepos {
    pub instance_invite_repo: Arc<dyn InstanceInviteRepository>,
    pub channel_lens_repo: Arc<dyn ChannelLensRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub keybind_repo: Arc<dyn KeybindRepository>,
    pub macro_repo: Arc<dyn MacroRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub message_attachment_repo: Arc<dyn MessageAttachmentRepository>,
    pub message_thread_repo: Arc<dyn MessageThreadRepository>,
    pub user_command_repo: Arc<dyn UserCommandRepository>,
    pub server_repo: Arc<dyn ServerRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub user_repo: Arc<dyn UserRepository>,
    pub session_repo: Arc<dyn SessionRepository>,
    pub invite_repo: Arc<dyn InviteRepository>,
    pub message_reaction_repo: Arc<dyn MessageReactionRepository>,
    pub message_mention_repo: Arc<dyn MessageMentionRepository>,
    pub message_pin_repo: Arc<dyn MessagePinRepository>,
    pub notification_preference_repo: Arc<dyn NotificationPreferenceRepository>,
    pub password_reset_token_repo: Arc<dyn PasswordResetTokenRepository>,
    pub read_state_repo: Arc<dyn ConversationReadStateRepository>,
    pub link_preview_repo: Arc<dyn LinkPreviewRepository>,
    pub custom_emoji_repo: Arc<dyn CustomEmojiRepository>,
    pub workspace_state_repo: Arc<dyn WorkspaceStateRepository>,
    pub pipeline_repo: Arc<dyn PipelineRepository>,
    pub capability_registry_repo: Arc<dyn CapabilityRegistryRepository>,
    pub moderation_repo: Arc<dyn ModerationRepository>,
    pub notification_repo: Arc<dyn NotificationRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub key_challenge_repo: Arc<dyn KeyChallengeRepository>,
    pub device_link_repo: Arc<dyn DeviceLinkRepository>,
    pub workspace_plugin_repo: Arc<dyn WorkspacePluginRepository>,
    pub e2ee_repo: Arc<dyn E2eeRepository>,
    pub relay_repo: Arc<dyn RelayRepository>,
    pub host_repo: Arc<dyn HostRepository>,
    pub delegation_repo: Arc<dyn DelegationRepository>,
    pub subscription_repo: Arc<dyn SubscriptionRepository>,
    pub relay_abuse_repo: Arc<dyn RelayAbuseRepository>,
}

#[derive(Clone)]
pub struct ConversationRepos {
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

#[derive(Clone)]
pub struct MessageRepos {
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub message_attachment_repo: Arc<dyn MessageAttachmentRepository>,
    pub message_mention_repo: Arc<dyn MessageMentionRepository>,
    pub message_thread_repo: Arc<dyn MessageThreadRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    /// Used to resolve the `from:@username` search operator to a `UserId`.
    pub user_repo: Arc<dyn UserRepository>,
}

#[derive(Clone)]
pub struct KeybindRepos {
    pub keybind_repo: Arc<dyn KeybindRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

#[derive(Clone)]
pub struct ThreadRepos {
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub message_thread_repo: Arc<dyn MessageThreadRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

#[derive(Clone)]
pub struct ServerRepos {
    pub server_repo: Arc<dyn ServerRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
}

#[derive(Clone)]
pub struct ServerMemberRepos {
    pub server_repo: Arc<dyn ServerRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub user_repo: Arc<dyn UserRepository>,
    /// A new member auto-joins the public channels (`util::add_workspace_member`).
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    /// Bans (owner-added members) and active timeouts (`ServerMember.timeoutUntil`).
    pub moderation_repo: Arc<dyn ModerationRepository>,
}

#[derive(Clone)]
pub struct UserRepos {
    pub user_repo: Arc<dyn UserRepository>,
}

#[derive(Clone)]
pub struct AuthRepos {
    pub user_repo: Arc<dyn UserRepository>,
    pub server_repo: Arc<dyn ServerRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub session_repo: Arc<dyn SessionRepository>,
    pub invite_repo: Arc<dyn InviteRepository>,
    pub instance_invite_repo: Arc<dyn InstanceInviteRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    /// Ban checks on the workspace join paths.
    pub moderation_repo: Arc<dyn ModerationRepository>,
}

impl AuthRepos {
    /// The repositories the shared signup helpers (`crate::signup`) check and claim
    /// registration tokens with.
    pub(crate) fn signup_repos(&self) -> crate::signup::SignupRepos {
        crate::signup::SignupRepos {
            instance_invite_repo: Arc::clone(&self.instance_invite_repo),
            invite_repo: Arc::clone(&self.invite_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            moderation_repo: Arc::clone(&self.moderation_repo),
        }
    }
}

#[derive(Clone)]
pub struct CommandRepos {
    pub user_command_repo: Arc<dyn UserCommandRepository>,
    pub macro_repo: Arc<dyn MacroRepository>,
}

#[derive(Clone)]
pub struct AttachmentRepos {
    pub message_attachment_repo: Arc<dyn MessageAttachmentRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

#[derive(Clone)]
pub struct ReactionRepos {
    pub message_reaction_repo: Arc<dyn MessageReactionRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

#[derive(Clone)]
pub struct ReadStateRepos {
    pub read_state_repo: Arc<dyn ConversationReadStateRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    /// Reading a conversation also marks its notifications read.
    pub notification_repo: Arc<dyn NotificationRepository>,
}

#[derive(Clone)]
pub struct MentionRepos {
    pub message_mention_repo: Arc<dyn MessageMentionRepository>,
}

#[derive(Clone)]
pub struct PinRepos {
    pub message_pin_repo: Arc<dyn MessagePinRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

#[derive(Clone)]
pub struct RequestAuthRepos {
    pub session_repo: Arc<dyn SessionRepository>,
}

impl AppRepos {
    pub fn for_channel_lens(&self) -> crate::channel_lens::ChannelLensRepos {
        crate::channel_lens::ChannelLensRepos {
            channel_lens_repo: Arc::clone(&self.channel_lens_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            message_repo: Arc::clone(&self.message_repo),
            message_pin_repo: Arc::clone(&self.message_pin_repo),
        }
    }

    pub fn for_conversation(&self) -> ConversationRepos {
        ConversationRepos {
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_message(&self) -> MessageRepos {
        MessageRepos {
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            message_repo: Arc::clone(&self.message_repo),
            message_attachment_repo: Arc::clone(&self.message_attachment_repo),
            message_mention_repo: Arc::clone(&self.message_mention_repo),
            message_thread_repo: Arc::clone(&self.message_thread_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            user_repo: Arc::clone(&self.user_repo),
        }
    }

    pub fn for_macro(&self) -> crate::macro_def::MacroRepos {
        crate::macro_def::MacroRepos {
            macro_repo: Arc::clone(&self.macro_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_keybind(&self) -> KeybindRepos {
        KeybindRepos {
            keybind_repo: Arc::clone(&self.keybind_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_thread(&self) -> ThreadRepos {
        ThreadRepos {
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            message_repo: Arc::clone(&self.message_repo),
            message_thread_repo: Arc::clone(&self.message_thread_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_server(&self) -> ServerRepos {
        ServerRepos {
            server_repo: Arc::clone(&self.server_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
        }
    }

    pub fn for_server_member(&self) -> ServerMemberRepos {
        ServerMemberRepos {
            server_repo: Arc::clone(&self.server_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            user_repo: Arc::clone(&self.user_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            moderation_repo: Arc::clone(&self.moderation_repo),
        }
    }

    pub fn for_moderation(&self) -> crate::moderation::ModerationRepos {
        crate::moderation::ModerationRepos {
            server_member_repo: Arc::clone(&self.server_member_repo),
            user_repo: Arc::clone(&self.user_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            message_repo: Arc::clone(&self.message_repo),
            message_attachment_repo: Arc::clone(&self.message_attachment_repo),
            moderation_repo: Arc::clone(&self.moderation_repo),
        }
    }

    pub fn for_user(&self) -> UserRepos {
        UserRepos {
            user_repo: Arc::clone(&self.user_repo),
        }
    }

    pub fn for_auth(&self) -> AuthRepos {
        AuthRepos {
            user_repo: Arc::clone(&self.user_repo),
            server_repo: Arc::clone(&self.server_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            session_repo: Arc::clone(&self.session_repo),
            invite_repo: Arc::clone(&self.invite_repo),
            instance_invite_repo: Arc::clone(&self.instance_invite_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            moderation_repo: Arc::clone(&self.moderation_repo),
        }
    }

    pub fn for_key_auth(&self) -> crate::key_auth::KeyAuthRepos {
        crate::key_auth::KeyAuthRepos {
            user_repo: Arc::clone(&self.user_repo),
            session_repo: Arc::clone(&self.session_repo),
            key_identity_repo: Arc::clone(&self.key_identity_repo),
            key_challenge_repo: Arc::clone(&self.key_challenge_repo),
            signup: self.for_auth().signup_repos(),
            host_repo: Arc::clone(&self.host_repo),
            relay_abuse_repo: Arc::clone(&self.relay_abuse_repo),
        }
    }

    pub fn for_relay(&self) -> crate::relay::RelayRepos {
        crate::relay::RelayRepos {
            relay_repo: Arc::clone(&self.relay_repo),
            host_repo: Arc::clone(&self.host_repo),
            key_identity_repo: Arc::clone(&self.key_identity_repo),
            e2ee_repo: Arc::clone(&self.e2ee_repo),
            user_repo: Arc::clone(&self.user_repo),
            subscription_repo: Arc::clone(&self.subscription_repo),
            relay_abuse_repo: Arc::clone(&self.relay_abuse_repo),
        }
    }

    pub fn for_relay_abuse(&self) -> crate::relay::abuse::RelayAbuseRepos {
        crate::relay::abuse::RelayAbuseRepos {
            relay_abuse_repo: Arc::clone(&self.relay_abuse_repo),
            key_identity_repo: Arc::clone(&self.key_identity_repo),
            session_repo: Arc::clone(&self.session_repo),
        }
    }

    pub fn for_host(&self) -> crate::host::HostRepos {
        crate::host::HostRepos {
            host_repo: Arc::clone(&self.host_repo),
            relay_repo: Arc::clone(&self.relay_repo),
            key_identity_repo: Arc::clone(&self.key_identity_repo),
            e2ee_repo: Arc::clone(&self.e2ee_repo),
            user_repo: Arc::clone(&self.user_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            relay_abuse_repo: Arc::clone(&self.relay_abuse_repo),
            delegation_repo: Arc::clone(&self.delegation_repo),
        }
    }

    pub fn for_workspace_host(&self) -> crate::remote_workspace::WorkspaceHostRepos {
        crate::remote_workspace::WorkspaceHostRepos {
            host_repo: Arc::clone(&self.host_repo),
            delegation_repo: Arc::clone(&self.delegation_repo),
            key_identity_repo: Arc::clone(&self.key_identity_repo),
            key_challenge_repo: Arc::clone(&self.key_challenge_repo),
            invite_repo: Arc::clone(&self.invite_repo),
            moderation_repo: Arc::clone(&self.moderation_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            session_repo: Arc::clone(&self.session_repo),
            e2ee_repo: Arc::clone(&self.e2ee_repo),
        }
    }

    pub fn for_workspace_relay(&self) -> crate::remote_workspace::WorkspaceRelayRepos {
        crate::remote_workspace::WorkspaceRelayRepos {
            subscription_repo: Arc::clone(&self.subscription_repo),
            relay_repo: Arc::clone(&self.relay_repo),
            key_identity_repo: Arc::clone(&self.key_identity_repo),
            user_repo: Arc::clone(&self.user_repo),
        }
    }

    pub fn for_e2ee(&self) -> crate::e2ee::E2eeRepos {
        crate::e2ee::E2eeRepos {
            e2ee_repo: Arc::clone(&self.e2ee_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            key_identity_repo: Arc::clone(&self.key_identity_repo),
            message_repo: Arc::clone(&self.message_repo),
            moderation_repo: Arc::clone(&self.moderation_repo),
        }
    }

    pub fn for_device_link(&self) -> crate::device_link::DeviceLinkRepos {
        crate::device_link::DeviceLinkRepos {
            user_repo: Arc::clone(&self.user_repo),
            device_link_repo: Arc::clone(&self.device_link_repo),
        }
    }

    pub fn for_workspace_plugin(&self) -> crate::workspace_plugin::WorkspacePluginRepos {
        crate::workspace_plugin::WorkspacePluginRepos {
            server_member_repo: Arc::clone(&self.server_member_repo),
            key_identity_repo: Arc::clone(&self.key_identity_repo),
            workspace_plugin_repo: Arc::clone(&self.workspace_plugin_repo),
        }
    }

    pub fn for_plugin_bundle(&self) -> crate::plugin_bundle::PluginBundleRepos {
        crate::plugin_bundle::PluginBundleRepos {
            server_member_repo: Arc::clone(&self.server_member_repo),
            workspace_plugin_repo: Arc::clone(&self.workspace_plugin_repo),
        }
    }

    pub fn for_password_reset(&self) -> crate::password_reset::PasswordResetRepos {
        crate::password_reset::PasswordResetRepos {
            user_repo: Arc::clone(&self.user_repo),
            password_reset_token_repo: Arc::clone(&self.password_reset_token_repo),
            session_repo: Arc::clone(&self.session_repo),
        }
    }

    pub fn for_command(&self) -> CommandRepos {
        CommandRepos {
            user_command_repo: Arc::clone(&self.user_command_repo),
            macro_repo: Arc::clone(&self.macro_repo),
        }
    }

    pub fn for_capability(&self) -> crate::capability::CapabilityRepos {
        crate::capability::CapabilityRepos {
            capability_registry_repo: Arc::clone(&self.capability_registry_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_attachment(&self) -> AttachmentRepos {
        AttachmentRepos {
            message_attachment_repo: Arc::clone(&self.message_attachment_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            message_repo: Arc::clone(&self.message_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_reaction(&self) -> ReactionRepos {
        ReactionRepos {
            message_reaction_repo: Arc::clone(&self.message_reaction_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            message_repo: Arc::clone(&self.message_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_read_state(&self) -> ReadStateRepos {
        ReadStateRepos {
            read_state_repo: Arc::clone(&self.read_state_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            notification_repo: Arc::clone(&self.notification_repo),
        }
    }

    pub fn for_mention(&self) -> MentionRepos {
        MentionRepos {
            message_mention_repo: Arc::clone(&self.message_mention_repo),
        }
    }

    pub fn for_notification_preference(
        &self,
    ) -> crate::notification_preference::NotificationPreferenceRepos {
        crate::notification_preference::NotificationPreferenceRepos {
            notification_preference_repo: Arc::clone(&self.notification_preference_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_notification(&self) -> crate::notification::NotificationRepos {
        crate::notification::NotificationRepos {
            notification_repo: Arc::clone(&self.notification_repo),
            notification_preference_repo: Arc::clone(&self.notification_preference_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            message_repo: Arc::clone(&self.message_repo),
        }
    }

    pub fn for_notification_jobs(&self) -> crate::notification::NotificationJobRepos {
        crate::notification::NotificationJobRepos {
            notification_repo: Arc::clone(&self.notification_repo),
            message_repo: Arc::clone(&self.message_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            user_repo: Arc::clone(&self.user_repo),
        }
    }

    pub fn for_pin(&self) -> PinRepos {
        PinRepos {
            message_pin_repo: Arc::clone(&self.message_pin_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            conversation_member_repo: Arc::clone(&self.conversation_member_repo),
            message_repo: Arc::clone(&self.message_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_link_preview(&self) -> Arc<dyn LinkPreviewRepository> {
        Arc::clone(&self.link_preview_repo)
    }

    pub fn for_custom_emoji(&self) -> crate::custom_emoji::CustomEmojiRepos {
        crate::custom_emoji::CustomEmojiRepos {
            custom_emoji_repo: Arc::clone(&self.custom_emoji_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_workspace_state(&self) -> crate::workspace_state::WorkspaceStateRepos {
        crate::workspace_state::WorkspaceStateRepos {
            workspace_state_repo: Arc::clone(&self.workspace_state_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_pipeline(&self) -> crate::pipeline::PipelineRepos {
        crate::pipeline::PipelineRepos {
            pipeline_repo: Arc::clone(&self.pipeline_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
        }
    }

    pub fn for_request_auth(&self) -> RequestAuthRepos {
        RequestAuthRepos {
            session_repo: Arc::clone(&self.session_repo),
        }
    }

    /// Build a complete set of repos from a [`DbHandle`].
    ///
    /// This is used to create repos that share a database transaction, so that
    /// multiple repository calls participate in the same atomic commit.
    pub fn with_handle(handle: &DbHandle) -> Self {
        use rim_infra_db::sea_orm_impl;

        AppRepos {
            instance_invite_repo: Arc::new(
                sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository::new(handle.clone()),
            ),
            channel_lens_repo: Arc::new(
                sea_orm_impl::channel_lens::SeaOrmChannelLensRepository::new(handle.clone()),
            ),
            conversation_repo: Arc::new(
                sea_orm_impl::conversation::SeaOrmConversationRepository::new(handle.clone()),
            ),
            conversation_member_repo: Arc::new(
                sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository::new(
                    handle.clone(),
                ),
            ),
            keybind_repo: Arc::new(sea_orm_impl::keybind::SeaOrmKeybindRepository::new(
                handle.clone(),
            )),
            macro_repo: Arc::new(sea_orm_impl::macro_def::SeaOrmMacroRepository::new(
                handle.clone(),
            )),
            message_repo: Arc::new(sea_orm_impl::message::SeaOrmMessageRepository::new(
                handle.clone(),
            )),
            message_attachment_repo: Arc::new(
                sea_orm_impl::message_attachment::SeaOrmMessageAttachmentRepository::new(
                    handle.clone(),
                ),
            ),
            message_thread_repo: Arc::new(
                sea_orm_impl::message_thread::SeaOrmMessageThreadRepository::new(handle.clone()),
            ),
            user_command_repo: Arc::new(
                sea_orm_impl::user_command::SeaOrmUserCommandRepository::new(handle.clone()),
            ),
            server_repo: Arc::new(sea_orm_impl::server::SeaOrmServerRepository::new(
                handle.clone(),
            )),
            server_member_repo: Arc::new(
                sea_orm_impl::server_member::SeaOrmServerMemberRepository::new(handle.clone()),
            ),
            user_repo: Arc::new(sea_orm_impl::user::SeaOrmUserRepository::new(
                handle.clone(),
            )),
            session_repo: Arc::new(sea_orm_impl::session::SeaOrmSessionRepository::new(
                handle.clone(),
            )),
            invite_repo: Arc::new(sea_orm_impl::invite::SeaOrmInviteRepository::new(
                handle.clone(),
            )),
            message_reaction_repo: Arc::new(
                sea_orm_impl::message_reaction::SeaOrmMessageReactionRepository::new(
                    handle.clone(),
                ),
            ),
            message_mention_repo: Arc::new(
                sea_orm_impl::message_mention::SeaOrmMessageMentionRepository::new(handle.clone()),
            ),
            message_pin_repo: Arc::new(sea_orm_impl::message_pin::SeaOrmMessagePinRepository::new(
                handle.clone(),
            )),
            notification_preference_repo: Arc::new(
                sea_orm_impl::notification_preference::SeaOrmNotificationPreferenceRepository::new(
                    handle.clone(),
                ),
            ),
            password_reset_token_repo: Arc::new(
                sea_orm_impl::password_reset_token::SeaOrmPasswordResetTokenRepository::new(
                    handle.clone(),
                ),
            ),
            read_state_repo: Arc::new(
                sea_orm_impl::conversation_read_state::SeaOrmConversationReadStateRepository::new(
                    handle.clone(),
                ),
            ),
            link_preview_repo: Arc::new(
                sea_orm_impl::link_preview::SeaOrmLinkPreviewRepository::new(handle.clone()),
            ),
            custom_emoji_repo: Arc::new(
                sea_orm_impl::custom_emoji::SeaOrmCustomEmojiRepository::new(handle.clone()),
            ),
            workspace_state_repo: Arc::new(
                sea_orm_impl::workspace_state::SeaOrmWorkspaceStateRepository::new(handle.clone()),
            ),
            pipeline_repo: Arc::new(sea_orm_impl::pipeline::SeaOrmPipelineRepository::new(
                handle.clone(),
            )),
            capability_registry_repo: Arc::new(
                sea_orm_impl::capability_registry::SeaOrmCapabilityRegistryRepository::new(
                    handle.clone(),
                ),
            ),
            moderation_repo: Arc::new(sea_orm_impl::moderation::SeaOrmModerationRepository::new(
                handle.clone(),
            )),
            notification_repo: Arc::new(
                sea_orm_impl::notification::SeaOrmNotificationRepository::new(handle.clone()),
            ),
            key_identity_repo: Arc::new(
                sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository::new(handle.clone()),
            ),
            key_challenge_repo: Arc::new(
                sea_orm_impl::key_identity::SeaOrmKeyChallengeRepository::new(handle.clone()),
            ),
            device_link_repo: Arc::new(sea_orm_impl::device_link::SeaOrmDeviceLinkRepository::new(
                handle.clone(),
            )),
            workspace_plugin_repo: Arc::new(
                sea_orm_impl::workspace_plugin::SeaOrmWorkspacePluginRepository::new(
                    handle.clone(),
                ),
            ),
            e2ee_repo: Arc::new(sea_orm_impl::e2ee::SeaOrmE2eeRepository::new(
                handle.clone(),
            )),
            relay_repo: Arc::new(sea_orm_impl::relay::SeaOrmRelayRepository::new(
                handle.clone(),
            )),
            host_repo: Arc::new(sea_orm_impl::host::SeaOrmHostRepository::new(
                handle.clone(),
            )),
            delegation_repo: Arc::new(
                sea_orm_impl::remote_workspace::SeaOrmDelegationRepository::new(handle.clone()),
            ),
            subscription_repo: Arc::new(
                sea_orm_impl::remote_workspace::SeaOrmSubscriptionRepository::new(handle.clone()),
            ),
            relay_abuse_repo: Arc::new(sea_orm_impl::relay_abuse::SeaOrmRelayAbuseRepository::new(
                handle.clone(),
            )),
        }
    }
}
