use rim_core::conversation::model::ConversationId;
use rim_core::e2ee::model::{MlsWelcomeCursor, MlsWelcomeId};
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use uuid::Uuid;

use crate::Actor;

/// A signed-in caller and the device its session is bound to, if any. Every MLS write needs
/// the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MlsCaller {
    pub actor: Actor,
    pub device_key_id: Option<DeviceKeyId>,
}

/// Binary values are unpadded base64url text, decoded and size-checked by the service.
#[derive(Debug, Clone)]
pub struct UploadMlsKeyPackagesCmd {
    pub caller: MlsCaller,
    pub key_packages: Vec<String>,
}

/// One target device of a claim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MlsDeviceRef {
    pub principal_id: String,
    pub device_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ClaimMlsKeyPackagesCmd {
    pub caller: MlsCaller,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub devices: Vec<MlsDeviceRef>,
}

#[derive(Debug, Clone)]
pub struct SubmitMlsCommitCmd {
    pub caller: MlsCaller,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    pub epoch: i64,
    /// Required for an incarnation's first Commit (epoch 0).
    pub cipher_suite: Option<i32>,
    pub commit: String,
    /// Required when the Commit adds devices.
    pub welcome: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ListMlsCommitsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    /// Exclusive; `None` reads from epoch 0.
    pub after_epoch: Option<i64>,
    /// 1 to 100; 50 when `None`.
    pub limit: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct ListMlsWelcomesQuery {
    pub caller: MlsCaller,
    /// Exclusive; `None` reads from the oldest.
    pub after: Option<MlsWelcomeCursor>,
    /// 1 to 100; 100 when `None`.
    pub limit: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct AckMlsWelcomesCmd {
    pub caller: MlsCaller,
    pub ids: Vec<MlsWelcomeId>,
}

#[derive(Debug, Clone)]
pub struct RequestMlsReaddCmd {
    pub caller: MlsCaller,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    /// The `mls-readd` statement as signed.
    pub payload: String,
    pub signature: String,
}

#[derive(Debug, Clone, Copy)]
pub struct GetMlsGroupQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone)]
pub struct PrincipalDevicesQuery {
    pub actor: Actor,
    pub principal_ids: Vec<String>,
}

/// A report revealing one encrypted message's content and franking key to the moderators.
#[derive(Debug, Clone)]
pub struct ReportEncryptedMessageCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub reason: String,
    pub content: String,
    pub franking_key: String,
    pub receipt: String,
    pub receipt_key_id: i32,
    pub sender_user_id: UserId,
    /// The sender's client device id.
    pub sender_device_id: Uuid,
    /// The message's `sent_at` in microseconds since the Unix epoch, as decimal text.
    pub sent_at_us: String,
}
