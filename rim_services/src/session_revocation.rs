//! The hook that ends live connections when their session ends.
//!
//! A connection that runs under a session, such as a WebSocket, takes a [`SessionWatch`] for it
//! from [`SessionRevocations::watch`]. Every path that ends sessions in the database reports
//! them to [`SessionRevocations`] after its write, and each watch tied to one of them resolves.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use rim_core::auth::model::SessionId;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::user::model::UserId;
use tokio::sync::oneshot;

/// The session a live connection runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchedSession {
    pub session_id: SessionId,
    pub user_id: UserId,
    /// The device the session is bound to; `None` for a password session.
    pub device_key_id: Option<DeviceKeyId>,
}

/// The sessions one write ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndedSessions {
    /// These sessions.
    Sessions(Vec<SessionId>),
    /// Every session of `user_id` but `except`.
    User {
        user_id: UserId,
        except: Option<SessionId>,
    },
    /// Every session bound to this device.
    Device(DeviceKeyId),
}

impl EndedSessions {
    /// Whether `session` is among the ended sessions.
    pub fn covers(&self, session: &WatchedSession) -> bool {
        match self {
            Self::Sessions(ids) => ids.contains(&session.session_id),
            Self::User { user_id, except } => {
                session.user_id == *user_id && *except != Some(session.session_id)
            }
            Self::Device(device) => session.device_key_id == Some(*device),
        }
    }
}

/// Where ended sessions are reported and live connections watch theirs.
pub trait SessionRevocations: Send + Sync {
    /// Resolve the watch of every connection whose session `ended` names.
    fn end(&self, ended: EndedSessions);

    /// A watch that resolves once `session` ends; dropping it stops watching.
    fn watch(&self, session: WatchedSession) -> SessionWatch;

    /// The sessions watched now, each once.
    fn watched(&self) -> Vec<SessionId>;

    /// End the connections of `session_ids`.
    fn revoke(&self, session_ids: &[SessionId]) {
        if !session_ids.is_empty() {
            self.end(EndedSessions::Sessions(session_ids.to_vec()));
        }
    }

    /// End the connections of every session of `user_id`.
    fn revoke_user(&self, user_id: UserId) {
        self.end(EndedSessions::User {
            user_id,
            except: None,
        });
    }

    /// End the connections of every session of `user_id` but `keep`.
    fn revoke_user_except(&self, user_id: UserId, keep: SessionId) {
        self.end(EndedSessions::User {
            user_id,
            except: Some(keep),
        });
    }

    /// End the connections of every session bound to `device_key_id`.
    fn revoke_device(&self, device_key_id: DeviceKeyId) {
        self.end(EndedSessions::Device(device_key_id));
    }
}

/// A future that resolves once the watched session ends. It stays pending if the registry
/// goes away first. Dropping it stops watching.
pub struct SessionWatch {
    ended: oneshot::Receiver<()>,
    /// Removes the watch from its registry when dropped.
    _registration: Box<dyn Send + Sync>,
}

impl SessionWatch {
    /// A watch resolved by a send on `ended`, and the registration dropped with it.
    pub fn new(ended: oneshot::Receiver<()>, registration: impl Send + Sync + 'static) -> Self {
        Self {
            ended,
            _registration: Box::new(registration),
        }
    }

    /// Whether the session has ended, without waiting.
    pub fn has_ended(&mut self) -> bool {
        matches!(self.ended.try_recv(), Ok(()))
    }
}

impl Future for SessionWatch {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        match Pin::new(&mut self.ended).poll(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(()),
            // The registry dropped the sender without a revocation.
            Poll::Ready(Err(_)) | Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(user_id: UserId, device_key_id: Option<DeviceKeyId>) -> WatchedSession {
        WatchedSession {
            session_id: SessionId::new(),
            user_id,
            device_key_id,
        }
    }

    #[test]
    fn ended_sessions_cover_what_they_name() {
        let user = UserId::new();
        let device = DeviceKeyId::new();
        let on_device = session(user, Some(device));
        let password = session(user, None);
        let other = session(UserId::new(), None);

        let named = EndedSessions::Sessions(vec![password.session_id]);
        assert!(named.covers(&password));
        assert!(!named.covers(&on_device));

        let all = EndedSessions::User {
            user_id: user,
            except: None,
        };
        assert!(all.covers(&password) && all.covers(&on_device));
        assert!(!all.covers(&other));

        let but_one = EndedSessions::User {
            user_id: user,
            except: Some(password.session_id),
        };
        assert!(but_one.covers(&on_device));
        assert!(!but_one.covers(&password));

        let by_device = EndedSessions::Device(device);
        assert!(by_device.covers(&on_device));
        assert!(!by_device.covers(&password));
    }
}
