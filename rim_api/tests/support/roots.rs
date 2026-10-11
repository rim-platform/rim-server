/// One GraphQL document per messaging root, each naming its targets through the shared variables
/// of [`Targets`], so a suite sends every root at the same ids as any caller.
use serde_json::{Value, json};

use rim_api::AppSchema;

use super::graphql::{Call, Caller, exec_ok};

/// Roots that read or post into conversation `$c` of workspace `$s`.
pub const CONVERSATION_MESSAGE_ROOTS: &[&str] = &[
    "query($s: ID!, $c: ID!) { messages(serverId: $s, conversationId: $c, dir: AFTER, limit: 5, \
     includeDeleted: false) { items { id } } }",
    "query($s: ID!, $c: ID!) { searchMessages(input: { serverId: $s, conversationId: $c, \
     query: \"x\" }) { items { id } } }",
    "query($s: ID!, $c: ID!) { pinnedMessages(serverId: $s, conversationId: $c) { id } }",
    "mutation($s: ID!, $c: ID!) { sendMessage(input: { serverId: $s, conversationId: $c, \
     text: \"x\" }) { id } }",
];

/// Roots that name message `$m` of `$c`, or `$t`, the thread under it.
pub const MESSAGE_ROOTS: &[&str] = &[
    "query($s: ID!, $c: ID!, $m: ID!) { message(serverId: $s, conversationId: $c, \
     messageId: $m) { id } }",
    "query($s: ID!, $c: ID!, $t: ID!) { thread(serverId: $s, conversationId: $c, threadId: $t) \
     { id } }",
    "query($s: ID!, $c: ID!, $m: ID!) { threadByRootMessage(serverId: $s, conversationId: $c, \
     rootMessageId: $m) { id } }",
    "query($s: ID!, $c: ID!, $t: ID!) { threadMessages(serverId: $s, conversationId: $c, \
     threadId: $t, dir: AFTER, limit: 5, includeDeleted: false) { items { id } } }",
    "mutation($s: ID!, $c: ID!, $m: ID!) { replyToMessage(input: { serverId: $s, \
     conversationId: $c, rootMessageId: $m, text: \"x\" }) { id } }",
    "mutation($s: ID!, $c: ID!, $m: ID!) { editMessage(input: { serverId: $s, \
     conversationId: $c, messageId: $m, text: \"x\" }) { id } }",
    "mutation($s: ID!, $c: ID!, $m: ID!) { deleteMessage(input: { serverId: $s, \
     conversationId: $c, messageId: $m }) }",
    "mutation($s: ID!, $c: ID!, $m: ID!) { forwardMessage(input: { serverId: $s, \
     sourceConversationId: $c, targetConversationId: $c, messageId: $m }) { id } }",
    "mutation($s: ID!, $c: ID!, $m: ID!) { toggleReaction(input: { serverId: $s, \
     conversationId: $c, messageId: $m, emoji: \"👍\" }) { emoji } }",
    "mutation($s: ID!, $c: ID!, $m: ID!) { pinMessage(input: { serverId: $s, \
     conversationId: $c, messageId: $m }) }",
    "mutation($s: ID!, $c: ID!, $m: ID!) { unpinMessage(input: { serverId: $s, \
     conversationId: $c, messageId: $m }) }",
];

/// Roots that attach a file to message `$m` or change its attachment `$a`.
pub const ATTACHMENT_ROOTS: &[&str] = &[
    "mutation($s: ID!, $c: ID!, $m: ID!) { createAttachment(input: { serverId: $s, \
     conversationId: $c, messageId: $m, filename: \"a.txt\", contentType: \"text/plain\", \
     sizeBytes: 3 }) { attachmentId } }",
    "mutation($s: ID!, $a: ID!) { finalizeAttachment(input: { serverId: $s, attachmentId: $a }) \
     { id } }",
    "mutation($s: ID!, $a: ID!) { deleteAttachment(input: { serverId: $s, attachmentId: $a }) }",
];

/// Roots that need the caller to see conversation `$c`; `$u` is the member removed.
pub const CONVERSATION_ROOTS: &[&str] = &[
    "query($s: ID!, $c: ID!) { conversation(serverId: $s, conversationId: $c) { id } }",
    "mutation($s: ID!, $c: ID!) { leaveConversation(serverId: $s, conversationId: $c) }",
    "mutation($s: ID!, $c: ID!, $u: ID!) { removeConversationMember(serverId: $s, \
     conversationId: $c, userId: $u) }",
    "mutation($s: ID!, $c: ID!) { updateConversationTopic(input: { serverId: $s, \
     conversationId: $c, topic: \"x\" }) { id } }",
    "mutation($s: ID!, $c: ID!) { markAsRead(input: { workspaceId: $s, conversationId: $c, \
     readAt: \"2026-01-01T00:00:00Z\" }) }",
];

/// Roots that bring someone into channel `$c`: the caller, or `$u`.
pub const JOIN_ROOTS: &[&str] = &[
    "mutation($s: ID!, $c: ID!) { joinChannel(serverId: $s, conversationId: $c) { id } }",
    "mutation($s: ID!, $c: ID!, $u: ID!) { addConversationMembers(serverId: $s, \
     conversationId: $c, userIds: [$u]) { id } }",
];

/// The caller's notification rules for conversation `$c`.
pub const CONVERSATION_NOTIFICATION_ROOTS: &[&str] = &[
    "mutation($s: ID!, $c: ID!) { setConversationNotificationLevel(serverId: $s, \
     conversationId: $c, level: ALL) { level } }",
    "mutation($s: ID!, $c: ID!) { muteConversation(input: { serverId: $s, conversationId: $c }) }",
    "mutation($s: ID!, $c: ID!) { unmuteConversation(input: { serverId: $s, \
     conversationId: $c }) }",
];

/// The ids the documents above name.
#[derive(Clone, Debug)]
pub struct Targets {
    /// `$s`.
    pub server: String,
    /// `$c`.
    pub conversation: String,
    /// `$m`, the root of `thread`.
    pub message: String,
    /// `$t`.
    pub thread: String,
    /// `$a`, an attachment of `message`.
    pub attachment: String,
    /// `$u`.
    pub user: String,
}

const SEND: &str = "mutation($s: ID!, $c: ID!, $text: String!) { \
     msg: sendMessage(input: { serverId: $s, conversationId: $c, text: $text }) { id } }";
const REPLY: &str = "mutation($s: ID!, $c: ID!, $m: ID!) { msg: replyToMessage(input: { \
     serverId: $s, conversationId: $c, rootMessageId: $m, text: \"a reply\" }) { threadId } }";
const ATTACH: &str = "mutation($s: ID!, $c: ID!, $m: ID!) { createAttachment(input: { \
     serverId: $s, conversationId: $c, messageId: $m, filename: \"notes.txt\", \
     contentType: \"text/plain\", sizeBytes: 5 }) { attachmentId } }";

impl Targets {
    /// In `conversation_id`, `author` posts `text` and attaches a file to it and `replier`
    /// replies to it. `user` is the member the conversation roots remove.
    pub async fn seed(
        schema: &AppSchema,
        server_id: &str,
        conversation_id: &str,
        author: &Caller,
        replier: &Caller,
        user: &Caller,
        text: &str,
    ) -> Self {
        let mut vars = json!({ "s": server_id, "c": conversation_id, "text": text });
        vars["m"] = exec_ok(schema, author.request(SEND, vars.clone())).await["msg"]["id"].take();
        let reply = exec_ok(schema, replier.request(REPLY, vars.clone())).await;
        let ticket = exec_ok(schema, author.request(ATTACH, vars.clone())).await;
        Self {
            server: server_id.to_string(),
            conversation: conversation_id.to_string(),
            message: id_of(&vars["m"]),
            thread: id_of(&reply["msg"]["threadId"]),
            attachment: id_of(&ticket["createAttachment"]["attachmentId"]),
            user: user.user_id.to_string(),
        }
    }

    /// Conversation `conversation_id` of workspace `server_id`, `user` as `$u`, and ids that
    /// name nothing below the conversation.
    pub fn conversation(server_id: &str, conversation_id: &str, user: &Caller) -> Self {
        let none = super::unknown_id();
        Self {
            server: server_id.to_string(),
            conversation: conversation_id.to_string(),
            message: none.clone(),
            thread: none.clone(),
            attachment: none,
            user: user.user_id.to_string(),
        }
    }

    /// The same workspace with every other id set to `id`.
    pub fn elsewhere(&self, id: &str) -> Self {
        Self {
            server: self.server.clone(),
            conversation: id.to_string(),
            message: id.to_string(),
            thread: id.to_string(),
            attachment: id.to_string(),
            user: id.to_string(),
        }
    }

    /// The variables of the documents above.
    pub fn vars(&self) -> Value {
        json!({
            "s": self.server, "c": self.conversation, "m": self.message, "t": self.thread,
            "a": self.attachment, "u": self.user,
        })
    }

    /// Each of `docs` sent at these targets.
    pub fn calls(&self, docs: &[&[&'static str]]) -> Vec<Call> {
        let vars = self.vars();
        docs.iter()
            .flat_map(|list| list.iter())
            .map(|doc| (*doc, vars.clone()))
            .collect()
    }
}

fn id_of(v: &Value) -> String {
    v.as_str().expect("id").to_string()
}
