use tokio::sync::broadcast;

pub use rim_services::events::*;

/// In-memory broadcaster using tokio broadcast channels.
/// Suitable for single-instance deployments.
#[derive(Clone)]
pub struct InMemoryBroadcaster {
    sender: broadcast::Sender<WorkspaceEvent>,
}

impl InMemoryBroadcaster {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkspaceEvent> {
        self.sender.subscribe()
    }
}

impl Broadcaster for InMemoryBroadcaster {
    fn broadcast(&self, event: WorkspaceEvent) {
        let _ = self.sender.send(event);
    }
}
