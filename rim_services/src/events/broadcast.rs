use super::types::WorkspaceEvent;

/// Trait for broadcasting events. Implementations can use in-memory channels,
/// Redis PUB/SUB, or any other transport.
pub trait Broadcaster: Send + Sync {
    fn broadcast(&self, event: WorkspaceEvent);
}

/// No-op broadcaster for use in tests where event broadcasting is not needed.
pub struct NoopBroadcaster;

impl Broadcaster for NoopBroadcaster {
    fn broadcast(&self, _event: WorkspaceEvent) {}
}
