use async_graphql::*;

use crate::types::ServerInfo;

#[derive(Default)]
pub(crate) struct ServerInfoQueries;

#[Object]
impl ServerInfoQueries {
    /// The server's release version and API version. Readable without a session.
    async fn server_info(&self) -> ServerInfo {
        ServerInfo::current()
    }
}
