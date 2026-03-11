#[cfg(test)]
pub mod auth {
    use async_trait::async_trait;
    use rim_core::auth::model::{Invite, InviteId, NewInvite, NewSession, Session, SessionId};
    use rim_core::repository::error::RepoResult;
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::user::model::{NewUser, User, UserId};

    #[async_trait]
    pub trait UserRepo: Send + Sync {
        async fn any_users_exist(&self) -> RepoResult<bool>;
        async fn create_user(&self, user_data: NewUser) -> RepoResult<User>;
        async fn find_by_username(&self, username: &str) -> RepoResult<Option<User>>;
    }

    #[async_trait]
    pub trait ServerRepo: Send + Sync {
        async fn create_server(&self, name: &str) -> RepoResult<Server>;
    }

    #[async_trait]
    pub trait ServerMemberRepo: Send + Sync {
        async fn get_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
        ) -> RepoResult<Option<ServerMember>>;

        async fn add_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
            role: ServerMemberRole,
        ) -> RepoResult<ServerMember>;
    }

    #[async_trait]
    pub trait SessionRepo: Send + Sync {
        async fn get_by_id(&self, id: SessionId) -> RepoResult<Option<Session>>;
        async fn create_session(&self, session: NewSession) -> RepoResult<Session>;
        async fn revoke_session(&self, id: SessionId) -> RepoResult<()>;
    }

    #[async_trait]
    pub trait InviteRepo: Send + Sync {
        async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Invite>>;
        async fn create_invite(&self, invite: NewInvite) -> RepoResult<Invite>;
        async fn try_consume(&self, id: InviteId) -> RepoResult<bool>;
    }

    mockall::mock! {
        pub UserRepo {}
        #[async_trait]
        impl UserRepo for UserRepo {
            async fn any_users_exist(&self) -> RepoResult<bool>;
            async fn create_user(&self, user_data: NewUser) -> RepoResult<User>;
            async fn find_by_username(&self, username: &str) -> RepoResult<Option<User>>;
        }
    }

    mockall::mock! {
        pub ServerRepo {}
        #[async_trait]
        impl ServerRepo for ServerRepo {
            async fn create_server(&self, name: &str) -> RepoResult<Server>;
        }
    }

    mockall::mock! {
        pub ServerMemberRepo {}
        #[async_trait]
        impl ServerMemberRepo for ServerMemberRepo {
            async fn get_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<Option<ServerMember>>;
            async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<ServerMember>;
        }
    }

    mockall::mock! {
        pub SessionRepo {}
        #[async_trait]
        impl SessionRepo for SessionRepo {
            async fn get_by_id(&self, id: SessionId) -> RepoResult<Option<Session>>;
            async fn create_session(&self, session: NewSession) -> RepoResult<Session>;
            async fn revoke_session(&self, id: SessionId) -> RepoResult<()>;
        }
    }

    mockall::mock! {
        pub InviteRepo {}
        #[async_trait]
        impl InviteRepo for InviteRepo {
            async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Invite>>;
            async fn create_invite(&self, invite: NewInvite) -> RepoResult<Invite>;
            async fn try_consume(&self, id: InviteId) -> RepoResult<bool>;
        }
    }

    #[derive(Clone)]
    pub struct AuthReposPorts {
        pub user_repo: std::sync::Arc<dyn UserRepo>,
        pub server_repo: std::sync::Arc<dyn ServerRepo>,
        pub server_member_repo: std::sync::Arc<dyn ServerMemberRepo>,
        pub session_repo: std::sync::Arc<dyn SessionRepo>,
        pub invite_repo: std::sync::Arc<dyn InviteRepo>,
    }

    pub struct UserRepoPortAdapter(pub std::sync::Arc<dyn UserRepo>);
    pub struct ServerRepoPortAdapter(pub std::sync::Arc<dyn ServerRepo>);
    pub struct ServerMemberRepoPortAdapter(pub std::sync::Arc<dyn ServerMemberRepo>);
    pub struct SessionRepoPortAdapter(pub std::sync::Arc<dyn SessionRepo>);
    pub struct InviteRepoPortAdapter(pub std::sync::Arc<dyn InviteRepo>);

    #[async_trait]
    impl rim_core::user::repository::UserRepository for UserRepoPortAdapter {
        async fn get_by_id(&self, _id: UserId) -> RepoResult<Option<User>> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn create_user(&self, user_data: NewUser) -> RepoResult<User> {
            self.0.create_user(user_data).await
        }

        async fn find_by_username(&self, username: &str) -> RepoResult<Option<User>> {
            self.0.find_by_username(username).await
        }

        async fn delete_by_username(&self, _username: &str) -> RepoResult<()> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn any_users_exist(&self) -> RepoResult<bool> {
            self.0.any_users_exist().await
        }
    }

    #[async_trait]
    impl rim_core::server::repository::ServerRepository for ServerRepoPortAdapter {
        async fn get_by_id(&self, _id: ServerId) -> RepoResult<Option<Server>> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn create_server(&self, name: &str) -> RepoResult<Server> {
            self.0.create_server(name).await
        }

        async fn find_by_name(&self, _name: &str) -> RepoResult<Option<Server>> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn get_servers_for_user(&self, _user_id: UserId) -> RepoResult<Vec<Server>> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }
    }

    #[async_trait]
    impl rim_core::server_member::repository::ServerMemberRepository for ServerMemberRepoPortAdapter {
        async fn get_by_id(&self, _id: ServerMemberId) -> RepoResult<Option<ServerMember>> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn get_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
        ) -> RepoResult<Option<ServerMember>> {
            self.0.get_member(server_id, user_id).await
        }

        async fn add_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
            role: ServerMemberRole,
        ) -> RepoResult<ServerMember> {
            self.0.add_member(server_id, user_id, role).await
        }

        async fn change_member_role(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: ServerMemberRole,
        ) -> RepoResult<()> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn remove_member(&self, _server_id: ServerId, _user_id: UserId) -> RepoResult<()> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn get_members_for_server(
            &self,
            _server_id: ServerId,
        ) -> RepoResult<Vec<ServerMember>> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn is_member(&self, _server_id: ServerId, _user_id: UserId) -> RepoResult<bool> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }
    }

    #[async_trait]
    impl rim_core::auth::repository::SessionRepository for SessionRepoPortAdapter {
        async fn get_by_id(&self, id: SessionId) -> RepoResult<Option<Session>> {
            self.0.get_by_id(id).await
        }

        async fn get_by_token_hash(&self, _token_hash: &str) -> RepoResult<Option<Session>> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn create_session(&self, session: NewSession) -> RepoResult<Session> {
            self.0.create_session(session).await
        }

        async fn touch_last_seen_at(&self, _id: SessionId) -> RepoResult<()> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn revoke_session(&self, id: SessionId) -> RepoResult<()> {
            self.0.revoke_session(id).await
        }

        async fn revoke_all_for_user(&self, _user_id: UserId) -> RepoResult<u64> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }
    }

    #[async_trait]
    impl rim_core::auth::repository::InviteRepository for InviteRepoPortAdapter {
        async fn get_by_id(&self, _id: InviteId) -> RepoResult<Option<Invite>> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }

        async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Invite>> {
            self.0.get_by_token_hash(token_hash).await
        }

        async fn create_invite(&self, invite: NewInvite) -> RepoResult<Invite> {
            self.0.create_invite(invite).await
        }

        async fn try_consume(&self, id: InviteId) -> RepoResult<bool> {
            self.0.try_consume(id).await
        }

        async fn revoke_invite(&self, _id: InviteId) -> RepoResult<()> {
            Err(rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::Unsupported,
            ))
        }
    }
}

#[cfg(test)]
pub mod conversation {
    use async_trait::async_trait;
    use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
    use rim_core::conversation::repository::{
        ConversationActivityCursor, ConversationCreatedCursor,
    };
    use rim_core::conversation_member::model::{ConversationMember, ConversationMemberId};
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use std::collections::HashMap;

    #[async_trait]
    pub trait ConversationRepo: Send + Sync {
        async fn create_conversation(
            &self,
            conversation_type: ConversationType,
            server_id: ServerId,
            title: Option<String>,
        ) -> RepoResult<Conversation>;

        async fn create_conversation_with_members(
            &self,
            conversation_type: ConversationType,
            server_id: ServerId,
            title: Option<String>,
            member_ids: &[UserId],
        ) -> RepoResult<Conversation>;

        async fn find_direct_between_users(
            &self,
            server_id: ServerId,
            user_a: UserId,
            user_b: UserId,
        ) -> RepoResult<Option<Conversation>>;

        async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;

        async fn list_inbox_conversations_for_user(
            &self,
            user_id: UserId,
            server_id: ServerId,
            limit: u32,
            dir: PageDir,
            cursor: Option<ConversationActivityCursor>,
        ) -> RepoResult<Page<Conversation, ConversationActivityCursor>>;

        async fn list_channels_for_user(
            &self,
            user_id: UserId,
            server_id: ServerId,
            limit: u32,
            dir: PageDir,
            cursor: Option<ConversationCreatedCursor>,
        ) -> RepoResult<Page<Conversation, ConversationCreatedCursor>>;
    }

    #[async_trait]
    pub trait ConversationMemberRepo: Send + Sync {
        async fn add_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<ConversationMember>;

        async fn is_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<bool>;

        async fn remove_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<()>;

        async fn list_member_ids(&self, conversation_id: ConversationId)
        -> RepoResult<Vec<UserId>>;

        async fn list_member_ids_for_conversations(
            &self,
            conversation_ids: &[ConversationId],
        ) -> RepoResult<HashMap<ConversationId, Vec<UserId>>>;
    }

    #[async_trait]
    pub trait ServerMemberRepo: Send + Sync {
        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
    }

    mockall::mock! {
        pub ConversationRepo {}
        #[async_trait]
        impl ConversationRepo for ConversationRepo {
            async fn create_conversation(&self, conversation_type: ConversationType, server_id: ServerId, title: Option<String>) -> RepoResult<Conversation>;
            async fn create_conversation_with_members(&self, conversation_type: ConversationType, server_id: ServerId, title: Option<String>, member_ids: &[UserId]) -> RepoResult<Conversation>;
            async fn find_direct_between_users(&self, server_id: ServerId, user_a: UserId, user_b: UserId) -> RepoResult<Option<Conversation>>;
            async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;
            async fn list_inbox_conversations_for_user(&self, user_id: UserId, server_id: ServerId, limit: u32, dir: PageDir, cursor: Option<ConversationActivityCursor>) -> RepoResult<Page<Conversation, ConversationActivityCursor>>;
            async fn list_channels_for_user(&self, user_id: UserId, server_id: ServerId, limit: u32, dir: PageDir, cursor: Option<ConversationCreatedCursor>) -> RepoResult<Page<Conversation, ConversationCreatedCursor>>;
        }
    }

    mockall::mock! {
        pub ConversationMemberRepo {}
        #[async_trait]
        impl ConversationMemberRepo for ConversationMemberRepo {
            async fn add_member(&self, conversation_id: ConversationId, user_id: UserId) -> RepoResult<ConversationMember>;
            async fn is_member(&self, conversation_id: ConversationId, user_id: UserId) -> RepoResult<bool>;
            async fn remove_member(&self, conversation_id: ConversationId, user_id: UserId) -> RepoResult<()>;
            async fn list_member_ids(&self, conversation_id: ConversationId) -> RepoResult<Vec<UserId>>;
            async fn list_member_ids_for_conversations(&self, conversation_ids: &[ConversationId]) -> RepoResult<HashMap<ConversationId, Vec<UserId>>>;
        }
    }

    mockall::mock! {
        pub ServerMemberRepo {}
        #[async_trait]
        impl ServerMemberRepo for ServerMemberRepo {
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
        }
    }

    #[derive(Clone)]
    pub struct ConversationReposPorts {
        pub conversation_repo: std::sync::Arc<dyn ConversationRepo>,
        pub conversation_member_repo: std::sync::Arc<dyn ConversationMemberRepo>,
        pub server_member_repo: std::sync::Arc<dyn ServerMemberRepo>,
    }

    pub struct ConversationRepoPortAdapter(pub std::sync::Arc<dyn ConversationRepo>);
    pub struct ConversationMemberRepoPortAdapter(pub std::sync::Arc<dyn ConversationMemberRepo>);
    pub struct ServerMemberRepoPortAdapter(pub std::sync::Arc<dyn ServerMemberRepo>);

    #[async_trait]
    impl rim_core::conversation::repository::ConversationRepository for ConversationRepoPortAdapter {
        async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>> {
            self.0.get_by_id(id).await
        }

        async fn create_conversation(
            &self,
            conversation_type: ConversationType,
            server_id: ServerId,
            title: Option<String>,
        ) -> RepoResult<Conversation> {
            self.0
                .create_conversation(conversation_type, server_id, title)
                .await
        }

        async fn create_conversation_with_members(
            &self,
            conversation_type: ConversationType,
            server_id: ServerId,
            title: Option<String>,
            member_ids: &[UserId],
        ) -> RepoResult<Conversation> {
            self.0
                .create_conversation_with_members(conversation_type, server_id, title, member_ids)
                .await
        }

        async fn find_direct_between_users(
            &self,
            server_id: ServerId,
            user_a: UserId,
            user_b: UserId,
        ) -> RepoResult<Option<Conversation>> {
            self.0
                .find_direct_between_users(server_id, user_a, user_b)
                .await
        }

        async fn list_inbox_conversations_for_user(
            &self,
            user_id: UserId,
            server_id: ServerId,
            limit: u32,
            dir: PageDir,
            cursor: Option<ConversationActivityCursor>,
        ) -> RepoResult<Page<Conversation, ConversationActivityCursor>> {
            self.0
                .list_inbox_conversations_for_user(user_id, server_id, limit, dir, cursor)
                .await
        }

        async fn list_channels_for_user(
            &self,
            user_id: UserId,
            server_id: ServerId,
            limit: u32,
            dir: PageDir,
            cursor: Option<ConversationCreatedCursor>,
        ) -> RepoResult<Page<Conversation, ConversationCreatedCursor>> {
            self.0
                .list_channels_for_user(user_id, server_id, limit, dir, cursor)
                .await
        }

        async fn touch_last_activity_at(
            &self,
            _conversation_id: ConversationId,
            _at: chrono::DateTime<chrono::Utc>,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::conversation_member::repository::ConversationMemberRepository
        for ConversationMemberRepoPortAdapter
    {
        async fn get_by_id(
            &self,
            _id: ConversationMemberId,
        ) -> RepoResult<Option<ConversationMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<ConversationMember> {
            self.0.add_member(conversation_id, user_id).await
        }

        async fn remove_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<()> {
            self.0.remove_member(conversation_id, user_id).await
        }

        async fn get_members_for_conversation(
            &self,
            _conversation_id: ConversationId,
        ) -> RepoResult<Vec<ConversationMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn is_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<bool> {
            self.0.is_member(conversation_id, user_id).await
        }

        async fn list_member_ids(
            &self,
            conversation_id: ConversationId,
        ) -> RepoResult<Vec<UserId>> {
            self.0.list_member_ids(conversation_id).await
        }

        async fn list_member_ids_for_conversations(
            &self,
            conversation_ids: &[ConversationId],
        ) -> RepoResult<HashMap<ConversationId, Vec<UserId>>> {
            self.0
                .list_member_ids_for_conversations(conversation_ids)
                .await
        }
    }

    #[async_trait]
    impl rim_core::server_member::repository::ServerMemberRepository for ServerMemberRepoPortAdapter {
        async fn get_by_id(
            &self,
            _id: rim_core::server_member::model::ServerMemberId,
        ) -> RepoResult<Option<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
        ) -> RepoResult<Option<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: rim_core::server_member::model::ServerMemberRole,
        ) -> RepoResult<rim_core::server_member::model::ServerMember> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn change_member_role(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: rim_core::server_member::model::ServerMemberRole,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(&self, _server_id: ServerId, _user_id: UserId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_members_for_server(
            &self,
            _server_id: ServerId,
        ) -> RepoResult<Vec<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
            self.0.is_member(server_id, user_id).await
        }
    }
}

#[cfg(test)]
pub mod server {
    use async_trait::async_trait;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::user::model::UserId;

    #[async_trait]
    pub trait ServerRepo: Send + Sync {
        async fn create_server(&self, name: &str) -> RepoResult<Server>;
        async fn get_servers_for_user(&self, user_id: UserId) -> RepoResult<Vec<Server>>;
        async fn get_by_id(&self, id: ServerId) -> RepoResult<Option<Server>>;
    }

    #[async_trait]
    pub trait ServerMemberRepo: Send + Sync {
        async fn add_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
            role: ServerMemberRole,
        ) -> RepoResult<ServerMember>;

        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
    }

    mockall::mock! {
        pub ServerRepo {}
        #[async_trait]
        impl ServerRepo for ServerRepo {
            async fn create_server(&self, name: &str) -> RepoResult<Server>;
            async fn get_servers_for_user(&self, user_id: UserId) -> RepoResult<Vec<Server>>;
            async fn get_by_id(&self, id: ServerId) -> RepoResult<Option<Server>>;
        }
    }

    mockall::mock! {
        pub ServerMemberRepo {}
        #[async_trait]
        impl ServerMemberRepo for ServerMemberRepo {
            async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<ServerMember>;
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
        }
    }

    #[derive(Clone)]
    pub struct ServerReposPorts {
        pub server_repo: std::sync::Arc<dyn ServerRepo>,
        pub server_member_repo: std::sync::Arc<dyn ServerMemberRepo>,
    }

    pub struct ServerRepoPortAdapter(pub std::sync::Arc<dyn ServerRepo>);
    pub struct ServerMemberRepoPortAdapter(pub std::sync::Arc<dyn ServerMemberRepo>);

    #[async_trait]
    impl rim_core::server::repository::ServerRepository for ServerRepoPortAdapter {
        async fn get_by_id(&self, id: ServerId) -> RepoResult<Option<Server>> {
            self.0.get_by_id(id).await
        }

        async fn create_server(&self, name: &str) -> RepoResult<Server> {
            self.0.create_server(name).await
        }

        async fn find_by_name(&self, _name: &str) -> RepoResult<Option<Server>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_servers_for_user(&self, user_id: UserId) -> RepoResult<Vec<Server>> {
            self.0.get_servers_for_user(user_id).await
        }
    }

    #[async_trait]
    impl rim_core::server_member::repository::ServerMemberRepository for ServerMemberRepoPortAdapter {
        async fn get_by_id(&self, _id: ServerMemberId) -> RepoResult<Option<ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
        ) -> RepoResult<Option<ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
            role: ServerMemberRole,
        ) -> RepoResult<ServerMember> {
            self.0.add_member(server_id, user_id, role).await
        }

        async fn change_member_role(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: ServerMemberRole,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(&self, _server_id: ServerId, _user_id: UserId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_members_for_server(
            &self,
            _server_id: ServerId,
        ) -> RepoResult<Vec<ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
            self.0.is_member(server_id, user_id).await
        }
    }
}

#[cfg(test)]
pub mod server_member {
    use async_trait::async_trait;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::user::model::UserId;

    #[async_trait]
    pub trait ServerRepo: Send + Sync {
        async fn get_by_id(&self, id: ServerId) -> RepoResult<Option<Server>>;
    }

    #[async_trait]
    pub trait ServerMemberRepo: Send + Sync {
        async fn get_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
        ) -> RepoResult<Option<ServerMember>>;
        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
        async fn add_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
            role: ServerMemberRole,
        ) -> RepoResult<ServerMember>;
        async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;
        async fn get_members_for_server(
            &self,
            server_id: ServerId,
        ) -> RepoResult<Vec<ServerMember>>;
    }

    mockall::mock! {
        pub ServerRepo {}
        #[async_trait]
        impl ServerRepo for ServerRepo {
            async fn get_by_id(&self, id: ServerId) -> RepoResult<Option<Server>>;
        }
    }

    mockall::mock! {
        pub ServerMemberRepo {}
        #[async_trait]
        impl ServerMemberRepo for ServerMemberRepo {
            async fn get_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<Option<ServerMember>>;
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<ServerMember>;
            async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;
            async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>>;
        }
    }

    #[derive(Clone)]
    pub struct ServerMemberReposPorts {
        pub server_repo: std::sync::Arc<dyn ServerRepo>,
        pub server_member_repo: std::sync::Arc<dyn ServerMemberRepo>,
    }

    pub struct ServerRepoPortAdapter(pub std::sync::Arc<dyn ServerRepo>);
    pub struct ServerMemberRepoPortAdapter(pub std::sync::Arc<dyn ServerMemberRepo>);

    #[async_trait]
    impl rim_core::server::repository::ServerRepository for ServerRepoPortAdapter {
        async fn get_by_id(&self, id: ServerId) -> RepoResult<Option<Server>> {
            self.0.get_by_id(id).await
        }

        async fn create_server(&self, _name: &str) -> RepoResult<Server> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn find_by_name(&self, _name: &str) -> RepoResult<Option<Server>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_servers_for_user(&self, _user_id: UserId) -> RepoResult<Vec<Server>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::server_member::repository::ServerMemberRepository for ServerMemberRepoPortAdapter {
        async fn get_by_id(&self, _id: ServerMemberId) -> RepoResult<Option<ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
        ) -> RepoResult<Option<ServerMember>> {
            self.0.get_member(server_id, user_id).await
        }

        async fn add_member(
            &self,
            server_id: ServerId,
            user_id: UserId,
            role: ServerMemberRole,
        ) -> RepoResult<ServerMember> {
            self.0.add_member(server_id, user_id, role).await
        }

        async fn change_member_role(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: ServerMemberRole,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()> {
            self.0.remove_member(server_id, user_id).await
        }

        async fn get_members_for_server(
            &self,
            server_id: ServerId,
        ) -> RepoResult<Vec<ServerMember>> {
            self.0.get_members_for_server(server_id).await
        }

        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
            self.0.is_member(server_id, user_id).await
        }
    }
}

#[cfg(test)]
pub mod user {
    use async_trait::async_trait;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::user::model::{User, UserId};

    #[async_trait]
    pub trait UserRepo: Send + Sync {
        async fn get_by_id(&self, id: UserId) -> RepoResult<Option<User>>;
    }

    mockall::mock! {
        pub UserRepo {}
        #[async_trait]
        impl UserRepo for UserRepo {
            async fn get_by_id(&self, id: UserId) -> RepoResult<Option<User>>;
        }
    }

    #[derive(Clone)]
    pub struct UserReposPorts {
        pub user_repo: std::sync::Arc<dyn UserRepo>,
    }

    pub struct UserRepoPortAdapter(pub std::sync::Arc<dyn UserRepo>);

    #[async_trait]
    impl rim_core::user::repository::UserRepository for UserRepoPortAdapter {
        async fn get_by_id(&self, id: UserId) -> RepoResult<Option<User>> {
            self.0.get_by_id(id).await
        }

        async fn create_user(
            &self,
            _user_data: rim_core::user::model::NewUser,
        ) -> RepoResult<User> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn find_by_username(&self, _username: &str) -> RepoResult<Option<User>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn delete_by_username(&self, _username: &str) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn any_users_exist(&self) -> RepoResult<bool> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }
}

#[cfg(test)]
pub mod attachment {
    use async_trait::async_trait;
    use rim_core::conversation::model::{Conversation, ConversationId};
    use rim_core::message::model::{Message, MessageId};
    use rim_core::message_attachment::model::{MessageAttachment, MessageAttachmentId};
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use std::collections::HashMap;

    #[async_trait]
    pub trait ConversationRepo: Send + Sync {
        async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;
    }

    #[async_trait]
    pub trait ConversationMemberRepo: Send + Sync {
        async fn is_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<bool>;
    }

    #[async_trait]
    pub trait MessageRepo: Send + Sync {
        async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>>;
        async fn set_has_attachments(
            &self,
            message_id: MessageId,
            has_attachments: bool,
        ) -> RepoResult<Message>;
    }

    #[async_trait]
    pub trait MessageAttachmentRepo: Send + Sync {
        async fn get_by_id(&self, id: MessageAttachmentId)
        -> RepoResult<Option<MessageAttachment>>;
        async fn add_attachment(
            &self,
            message_id: MessageId,
            storage_url: &str,
            file_name: &str,
            file_size: i64,
            mime_type: &str,
            metadata: Option<serde_json::Value>,
        ) -> RepoResult<MessageAttachment>;
        async fn update_attachment(
            &self,
            id: MessageAttachmentId,
            storage_url: String,
            metadata: Option<serde_json::Value>,
        ) -> RepoResult<MessageAttachment>;
        async fn get_attachments_for_message(
            &self,
            message_id: MessageId,
        ) -> RepoResult<Vec<MessageAttachment>>;
        async fn get_attachments_for_messages(
            &self,
            message_ids: &[MessageId],
        ) -> RepoResult<HashMap<MessageId, Vec<MessageAttachment>>>;
        async fn delete_attachment(&self, id: MessageAttachmentId) -> RepoResult<()>;
    }

    #[async_trait]
    pub trait ServerMemberRepo: Send + Sync {
        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
    }

    mockall::mock! {
        pub ConversationRepo {}
        #[async_trait]
        impl ConversationRepo for ConversationRepo {
            async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;
        }
    }

    mockall::mock! {
        pub ConversationMemberRepo {}
        #[async_trait]
        impl ConversationMemberRepo for ConversationMemberRepo {
            async fn is_member(&self, conversation_id: ConversationId, user_id: UserId) -> RepoResult<bool>;
        }
    }

    mockall::mock! {
        pub MessageRepo {}
        #[async_trait]
        impl MessageRepo for MessageRepo {
            async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>>;
            async fn set_has_attachments(&self, message_id: MessageId, has_attachments: bool) -> RepoResult<Message>;
        }
    }

    mockall::mock! {
        pub MessageAttachmentRepo {}
        #[async_trait]
        impl MessageAttachmentRepo for MessageAttachmentRepo {
            async fn get_by_id(&self, id: MessageAttachmentId) -> RepoResult<Option<MessageAttachment>>;
            async fn add_attachment(&self, message_id: MessageId, storage_url: &str, file_name: &str, file_size: i64, mime_type: &str, metadata: Option<serde_json::Value>) -> RepoResult<MessageAttachment>;
            async fn update_attachment(&self, id: MessageAttachmentId, storage_url: String, metadata: Option<serde_json::Value>) -> RepoResult<MessageAttachment>;
            async fn get_attachments_for_message(&self, message_id: MessageId) -> RepoResult<Vec<MessageAttachment>>;
            async fn get_attachments_for_messages(&self, message_ids: &[MessageId]) -> RepoResult<HashMap<MessageId, Vec<MessageAttachment>>>;
            async fn delete_attachment(&self, id: MessageAttachmentId) -> RepoResult<()>;
        }
    }

    mockall::mock! {
        pub ServerMemberRepo {}
        #[async_trait]
        impl ServerMemberRepo for ServerMemberRepo {
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
        }
    }

    #[derive(Clone)]
    pub struct AttachmentReposPorts {
        pub conversation_repo: std::sync::Arc<dyn ConversationRepo>,
        pub conversation_member_repo: std::sync::Arc<dyn ConversationMemberRepo>,
        pub message_repo: std::sync::Arc<dyn MessageRepo>,
        pub message_attachment_repo: std::sync::Arc<dyn MessageAttachmentRepo>,
        pub server_member_repo: std::sync::Arc<dyn ServerMemberRepo>,
    }

    pub struct ConversationRepoPortAdapter(pub std::sync::Arc<dyn ConversationRepo>);
    pub struct ConversationMemberRepoPortAdapter(pub std::sync::Arc<dyn ConversationMemberRepo>);
    pub struct MessageRepoPortAdapter(pub std::sync::Arc<dyn MessageRepo>);
    pub struct MessageAttachmentRepoPortAdapter(pub std::sync::Arc<dyn MessageAttachmentRepo>);
    pub struct ServerMemberRepoPortAdapter(pub std::sync::Arc<dyn ServerMemberRepo>);

    #[async_trait]
    impl rim_core::conversation::repository::ConversationRepository for ConversationRepoPortAdapter {
        async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>> {
            self.0.get_by_id(id).await
        }

        async fn create_conversation(
            &self,
            _conversation_type: rim_core::conversation::model::ConversationType,
            _server_id: ServerId,
            _title: Option<String>,
        ) -> RepoResult<Conversation> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn create_conversation_with_members(
            &self,
            _conversation_type: rim_core::conversation::model::ConversationType,
            _server_id: ServerId,
            _title: Option<String>,
            _member_ids: &[UserId],
        ) -> RepoResult<Conversation> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn find_direct_between_users(
            &self,
            _server_id: ServerId,
            _user_a: UserId,
            _user_b: UserId,
        ) -> RepoResult<Option<Conversation>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_inbox_conversations_for_user(
            &self,
            _user_id: UserId,
            _server_id: ServerId,
            _limit: u32,
            _dir: rim_core::repository::pagination::PageDir,
            _cursor: Option<rim_core::conversation::repository::ConversationActivityCursor>,
        ) -> RepoResult<
            rim_core::repository::pagination::Page<
                Conversation,
                rim_core::conversation::repository::ConversationActivityCursor,
            >,
        > {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_channels_for_user(
            &self,
            _user_id: UserId,
            _server_id: ServerId,
            _limit: u32,
            _dir: rim_core::repository::pagination::PageDir,
            _cursor: Option<rim_core::conversation::repository::ConversationCreatedCursor>,
        ) -> RepoResult<
            rim_core::repository::pagination::Page<
                Conversation,
                rim_core::conversation::repository::ConversationCreatedCursor,
            >,
        > {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn touch_last_activity_at(
            &self,
            _conversation_id: ConversationId,
            _at: chrono::DateTime<chrono::Utc>,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::conversation_member::repository::ConversationMemberRepository
        for ConversationMemberRepoPortAdapter
    {
        async fn get_by_id(
            &self,
            _id: rim_core::conversation_member::model::ConversationMemberId,
        ) -> RepoResult<Option<rim_core::conversation_member::model::ConversationMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_member(
            &self,
            _conversation_id: ConversationId,
            _user_id: UserId,
        ) -> RepoResult<rim_core::conversation_member::model::ConversationMember> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(
            &self,
            _conversation_id: ConversationId,
            _user_id: UserId,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_members_for_conversation(
            &self,
            _conversation_id: ConversationId,
        ) -> RepoResult<Vec<rim_core::conversation_member::model::ConversationMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn is_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<bool> {
            self.0.is_member(conversation_id, user_id).await
        }

        async fn list_member_ids(
            &self,
            _conversation_id: ConversationId,
        ) -> RepoResult<Vec<UserId>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_member_ids_for_conversations(
            &self,
            _conversation_ids: &[ConversationId],
        ) -> RepoResult<std::collections::HashMap<ConversationId, Vec<UserId>>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::message::repository::MessageRepository for MessageRepoPortAdapter {
        async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>> {
            self.0.get_by_id(id).await
        }

        async fn set_has_attachments(
            &self,
            message_id: MessageId,
            has_attachments: bool,
        ) -> RepoResult<Message> {
            self.0
                .set_has_attachments(message_id, has_attachments)
                .await
        }

        async fn get_by_ids(
            &self,
            _ids: &[MessageId],
        ) -> RepoResult<std::collections::HashMap<MessageId, Message>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn create_message(
            &self,
            _new_message: &rim_core::message::model::NewMessage,
        ) -> RepoResult<Message> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_messages(
            &self,
            _conversation_id: ConversationId,
            _thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
            _limit: u32,
            _dir: rim_core::repository::pagination::PageDir,
            _cursor: Option<rim_core::message::repository::MessageCursor>,
            _include_deleted: bool,
            _order: rim_core::repository::pagination::SortOrder,
        ) -> RepoResult<
            rim_core::repository::pagination::Page<
                Message,
                rim_core::message::repository::MessageCursor,
            >,
        > {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn update_message_text(
            &self,
            _message_id: MessageId,
            _new_text: String,
        ) -> RepoResult<Message> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn soft_delete_message(&self, _message_id: MessageId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::message_attachment::repository::MessageAttachmentRepository
        for MessageAttachmentRepoPortAdapter
    {
        async fn get_by_id(
            &self,
            id: MessageAttachmentId,
        ) -> RepoResult<Option<MessageAttachment>> {
            self.0.get_by_id(id).await
        }

        async fn add_attachment(
            &self,
            message_id: rim_core::message::model::MessageId,
            storage_url: &str,
            file_name: &str,
            file_size: i64,
            mime_type: &str,
            metadata: Option<serde_json::Value>,
        ) -> RepoResult<MessageAttachment> {
            self.0
                .add_attachment(
                    message_id,
                    storage_url,
                    file_name,
                    file_size,
                    mime_type,
                    metadata,
                )
                .await
        }

        async fn get_attachments_for_message(
            &self,
            message_id: rim_core::message::model::MessageId,
        ) -> RepoResult<Vec<MessageAttachment>> {
            self.0.get_attachments_for_message(message_id).await
        }

        async fn get_attachments_for_messages(
            &self,
            message_ids: &[MessageId],
        ) -> RepoResult<HashMap<MessageId, Vec<MessageAttachment>>> {
            self.0.get_attachments_for_messages(message_ids).await
        }

        async fn update_attachment(
            &self,
            id: MessageAttachmentId,
            storage_url: String,
            metadata: Option<serde_json::Value>,
        ) -> RepoResult<MessageAttachment> {
            self.0.update_attachment(id, storage_url, metadata).await
        }

        async fn delete_attachment(&self, id: MessageAttachmentId) -> RepoResult<()> {
            self.0.delete_attachment(id).await
        }
    }

    #[async_trait]
    impl rim_core::server_member::repository::ServerMemberRepository for ServerMemberRepoPortAdapter {
        async fn get_by_id(
            &self,
            _id: rim_core::server_member::model::ServerMemberId,
        ) -> RepoResult<Option<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
        ) -> RepoResult<Option<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: rim_core::server_member::model::ServerMemberRole,
        ) -> RepoResult<rim_core::server_member::model::ServerMember> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn change_member_role(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: rim_core::server_member::model::ServerMemberRole,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(&self, _server_id: ServerId, _user_id: UserId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_members_for_server(
            &self,
            _server_id: ServerId,
        ) -> RepoResult<Vec<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
            self.0.is_member(server_id, user_id).await
        }
    }
}

#[cfg(test)]
pub mod message {
    use async_trait::async_trait;
    use rim_core::conversation::model::{Conversation, ConversationId};
    use rim_core::message::model::{Message, MessageId, NewMessage};
    use rim_core::message::repository::MessageCursor;
    use rim_core::message_attachment::model::{MessageAttachment, MessageAttachmentId};
    use rim_core::message_thread::model::{MessageThread, MessageThreadId};
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::repository::pagination::{Page, PageDir, SortOrder};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use std::collections::HashMap;

    #[async_trait]
    pub trait ConversationRepo: Send + Sync {
        async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;
        async fn touch_last_activity_at(
            &self,
            conversation_id: ConversationId,
            at: chrono::DateTime<chrono::Utc>,
        ) -> RepoResult<()>;
    }

    #[async_trait]
    pub trait ConversationMemberRepo: Send + Sync {
        async fn is_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<bool>;
    }

    #[async_trait]
    pub trait MessageRepo: Send + Sync {
        async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>>;
        async fn create_message(&self, new_message: &NewMessage) -> RepoResult<Message>;
        #[allow(clippy::too_many_arguments)]
        async fn list_messages(
            &self,
            conversation_id: ConversationId,
            thread_id: Option<MessageThreadId>,
            limit: u32,
            dir: PageDir,
            cursor: Option<MessageCursor>,
            include_deleted: bool,
            order: SortOrder,
        ) -> RepoResult<Page<Message, MessageCursor>>;
        async fn update_message_text(
            &self,
            message_id: MessageId,
            new_text: String,
        ) -> RepoResult<Message>;
        async fn set_has_attachments(
            &self,
            message_id: MessageId,
            has_attachments: bool,
        ) -> RepoResult<Message>;
        async fn soft_delete_message(&self, message_id: MessageId) -> RepoResult<()>;
    }

    #[async_trait]
    pub trait MessageAttachmentRepo: Send + Sync {
        async fn get_attachments_for_message(
            &self,
            message_id: MessageId,
        ) -> RepoResult<Vec<MessageAttachment>>;
        async fn get_attachments_for_messages(
            &self,
            message_ids: &[MessageId],
        ) -> RepoResult<HashMap<MessageId, Vec<MessageAttachment>>>;
    }

    #[async_trait]
    pub trait ThreadRepo: Send + Sync {
        async fn get_by_id(&self, id: MessageThreadId) -> RepoResult<Option<MessageThread>>;
        async fn get_by_root_message(
            &self,
            message_id: MessageId,
        ) -> RepoResult<Option<MessageThread>>;
        async fn get_thread_ids_by_root_messages(
            &self,
            message_ids: &[MessageId],
        ) -> RepoResult<HashMap<MessageId, MessageThreadId>>;
        async fn create_thread_for_root(
            &self,
            conversation_id: ConversationId,
            message_id: MessageId,
        ) -> RepoResult<MessageThread>;
        async fn set_first_message(
            &self,
            thread_id: MessageThreadId,
            message_id: MessageId,
        ) -> RepoResult<()>;
        async fn touch_last_message_at(
            &self,
            thread_id: MessageThreadId,
            at: chrono::DateTime<chrono::Utc>,
        ) -> RepoResult<()>;
    }

    #[async_trait]
    pub trait ServerMemberRepo: Send + Sync {
        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
    }

    mockall::mock! {
        pub ConversationRepo {}
        #[async_trait]
        impl ConversationRepo for ConversationRepo {
            async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;
            async fn touch_last_activity_at(&self, conversation_id: ConversationId, at: chrono::DateTime<chrono::Utc>) -> RepoResult<()>;
        }
    }

    mockall::mock! {
        pub ConversationMemberRepo {}
        #[async_trait]
        impl ConversationMemberRepo for ConversationMemberRepo {
            async fn is_member(&self, conversation_id: ConversationId, user_id: UserId) -> RepoResult<bool>;
        }
    }

    mockall::mock! {
        pub MessageRepo {}
        #[async_trait]
        impl MessageRepo for MessageRepo {
            async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>>;
            async fn create_message(&self, new_message: &NewMessage) -> RepoResult<Message>;
            async fn list_messages(&self, conversation_id: ConversationId, thread_id: Option<MessageThreadId>, limit: u32, dir: PageDir, cursor: Option<MessageCursor>, include_deleted: bool, order: SortOrder) -> RepoResult<Page<Message, MessageCursor>>;
            async fn update_message_text(&self, message_id: MessageId, new_text: String) -> RepoResult<Message>;
            async fn set_has_attachments(&self, message_id: MessageId, has_attachments: bool) -> RepoResult<Message>;
            async fn soft_delete_message(&self, message_id: MessageId) -> RepoResult<()>;
        }
    }

    mockall::mock! {
        pub MessageAttachmentRepo {}
        #[async_trait]
        impl MessageAttachmentRepo for MessageAttachmentRepo {
            async fn get_attachments_for_message(&self, message_id: MessageId) -> RepoResult<Vec<MessageAttachment>>;
            async fn get_attachments_for_messages(&self, message_ids: &[MessageId]) -> RepoResult<HashMap<MessageId, Vec<MessageAttachment>>>;
        }
    }

    mockall::mock! {
        pub ThreadRepo {}
        #[async_trait]
        impl ThreadRepo for ThreadRepo {
            async fn get_by_id(&self, id: MessageThreadId) -> RepoResult<Option<MessageThread>>;
            async fn get_by_root_message(&self, message_id: MessageId) -> RepoResult<Option<MessageThread>>;
            async fn get_thread_ids_by_root_messages(&self, message_ids: &[MessageId]) -> RepoResult<HashMap<MessageId, MessageThreadId>>;
            async fn create_thread_for_root(&self, conversation_id: ConversationId, message_id: MessageId) -> RepoResult<MessageThread>;
            async fn set_first_message(&self, thread_id: MessageThreadId, message_id: MessageId) -> RepoResult<()>;
            async fn touch_last_message_at(&self, thread_id: MessageThreadId, at: chrono::DateTime<chrono::Utc>) -> RepoResult<()>;
        }
    }

    mockall::mock! {
        pub ServerMemberRepo {}
        #[async_trait]
        impl ServerMemberRepo for ServerMemberRepo {
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
        }
    }

    #[derive(Clone)]
    pub struct MessageReposPorts {
        pub conversation_repo: std::sync::Arc<dyn ConversationRepo>,
        pub conversation_member_repo: std::sync::Arc<dyn ConversationMemberRepo>,
        pub message_repo: std::sync::Arc<dyn MessageRepo>,
        pub message_attachment_repo: std::sync::Arc<dyn MessageAttachmentRepo>,
        pub message_thread_repo: std::sync::Arc<dyn ThreadRepo>,
        pub server_member_repo: std::sync::Arc<dyn ServerMemberRepo>,
    }

    pub struct ConversationRepoPortAdapter(pub std::sync::Arc<dyn ConversationRepo>);
    pub struct ConversationMemberRepoPortAdapter(pub std::sync::Arc<dyn ConversationMemberRepo>);
    pub struct MessageRepoPortAdapter(pub std::sync::Arc<dyn MessageRepo>);
    pub struct MessageAttachmentRepoPortAdapter(pub std::sync::Arc<dyn MessageAttachmentRepo>);
    pub struct ThreadRepoPortAdapter(pub std::sync::Arc<dyn ThreadRepo>);
    pub struct ServerMemberRepoPortAdapter(pub std::sync::Arc<dyn ServerMemberRepo>);

    #[async_trait]
    impl rim_core::conversation::repository::ConversationRepository for ConversationRepoPortAdapter {
        async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>> {
            self.0.get_by_id(id).await
        }

        async fn create_conversation(
            &self,
            _conversation_type: rim_core::conversation::model::ConversationType,
            _server_id: ServerId,
            _title: Option<String>,
        ) -> RepoResult<Conversation> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn create_conversation_with_members(
            &self,
            _conversation_type: rim_core::conversation::model::ConversationType,
            _server_id: ServerId,
            _title: Option<String>,
            _member_ids: &[UserId],
        ) -> RepoResult<Conversation> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn find_direct_between_users(
            &self,
            _server_id: ServerId,
            _user_a: UserId,
            _user_b: UserId,
        ) -> RepoResult<Option<Conversation>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_inbox_conversations_for_user(
            &self,
            _user_id: UserId,
            _server_id: ServerId,
            _limit: u32,
            _dir: PageDir,
            _cursor: Option<rim_core::conversation::repository::ConversationActivityCursor>,
        ) -> RepoResult<
            Page<Conversation, rim_core::conversation::repository::ConversationActivityCursor>,
        > {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_channels_for_user(
            &self,
            _user_id: UserId,
            _server_id: ServerId,
            _limit: u32,
            _dir: PageDir,
            _cursor: Option<rim_core::conversation::repository::ConversationCreatedCursor>,
        ) -> RepoResult<
            Page<Conversation, rim_core::conversation::repository::ConversationCreatedCursor>,
        > {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn touch_last_activity_at(
            &self,
            conversation_id: ConversationId,
            at: chrono::DateTime<chrono::Utc>,
        ) -> RepoResult<()> {
            self.0.touch_last_activity_at(conversation_id, at).await
        }
    }

    #[async_trait]
    impl rim_core::conversation_member::repository::ConversationMemberRepository
        for ConversationMemberRepoPortAdapter
    {
        async fn get_by_id(
            &self,
            _id: rim_core::conversation_member::model::ConversationMemberId,
        ) -> RepoResult<Option<rim_core::conversation_member::model::ConversationMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_member(
            &self,
            _conversation_id: ConversationId,
            _user_id: UserId,
        ) -> RepoResult<rim_core::conversation_member::model::ConversationMember> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(
            &self,
            _conversation_id: ConversationId,
            _user_id: UserId,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_members_for_conversation(
            &self,
            _conversation_id: ConversationId,
        ) -> RepoResult<Vec<rim_core::conversation_member::model::ConversationMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn is_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<bool> {
            self.0.is_member(conversation_id, user_id).await
        }

        async fn list_member_ids(
            &self,
            _conversation_id: ConversationId,
        ) -> RepoResult<Vec<UserId>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_member_ids_for_conversations(
            &self,
            _conversation_ids: &[ConversationId],
        ) -> RepoResult<HashMap<ConversationId, Vec<UserId>>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::message::repository::MessageRepository for MessageRepoPortAdapter {
        async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>> {
            self.0.get_by_id(id).await
        }

        async fn get_by_ids(&self, ids: &[MessageId]) -> RepoResult<HashMap<MessageId, Message>> {
            let _ = ids;
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn create_message(&self, new_message: &NewMessage) -> RepoResult<Message> {
            self.0.create_message(new_message).await
        }

        async fn list_messages(
            &self,
            conversation_id: ConversationId,
            thread_id: Option<MessageThreadId>,
            limit: u32,
            dir: PageDir,
            cursor: Option<MessageCursor>,
            include_deleted: bool,
            order: SortOrder,
        ) -> RepoResult<Page<Message, MessageCursor>> {
            self.0
                .list_messages(
                    conversation_id,
                    thread_id,
                    limit,
                    dir,
                    cursor,
                    include_deleted,
                    order,
                )
                .await
        }

        async fn update_message_text(
            &self,
            message_id: MessageId,
            new_text: String,
        ) -> RepoResult<Message> {
            self.0.update_message_text(message_id, new_text).await
        }

        async fn set_has_attachments(
            &self,
            message_id: MessageId,
            has_attachments: bool,
        ) -> RepoResult<Message> {
            self.0
                .set_has_attachments(message_id, has_attachments)
                .await
        }

        async fn soft_delete_message(&self, message_id: MessageId) -> RepoResult<()> {
            self.0.soft_delete_message(message_id).await
        }
    }

    #[async_trait]
    impl rim_core::message_attachment::repository::MessageAttachmentRepository
        for MessageAttachmentRepoPortAdapter
    {
        async fn get_by_id(
            &self,
            _id: MessageAttachmentId,
        ) -> RepoResult<Option<MessageAttachment>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_attachment(
            &self,
            _message_id: MessageId,
            _storage_url: &str,
            _file_name: &str,
            _file_size: i64,
            _mime_type: &str,
            _metadata: Option<serde_json::Value>,
        ) -> RepoResult<MessageAttachment> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_attachments_for_message(
            &self,
            message_id: MessageId,
        ) -> RepoResult<Vec<MessageAttachment>> {
            self.0.get_attachments_for_message(message_id).await
        }

        async fn get_attachments_for_messages(
            &self,
            message_ids: &[MessageId],
        ) -> RepoResult<HashMap<MessageId, Vec<MessageAttachment>>> {
            self.0.get_attachments_for_messages(message_ids).await
        }

        async fn update_attachment(
            &self,
            _id: MessageAttachmentId,
            _storage_url: String,
            _metadata: Option<serde_json::Value>,
        ) -> RepoResult<MessageAttachment> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn delete_attachment(&self, _id: MessageAttachmentId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::message_thread::repository::MessageThreadRepository for ThreadRepoPortAdapter {
        async fn get_by_id(&self, id: MessageThreadId) -> RepoResult<Option<MessageThread>> {
            self.0.get_by_id(id).await
        }

        async fn get_by_root_message(
            &self,
            message_id: MessageId,
        ) -> RepoResult<Option<MessageThread>> {
            self.0.get_by_root_message(message_id).await
        }

        async fn get_thread_ids_by_root_messages(
            &self,
            message_ids: &[MessageId],
        ) -> RepoResult<HashMap<MessageId, MessageThreadId>> {
            self.0.get_thread_ids_by_root_messages(message_ids).await
        }

        async fn create_thread_for_root(
            &self,
            conversation_id: ConversationId,
            message_id: MessageId,
        ) -> RepoResult<MessageThread> {
            self.0
                .create_thread_for_root(conversation_id, message_id)
                .await
        }

        async fn create_thread(
            &self,
            _conversation_id: ConversationId,
        ) -> RepoResult<MessageThread> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn set_first_message(
            &self,
            thread_id: MessageThreadId,
            message_id: MessageId,
        ) -> RepoResult<()> {
            self.0.set_first_message(thread_id, message_id).await
        }

        async fn touch_last_message_at(
            &self,
            thread_id: MessageThreadId,
            at: chrono::DateTime<chrono::Utc>,
        ) -> RepoResult<()> {
            self.0.touch_last_message_at(thread_id, at).await
        }

        async fn list_threads_for_conversation(
            &self,
            _conversation_id: ConversationId,
            _limit: u32,
            _dir: PageDir,
            _cursor: Option<rim_core::message_thread::repository::ThreadCursor>,
        ) -> RepoResult<Page<MessageThread, rim_core::message_thread::repository::ThreadCursor>>
        {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::server_member::repository::ServerMemberRepository for ServerMemberRepoPortAdapter {
        async fn get_by_id(
            &self,
            _id: rim_core::server_member::model::ServerMemberId,
        ) -> RepoResult<Option<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
        ) -> RepoResult<Option<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: rim_core::server_member::model::ServerMemberRole,
        ) -> RepoResult<rim_core::server_member::model::ServerMember> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn change_member_role(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: rim_core::server_member::model::ServerMemberRole,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(&self, _server_id: ServerId, _user_id: UserId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_members_for_server(
            &self,
            _server_id: ServerId,
        ) -> RepoResult<Vec<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
            self.0.is_member(server_id, user_id).await
        }
    }
}

#[cfg(test)]
pub mod thread {
    use async_trait::async_trait;
    use rim_core::conversation::model::{Conversation, ConversationId};
    use rim_core::message::model::{Message, MessageId};
    use rim_core::message_thread::model::{MessageThread, MessageThreadId};
    use rim_core::message_thread::repository::ThreadCursor;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use std::collections::HashMap;

    #[async_trait]
    pub trait ConversationRepo: Send + Sync {
        async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;
    }

    #[async_trait]
    pub trait ConversationMemberRepo: Send + Sync {
        async fn is_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<bool>;
    }

    #[async_trait]
    pub trait MessageRepo: Send + Sync {
        async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>>;
        async fn get_by_ids(&self, ids: &[MessageId]) -> RepoResult<HashMap<MessageId, Message>>;
        async fn set_has_attachments(
            &self,
            message_id: MessageId,
            has_attachments: bool,
        ) -> RepoResult<Message>;
    }

    #[async_trait]
    pub trait ThreadRepo: Send + Sync {
        async fn get_by_id(&self, id: MessageThreadId) -> RepoResult<Option<MessageThread>>;
        async fn get_by_root_message(
            &self,
            message_id: MessageId,
        ) -> RepoResult<Option<MessageThread>>;
        async fn get_thread_ids_by_root_messages(
            &self,
            message_ids: &[MessageId],
        ) -> RepoResult<HashMap<MessageId, MessageThreadId>>;
        async fn create_thread(&self, conversation_id: ConversationId)
        -> RepoResult<MessageThread>;
        async fn create_thread_for_root(
            &self,
            conversation_id: ConversationId,
            message_id: MessageId,
        ) -> RepoResult<MessageThread>;
        async fn set_first_message(
            &self,
            thread_id: MessageThreadId,
            message_id: MessageId,
        ) -> RepoResult<()>;
        async fn list_threads_for_conversation(
            &self,
            conversation_id: ConversationId,
            limit: u32,
            dir: PageDir,
            cursor: Option<ThreadCursor>,
        ) -> RepoResult<Page<MessageThread, ThreadCursor>>;
    }

    #[async_trait]
    pub trait ServerMemberRepo: Send + Sync {
        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
    }

    mockall::mock! {
        pub ConversationRepo {}
        #[async_trait]
        impl ConversationRepo for ConversationRepo {
            async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;
        }
    }

    mockall::mock! {
        pub ConversationMemberRepo {}
        #[async_trait]
        impl ConversationMemberRepo for ConversationMemberRepo {
            async fn is_member(&self, conversation_id: ConversationId, user_id: UserId) -> RepoResult<bool>;
        }
    }

    mockall::mock! {
        pub MessageRepo {}
        #[async_trait]
        impl MessageRepo for MessageRepo {
            async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>>;
            async fn get_by_ids(&self, ids: &[MessageId]) -> RepoResult<HashMap<MessageId, Message>>;
            async fn set_has_attachments(&self, message_id: MessageId, has_attachments: bool) -> RepoResult<Message>;
        }
    }

    mockall::mock! {
        pub ThreadRepo {}
        #[async_trait]
        impl ThreadRepo for ThreadRepo {
            async fn get_by_id(&self, id: MessageThreadId) -> RepoResult<Option<MessageThread>>;
            async fn get_by_root_message(&self, message_id: MessageId) -> RepoResult<Option<MessageThread>>;
            async fn get_thread_ids_by_root_messages(&self, message_ids: &[MessageId]) -> RepoResult<HashMap<MessageId, MessageThreadId>>;
            async fn create_thread(&self, conversation_id: ConversationId) -> RepoResult<MessageThread>;
            async fn create_thread_for_root(&self, conversation_id: ConversationId, message_id: MessageId) -> RepoResult<MessageThread>;
            async fn set_first_message(&self, thread_id: MessageThreadId, message_id: MessageId) -> RepoResult<()>;
            async fn list_threads_for_conversation(&self, conversation_id: ConversationId, limit: u32, dir: PageDir, cursor: Option<ThreadCursor>) -> RepoResult<Page<MessageThread, ThreadCursor>>;
        }
    }

    mockall::mock! {
        pub ServerMemberRepo {}
        #[async_trait]
        impl ServerMemberRepo for ServerMemberRepo {
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
        }
    }

    #[derive(Clone)]
    pub struct ThreadReposPorts {
        pub conversation_repo: std::sync::Arc<dyn ConversationRepo>,
        pub conversation_member_repo: std::sync::Arc<dyn ConversationMemberRepo>,
        pub message_repo: std::sync::Arc<dyn MessageRepo>,
        pub message_thread_repo: std::sync::Arc<dyn ThreadRepo>,
        pub server_member_repo: std::sync::Arc<dyn ServerMemberRepo>,
    }

    pub struct ConversationRepoPortAdapter(pub std::sync::Arc<dyn ConversationRepo>);
    pub struct ConversationMemberRepoPortAdapter(pub std::sync::Arc<dyn ConversationMemberRepo>);
    pub struct MessageRepoPortAdapter(pub std::sync::Arc<dyn MessageRepo>);
    pub struct ThreadRepoPortAdapter(pub std::sync::Arc<dyn ThreadRepo>);
    pub struct ServerMemberRepoPortAdapter(pub std::sync::Arc<dyn ServerMemberRepo>);

    #[async_trait]
    impl rim_core::conversation::repository::ConversationRepository for ConversationRepoPortAdapter {
        async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>> {
            self.0.get_by_id(id).await
        }

        async fn create_conversation(
            &self,
            _conversation_type: rim_core::conversation::model::ConversationType,
            _server_id: ServerId,
            _title: Option<String>,
        ) -> RepoResult<Conversation> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn create_conversation_with_members(
            &self,
            _conversation_type: rim_core::conversation::model::ConversationType,
            _server_id: ServerId,
            _title: Option<String>,
            _member_ids: &[UserId],
        ) -> RepoResult<Conversation> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn find_direct_between_users(
            &self,
            _server_id: ServerId,
            _user_a: UserId,
            _user_b: UserId,
        ) -> RepoResult<Option<Conversation>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_inbox_conversations_for_user(
            &self,
            _user_id: UserId,
            _server_id: ServerId,
            _limit: u32,
            _dir: PageDir,
            _cursor: Option<rim_core::conversation::repository::ConversationActivityCursor>,
        ) -> RepoResult<
            Page<Conversation, rim_core::conversation::repository::ConversationActivityCursor>,
        > {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_channels_for_user(
            &self,
            _user_id: UserId,
            _server_id: ServerId,
            _limit: u32,
            _dir: PageDir,
            _cursor: Option<rim_core::conversation::repository::ConversationCreatedCursor>,
        ) -> RepoResult<
            Page<Conversation, rim_core::conversation::repository::ConversationCreatedCursor>,
        > {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn touch_last_activity_at(
            &self,
            _conversation_id: ConversationId,
            _at: chrono::DateTime<chrono::Utc>,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::conversation_member::repository::ConversationMemberRepository
        for ConversationMemberRepoPortAdapter
    {
        async fn get_by_id(
            &self,
            _id: rim_core::conversation_member::model::ConversationMemberId,
        ) -> RepoResult<Option<rim_core::conversation_member::model::ConversationMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_member(
            &self,
            _conversation_id: ConversationId,
            _user_id: UserId,
        ) -> RepoResult<rim_core::conversation_member::model::ConversationMember> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(
            &self,
            _conversation_id: ConversationId,
            _user_id: UserId,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_members_for_conversation(
            &self,
            _conversation_id: ConversationId,
        ) -> RepoResult<Vec<rim_core::conversation_member::model::ConversationMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn is_member(
            &self,
            conversation_id: ConversationId,
            user_id: UserId,
        ) -> RepoResult<bool> {
            self.0.is_member(conversation_id, user_id).await
        }

        async fn list_member_ids(
            &self,
            _conversation_id: ConversationId,
        ) -> RepoResult<Vec<UserId>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_member_ids_for_conversations(
            &self,
            _conversation_ids: &[ConversationId],
        ) -> RepoResult<HashMap<ConversationId, Vec<UserId>>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::message::repository::MessageRepository for MessageRepoPortAdapter {
        async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>> {
            self.0.get_by_id(id).await
        }

        async fn get_by_ids(&self, ids: &[MessageId]) -> RepoResult<HashMap<MessageId, Message>> {
            self.0.get_by_ids(ids).await
        }

        async fn set_has_attachments(
            &self,
            message_id: MessageId,
            has_attachments: bool,
        ) -> RepoResult<Message> {
            self.0
                .set_has_attachments(message_id, has_attachments)
                .await
        }

        async fn create_message(
            &self,
            _new_message: &rim_core::message::model::NewMessage,
        ) -> RepoResult<Message> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_messages(
            &self,
            _conversation_id: ConversationId,
            _thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
            _limit: u32,
            _dir: PageDir,
            _cursor: Option<rim_core::message::repository::MessageCursor>,
            _include_deleted: bool,
            _order: rim_core::repository::pagination::SortOrder,
        ) -> RepoResult<Page<Message, rim_core::message::repository::MessageCursor>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn update_message_text(
            &self,
            _message_id: MessageId,
            _new_text: String,
        ) -> RepoResult<Message> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn soft_delete_message(&self, _message_id: MessageId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    #[async_trait]
    impl rim_core::message_thread::repository::MessageThreadRepository for ThreadRepoPortAdapter {
        async fn get_by_id(&self, id: MessageThreadId) -> RepoResult<Option<MessageThread>> {
            self.0.get_by_id(id).await
        }

        async fn get_by_root_message(
            &self,
            message_id: MessageId,
        ) -> RepoResult<Option<MessageThread>> {
            self.0.get_by_root_message(message_id).await
        }

        async fn get_thread_ids_by_root_messages(
            &self,
            message_ids: &[MessageId],
        ) -> RepoResult<HashMap<MessageId, MessageThreadId>> {
            self.0.get_thread_ids_by_root_messages(message_ids).await
        }

        async fn create_thread(
            &self,
            conversation_id: ConversationId,
        ) -> RepoResult<MessageThread> {
            self.0.create_thread(conversation_id).await
        }

        async fn create_thread_for_root(
            &self,
            conversation_id: ConversationId,
            message_id: MessageId,
        ) -> RepoResult<MessageThread> {
            self.0
                .create_thread_for_root(conversation_id, message_id)
                .await
        }

        async fn set_first_message(
            &self,
            thread_id: MessageThreadId,
            message_id: MessageId,
        ) -> RepoResult<()> {
            self.0.set_first_message(thread_id, message_id).await
        }

        async fn touch_last_message_at(
            &self,
            _thread_id: MessageThreadId,
            _at: chrono::DateTime<chrono::Utc>,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn list_threads_for_conversation(
            &self,
            conversation_id: ConversationId,
            limit: u32,
            dir: PageDir,
            cursor: Option<ThreadCursor>,
        ) -> RepoResult<Page<MessageThread, ThreadCursor>> {
            self.0
                .list_threads_for_conversation(conversation_id, limit, dir, cursor)
                .await
        }
    }

    #[async_trait]
    impl rim_core::server_member::repository::ServerMemberRepository for ServerMemberRepoPortAdapter {
        async fn get_by_id(
            &self,
            _id: rim_core::server_member::model::ServerMemberId,
        ) -> RepoResult<Option<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
        ) -> RepoResult<Option<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn add_member(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: rim_core::server_member::model::ServerMemberRole,
        ) -> RepoResult<rim_core::server_member::model::ServerMember> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn change_member_role(
            &self,
            _server_id: ServerId,
            _user_id: UserId,
            _role: rim_core::server_member::model::ServerMemberRole,
        ) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn remove_member(&self, _server_id: ServerId, _user_id: UserId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_members_for_server(
            &self,
            _server_id: ServerId,
        ) -> RepoResult<Vec<rim_core::server_member::model::ServerMember>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
            self.0.is_member(server_id, user_id).await
        }
    }
}
