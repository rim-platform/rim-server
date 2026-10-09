use async_graphql::*;
use rim_services::Actor;
use rim_services::e2ee::command::{
    AckMlsWelcomesCmd, ClaimMlsKeyPackagesCmd, MlsDeviceRef, ReportEncryptedMessageCmd,
    RequestMlsReaddCmd, SubmitMlsCommitCmd, UploadMlsKeyPackagesCmd,
};
use rim_services::e2ee::service::E2eeService;
use rim_services::message::command::{EditEncryptedMessageCmd, SendEncryptedMessageCmd};
use rim_services::message::service::MessageService;

use crate::auth_context::{require_actor, require_mls_caller};
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{
    ClaimMlsKeyPackagesInput, ClaimedKeyPackage, EditEncryptedMessageInput, Message, MlsCommit,
    MlsKeyPackageStatus, ReportEncryptedMessageInput, RequestMlsReaddInput,
    SendEncryptedMessageInput, SubmitMlsCommitInput,
};

fn parse_uuid(id: &ID) -> Result<uuid::Uuid> {
    parse_uuid_id::<uuid::Uuid>(id)
}

#[derive(Default)]
pub(crate) struct E2eeMutations;

#[Object]
impl E2eeMutations {
    /// Store key packages of the session's device, all or none. A last-resort package replaces
    /// the device's previous one.
    async fn upload_mls_key_packages(
        &self,
        ctx: &Context<'_>,
        key_packages: Vec<String>,
    ) -> Result<MlsKeyPackageStatus> {
        let caller = require_mls_caller(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn E2eeService>>()?;
        let status = svc
            .upload_key_packages(UploadMlsKeyPackagesCmd {
                caller,
                key_packages,
            })
            .await
            .map_err(map_service_error)?;
        Ok(status.into())
    }

    /// One key package per device waiting to be added; devices with none usable are left out.
    async fn claim_mls_key_packages(
        &self,
        ctx: &Context<'_>,
        input: ClaimMlsKeyPackagesInput,
    ) -> Result<Vec<ClaimedKeyPackage>> {
        let caller = require_mls_caller(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn E2eeService>>()?;
        let devices = input
            .devices
            .iter()
            .map(|d| {
                Ok(MlsDeviceRef {
                    principal_id: d.principal_id.clone(),
                    device_id: parse_uuid(&d.device_id)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let claimed = svc
            .claim_key_packages(ClaimMlsKeyPackagesCmd {
                caller,
                server_id: parse_uuid_id(&input.server_id)?,
                conversation_id: parse_uuid_id(&input.conversation_id)?,
                devices,
            })
            .await
            .map_err(map_service_error)?;
        Ok(claimed.into_iter().map(Into::into).collect())
    }

    /// A Commit for the group's current epoch, or an incarnation's first Commit (epoch 0).
    /// `STALE_EPOCH` names the current incarnation and epoch when another Commit came first.
    async fn submit_mls_commit(
        &self,
        ctx: &Context<'_>,
        input: SubmitMlsCommitInput,
    ) -> Result<MlsCommit> {
        let caller = require_mls_caller(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn E2eeService>>()?;
        let commit = svc
            .submit_commit(SubmitMlsCommitCmd {
                caller,
                server_id: parse_uuid_id(&input.server_id)?,
                conversation_id: parse_uuid_id(&input.conversation_id)?,
                incarnation: input.incarnation,
                epoch: i64::from(input.epoch),
                cipher_suite: input.cipher_suite,
                commit: input.commit,
                welcome: input.welcome,
            })
            .await
            .map_err(map_service_error)?;
        Ok(commit.into())
    }

    /// An encrypted message under a client-chosen id, in the group's current epoch. Sending
    /// the same id and envelope again answers the stored message.
    async fn send_encrypted_message(
        &self,
        ctx: &Context<'_>,
        input: SendEncryptedMessageInput,
    ) -> Result<Message> {
        let caller = require_mls_caller(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let message = svc
            .send_encrypted_message(SendEncryptedMessageCmd {
                caller,
                server_id: parse_uuid_id(&input.server_id)?,
                conversation_id: parse_uuid_id(&input.conversation_id)?,
                message_id: parse_uuid_id(&input.message_id)?,
                envelope: input.envelope,
                thread_root_message_id: input
                    .thread_root_message_id
                    .as_ref()
                    .map(parse_uuid_id)
                    .transpose()?,
                forwarded_from_message_id: input
                    .forwarded_from_message_id
                    .as_ref()
                    .map(parse_uuid_id)
                    .transpose()?,
            })
            .await
            .map_err(map_service_error)?;
        Ok(message.into())
    }

    /// A new envelope carrying an encrypted message's complete new content.
    async fn edit_encrypted_message(
        &self,
        ctx: &Context<'_>,
        input: EditEncryptedMessageInput,
    ) -> Result<Message> {
        let caller = require_mls_caller(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let message = svc
            .edit_encrypted_message(EditEncryptedMessageCmd {
                caller,
                server_id: parse_uuid_id(&input.server_id)?,
                conversation_id: parse_uuid_id(&input.conversation_id)?,
                message_id: parse_uuid_id(&input.message_id)?,
                envelope: input.envelope,
            })
            .await
            .map_err(map_service_error)?;
        Ok(message.into())
    }

    /// Acknowledge Welcomes of the session's device; answers how many were acknowledged now.
    async fn ack_mls_welcomes(&self, ctx: &Context<'_>, ids: Vec<ID>) -> Result<i32> {
        let caller = require_mls_caller(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn E2eeService>>()?;
        let ids = ids.iter().map(parse_uuid_id).collect::<Result<Vec<_>>>()?;
        let acked = svc
            .ack_welcomes(AckMlsWelcomesCmd { caller, ids })
            .await
            .map_err(map_service_error)?;
        Ok(i32::try_from(acked).unwrap_or(i32::MAX))
    }

    /// Ask the group's other devices to remove and add the session's device again.
    async fn request_mls_readd(
        &self,
        ctx: &Context<'_>,
        input: RequestMlsReaddInput,
    ) -> Result<bool> {
        let caller = require_mls_caller(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn E2eeService>>()?;
        svc.request_readd(RequestMlsReaddCmd {
            caller,
            server_id: parse_uuid_id(&input.server_id)?,
            conversation_id: parse_uuid_id(&input.conversation_id)?,
            payload: input.request.payload,
            signature: input.request.signature,
        })
        .await
        .map_err(map_service_error)?;
        Ok(true)
    }

    /// Report an encrypted message by revealing its content and franking key; the server checks
    /// them against its receipt before the moderators see them.
    async fn report_encrypted_message(
        &self,
        ctx: &Context<'_>,
        input: ReportEncryptedMessageInput,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn E2eeService>>()?;
        svc.report_encrypted_message(ReportEncryptedMessageCmd {
            actor: Actor::new(actor),
            server_id: parse_uuid_id(&input.server_id)?,
            conversation_id: parse_uuid_id(&input.conversation_id)?,
            message_id: parse_uuid_id(&input.message_id)?,
            reason: input.reason,
            content: input.content,
            franking_key: input.franking_key,
            receipt: input.receipt,
            receipt_key_id: input.receipt_key_id,
            sender_user_id: parse_uuid_id(&input.sender_user_id)?,
            sender_device_id: parse_uuid(&input.sender_device_id)?,
            sent_at_us: input.sent_at_us,
        })
        .await
        .map_err(map_service_error)?;
        Ok(true)
    }
}
