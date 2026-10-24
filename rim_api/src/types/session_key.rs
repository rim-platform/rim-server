//! The answers of the session key roots. Keys travel as unpadded base64url strings.

use async_graphql::*;
use rim_services::session_key::dto::ReloadKeysDto;

use crate::types::account_password::encode_bytes;

/// The reload keys `reloadSession` hands a tab that reloaded.
#[derive(SimpleObject, Clone, Debug)]
pub struct ReloadSessionPayload {
    /// The reload key the tab's stored data is sealed under, 32 bytes.
    pub reload_key: String,
    /// The reload key the server now holds instead, 32 bytes: the tab seals its data again
    /// under it.
    pub next_reload_key: String,
}

impl From<ReloadKeysDto> for ReloadSessionPayload {
    fn from(dto: ReloadKeysDto) -> Self {
        Self {
            reload_key: encode_bytes(&dto.reload_key),
            next_reload_key: encode_bytes(&dto.next_reload_key),
        }
    }
}
