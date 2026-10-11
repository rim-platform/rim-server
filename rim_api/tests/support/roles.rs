/// A workspace seeded with one caller per role, plus an outsider who owns a second workspace.
use serde_json::json;

use rim_api::AppSchema;
use rim_core::server_member::model::ServerMemberRole;

use super::TestCtx;
use super::graphql::{Caller, exec_ok};

const CREATE_CHANNEL: &str = r#"
mutation CreateChannel($input: CreateConversationInput!) {
  conversation: createConversation(input: $input) { id }
}
"#;

/// The callers of a seeded workspace and the ids of both workspaces.
pub struct Roles {
    pub server_id: String,
    /// The workspace `olive` owns; nobody else belongs to it.
    pub other_server_id: String,
    /// Owner.
    pub alice: Caller,
    /// Moderator.
    pub mona: Caller,
    /// Member.
    pub bob: Caller,
    /// Second member.
    pub carol: Caller,
    /// Guest.
    pub gina: Caller,
    /// Not a member of the workspace; owner of the other one.
    pub olive: Caller,
}

impl TestCtx {
    /// Seed a workspace with alice (Owner), mona (Moderator), bob and carol (Member) and gina
    /// (Guest), and a second workspace owned by olive. Usernames are prefixed with `label`.
    pub async fn roles(&self, label: &str) -> Roles {
        let server = self.create_server(&format!("{label}-e2e")).await;
        let other = self.create_server(&format!("{label}-e2e-other")).await;
        let user = |name: &str| format!("{label}-{name}");
        let alice = self.create_user(&user("alice")).await;
        let mona = self.create_user(&user("mona")).await;
        let bob = self.create_user(&user("bob")).await;
        let carol = self.create_user(&user("carol")).await;
        let gina = self.create_user(&user("gina")).await;
        let olive = self.create_user(&user("olive")).await;
        self.add_owner(&server, &alice).await;
        self.add_member(&server, &mona, ServerMemberRole::Moderator)
            .await;
        self.add_member(&server, &bob, ServerMemberRole::Member)
            .await;
        self.add_member(&server, &carol, ServerMemberRole::Member)
            .await;
        self.add_member(&server, &gina, ServerMemberRole::Guest)
            .await;
        self.add_owner(&other, &olive).await;

        Roles {
            server_id: server.id.to_string(),
            other_server_id: other.id.to_string(),
            alice: self.caller(&alice).await,
            mona: self.caller(&mona).await,
            bob: self.caller(&bob).await,
            carol: self.caller(&carol).await,
            gina: self.caller(&gina).await,
            olive: self.caller(&olive).await,
        }
    }
}

/// Create a channel titled `title` in `server_id` as `who` and return its id.
pub async fn create_channel(
    schema: &AppSchema,
    who: &Caller,
    server_id: &str,
    title: &str,
    is_private: bool,
) -> String {
    let data = exec_ok(
        schema,
        who.request(
            CREATE_CHANNEL,
            json!({ "input": {
                "serverId": server_id, "isChannel": true, "isPrivate": is_private, "title": title,
            } }),
        ),
    )
    .await;
    data["conversation"]["id"]
        .as_str()
        .expect("channel id")
        .to_string()
}
