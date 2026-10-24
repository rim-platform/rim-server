use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::auth::model::SessionId;

use crate::ServiceResult;
use crate::session_key::dto::ReloadKeysDto;

/// The keys a session holds for its web client: the keep key of a kept session, and the reload
/// key of an unkept one with the window in which a reloaded tab may take it.
///
/// A tab of an unkept session keeps its data in `sessionStorage`, sealed under a reload key
/// only the server holds. After a reload the tab takes that key once, within 60 seconds of
/// the session's WebSocket closing and while no WebSocket of the session is open.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait SessionKeyService: Send + Sync {
    /// The keep key the server holds for the session `session_id`: `None` for an unkept
    /// session and for a kept one whose client holds its own keep key.
    ///
    /// # Errors
    /// `Unauthenticated` when the session is not live; storage errors propagate.
    async fn keep_key(&self, session_id: SessionId) -> ServiceResult<Option<Vec<u8>>>;

    /// Make a new reload key for the unkept session `session_id`, store it in place of the
    /// previous one, and return it.
    ///
    /// # Errors
    /// `Unauthenticated` when the session is kept or not live; storage errors propagate.
    async fn set_reload_key(&self, session_id: SessionId) -> ServiceResult<Vec<u8>>;

    /// Hand the unkept session `session_id` its reload key and a new one stored in its place,
    /// once per closing of its WebSocket, within 60 seconds of it.
    ///
    /// # Errors
    /// `Unauthenticated` when the session is kept or not live, holds no reload key, has a
    /// WebSocket open, or its WebSocket closed more than 60 seconds ago or the key was already
    /// taken since; storage errors propagate.
    async fn reload(&self, session_id: SessionId) -> ServiceResult<ReloadKeysDto>;

    /// Record that a WebSocket of the session `session_id` opened, which closes its reload
    /// window. A kept session records nothing.
    ///
    /// # Errors
    /// Propagates storage errors.
    async fn socket_opened(&self, session_id: SessionId) -> ServiceResult<()>;

    /// Record that a WebSocket of the session `session_id` closed now, which opens its reload
    /// window. A kept session records nothing.
    ///
    /// # Errors
    /// Propagates storage errors.
    async fn socket_closed(&self, session_id: SessionId) -> ServiceResult<()>;
}
