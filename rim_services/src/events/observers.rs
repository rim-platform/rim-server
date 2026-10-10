use std::sync::Arc;

use super::broadcast::Broadcaster;
use super::types::WorkspaceEvent;

/// Base trait for all observers — provides notify() which forwards to broadcaster.
pub trait EventObserver: Send + Sync {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster>;

    /// Further sinks every event goes to after the broadcaster.
    fn sinks(&self) -> &[Arc<dyn Broadcaster>] {
        &[]
    }

    fn notify(&self, event: WorkspaceEvent) {
        for sink in self.sinks() {
            sink.broadcast(event.clone());
        }
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
    sinks: Vec<Arc<dyn Broadcaster>>,
}

impl Observers {
    pub fn new(broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self {
            broadcaster,
            sinks: Vec::new(),
        }
    }

    /// Also send every event to `sink`, which must not drop events (the outbox writer).
    pub fn with_sink(mut self, sink: Arc<dyn Broadcaster>) -> Self {
        self.sinks.push(sink);
        self
    }
}

impl EventObserver for Observers {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster> {
        &self.broadcaster
    }

    fn sinks(&self) -> &[Arc<dyn Broadcaster>] {
        &self.sinks
    }
}

/// Observers as a broadcaster, for services that take one: every event reaches the sinks too.
impl Broadcaster for Observers {
    fn broadcast(&self, event: WorkspaceEvent) {
        self.notify(event);
    }
}
