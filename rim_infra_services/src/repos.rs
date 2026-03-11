use std::sync::Arc;

use rim_core::auth::repository::{InviteRepository, SessionRepository};
use rim_core::command::repository::UserCommandRepository;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::keybind::repository::KeybindRepository;
use rim_core::message::repository::MessageRepository;
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::repository::UserRepository;

#[derive(Clone)]
pub struct AppRepos {
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub keybind_repo: Arc<dyn KeybindRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub message_attachment_repo: Arc<dyn MessageAttachmentRepository>,
    pub message_thread_repo: Arc<dyn MessageThreadRepository>,
    pub user_command_repo: Arc<dyn UserCommandRepository>,
    pub server_repo: Arc<dyn ServerRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub user_repo: Arc<dyn UserRepository>,
    pub session_repo: Arc<dyn SessionRepository>,
    pub invite_repo: Arc<dyn InviteRepository>,
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
}

#[derive(Clone)]
pub struct ServerMemberRepos {
    pub server_repo: Arc<dyn ServerRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
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
pub struct RequestAuthRepos {
    pub session_repo: Arc<dyn SessionRepository>,
}

impl AppRepos {
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
            message_thread_repo: Arc::clone(&self.message_thread_repo),
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
        }
    }

    pub fn for_server_member(&self) -> ServerMemberRepos {
        ServerMemberRepos {
            server_repo: Arc::clone(&self.server_repo),
            server_member_repo: Arc::clone(&self.server_member_repo),
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

    pub fn for_request_auth(&self) -> RequestAuthRepos {
        RequestAuthRepos {
            session_repo: Arc::clone(&self.session_repo),
        }
    }
}
