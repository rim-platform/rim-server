use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::e2ee::model::MlsPurged;

use crate::ServiceResult;
use crate::e2ee::command::{
    AckMlsWelcomesCmd, ClaimMlsKeyPackagesCmd, GetMlsGroupQuery, ListMlsCommitsQuery, MlsCaller,
    PrincipalDevicesQuery, ReportEncryptedMessageCmd, RequestMlsReaddCmd, SubmitMlsCommitCmd,
    UploadMlsKeyPackagesCmd,
};
use crate::e2ee::dto::{
    ClaimedKeyPackageDto, MlsCommitDto, MlsCommitPageDto, MlsGroupDto, MlsKeyPackageStatusDto,
    MlsWelcomeDto, PrincipalDevicesDto,
};

/// Key packages, group sequencing, Welcome delivery, re-add requests and franked reports of
/// encrypted conversations.
///
/// Every call that names a device acts for the session's device and fails with `Forbidden`
/// when the session is not bound to one. Encrypted messages themselves go through
/// `MessageService`.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait E2eeService: Send + Sync {
    /// The session device's key packages, per cipher suite.
    async fn key_package_status(
        &self,
        caller: MlsCaller,
    ) -> ServiceResult<Vec<MlsKeyPackageStatusDto>>;

    /// Validate and store key packages of the session's device, all or none. A last-resort
    /// package replaces the device's previous one. Answers the status of the preferred suite.
    async fn upload_key_packages(
        &self,
        cmd: UploadMlsKeyPackagesCmd,
    ) -> ServiceResult<MlsKeyPackageStatusDto>;

    /// Hand out one key package per target device for a Commit in one conversation. Devices
    /// with no usable key package are left out.
    async fn claim_key_packages(
        &self,
        cmd: ClaimMlsKeyPackagesCmd,
    ) -> ServiceResult<Vec<ClaimedKeyPackageDto>>;

    /// Accept a Commit for the conversation's current epoch, or the first Commit of an
    /// incarnation.
    ///
    /// # Errors
    /// `StaleEpoch` when the epoch or incarnation is not current, with the current ones.
    async fn submit_commit(&self, cmd: SubmitMlsCommitCmd) -> ServiceResult<MlsCommitDto>;

    async fn list_commits(&self, query: ListMlsCommitsQuery) -> ServiceResult<MlsCommitPageDto>;

    /// The session device's unacknowledged Welcomes, oldest first.
    async fn pending_welcomes(&self, caller: MlsCaller) -> ServiceResult<Vec<MlsWelcomeDto>>;

    /// Acknowledge Welcomes of the session's device. Returns how many were acknowledged now.
    async fn ack_welcomes(&self, cmd: AckMlsWelcomesCmd) -> ServiceResult<u64>;

    async fn request_readd(&self, cmd: RequestMlsReaddCmd) -> ServiceResult<()>;

    /// The conversation's current group, `None` while it has none.
    async fn group(&self, query: GetMlsGroupQuery) -> ServiceResult<Option<MlsGroupDto>>;

    /// The certified devices of principals who share a workspace with the caller; an empty
    /// list for any other principal.
    async fn principal_devices(
        &self,
        query: PrincipalDevicesQuery,
    ) -> ServiceResult<Vec<PrincipalDevicesDto>>;

    /// Verify the franking of a reported message and store the report with its content.
    ///
    /// # Errors
    /// `Validation` when the report does not match the message; `Conflict` for a second report.
    async fn report_encrypted_message(&self, cmd: ReportEncryptedMessageCmd) -> ServiceResult<()>;

    /// Tell the members of the user's encrypted conversations that the user's devices changed
    /// (revoked, or replaced by a recovery), so their groups remove them.
    async fn devices_changed(&self, user_id: rim_core::user::model::UserId) -> ServiceResult<()>;

    /// Delete what the retention rules no longer keep.
    async fn purge(&self) -> ServiceResult<MlsPurged>;
}
