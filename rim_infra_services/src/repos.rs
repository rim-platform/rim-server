use std::sync::Arc;

use rim_core::auth::repository::{InviteRepository, SessionRepository};
use rim_core::channel_lens::repository::ChannelLensRepository;
use rim_core::command::repository::UserCommandRepository;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::conversation_read_state::repository::ConversationReadStateRepository;
use rim_core::custom_emoji::repository::CustomEmojiRepository;
use rim_core::incident::repository::{
    IncidentMessageTagRepository, IncidentPhaseRepository, IncidentRepository,
};
use rim_core::keybind::repository::KeybindRepository;
use rim_core::link_preview::repository::LinkPreviewRepository;
use rim_core::macro_def::repository::MacroRepository;
use rim_core::message::repository::MessageRepository;
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::message_mention::repository::MessageMentionRepository;
use rim_core::message_pin::repository::MessagePinRepository;
use rim_core::message_reaction::repository::MessageReactionRepository;
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::notification_preference::repository::NotificationPreferenceRepository;
use rim_core::password_reset::repository::PasswordResetTokenRepository;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::skill_profile::repository::SkillProfileRepository;
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;

#[derive(Clone)]
pub struct AppRepos {
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
    pub incident_repo: Arc<dyn IncidentRepository>,
    pub incident_phase_repo: Arc<dyn IncidentPhaseRepository>,
    pub incident_message_tag_repo: Arc<dyn IncidentMessageTagRepository>,
    pub skill_profile_repo: Arc<dyn SkillProfileRepository>,
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
}

#[derive(Clone)]
pub struct ServerMemberRepos {
    pub server_repo: Arc<dyn ServerRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub user_repo: Arc<dyn UserRepository>,
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
    pub conversation_repo: Arc<dyn ConversationRepository>,
}

#[derive(Clone)]
pub struct CommandRepos {
    pub user_command_repo: Arc<dyn UserCommandRepository>,
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
}

#[derive(Clone)]
pub struct MentionRepos {
    pub message_mention_repo: Arc<dyn MessageMentionRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
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
        }
    }

    pub fn for_macro(&self) -> crate::macro_def::MacroRepos {
        crate::macro_def::MacroRepos {
            macro_repo: Arc::clone(&self.macro_repo),
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
        }
    }

    pub fn for_server_member(&self) -> ServerMemberRepos {
        ServerMemberRepos {
            server_repo: Arc::clone(&self.server_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
            user_repo: Arc::clone(&self.user_repo),
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
            conversation_repo: Arc::clone(&self.conversation_repo),
        }
    }

    pub fn for_command(&self) -> CommandRepos {
        CommandRepos {
            user_command_repo: Arc::clone(&self.user_command_repo),
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
        }
    }

    pub fn for_mention(&self) -> MentionRepos {
        MentionRepos {
            message_mention_repo: Arc::clone(&self.message_mention_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
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

    pub fn for_incident(&self) -> crate::incident::IncidentRepos {
        crate::incident::IncidentRepos {
            incident_repo: Arc::clone(&self.incident_repo),
            incident_phase_repo: Arc::clone(&self.incident_phase_repo),
            incident_message_tag_repo: Arc::clone(&self.incident_message_tag_repo),
            conversation_repo: Arc::clone(&self.conversation_repo),
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
            incident_repo: Arc::new(
                sea_orm_impl::incident::SeaOrmIncidentRepository::new(handle.clone()),
            ),
            incident_phase_repo: Arc::new(
                sea_orm_impl::incident_phase::SeaOrmIncidentPhaseRepository::new(handle.clone()),
            ),
            incident_message_tag_repo: Arc::new(
                sea_orm_impl::incident_message_tag::SeaOrmIncidentMessageTagRepository::new(
                    handle.clone(),
                ),
            ),
            skill_profile_repo: Arc::new(
                sea_orm_impl::skill_profile::SeaOrmSkillProfileRepository::new(handle.clone()),
            ),
        }
    }
}
