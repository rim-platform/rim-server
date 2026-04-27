use std::sync::Arc;

use super::broadcast::Broadcaster;
use super::types::WorkspaceEvent;

/// Base trait for all observers — provides notify() which forwards to broadcaster.
pub trait EventObserver: Send + Sync {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster>;

    fn notify(&self, event: WorkspaceEvent) {
        self.broadcaster().broadcast(event);
    }

    fn emit(&self, event: WorkspaceEvent) {
        self.notify(event);
    }
}

/// Single observer that holds the broadcaster and emits any WorkspaceEvent.
///
/// Replaces the previous 6 typed sub-observer structs (MessageObserver,
/// ReactionObserver, ThreadObserver, WorkspaceObserver, ReadStateObserver,
/// MentionObserver) and their 9+ positional-arg `on_*` trampolines.
/// Call sites construct WorkspaceEvent variants inline and call `observers.emit(event)`.
#[derive(Clone)]
pub struct Observers {
    broadcaster: Arc<dyn Broadcaster>,
}

impl Observers {
    pub fn new(broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self { broadcaster }
    }
}

impl EventObserver for Observers {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster> {
        &self.broadcaster
    }
}
