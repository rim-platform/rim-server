//! Integration test for F1 startup AI-callable inventory audit emission
//! (Phase 1 Task 8 / Widow §7.5 F1).
//!
//! Verifies that `emit_ai_callable_inventory_audit` emits a structured
//! `ai_callable_inventory` audit event with the correct count when the live
//! builtin command registry is used. The tracing output is captured via a
//! custom in-memory writer so the assertion is structural (field presence)
//! rather than fragile string matching on exact formatting.

use std::io;
use std::sync::{Arc, Mutex};

use rim_infra_services::command::emit_ai_callable_inventory_audit;

// ── Tracing capture infrastructure ──────────────────────────────────────────

#[derive(Clone)]
struct CapturingWriter(Arc<Mutex<Vec<u8>>>);

impl CapturingWriter {
    fn new() -> (Self, Arc<Mutex<Vec<u8>>>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        (Self(Arc::clone(&buf)), buf)
    }
}

struct CapturingWriterInstance(Arc<Mutex<Vec<u8>>>);

impl io::Write for CapturingWriterInstance {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingWriter {
    type Writer = CapturingWriterInstance;

    fn make_writer(&'a self) -> Self::Writer {
        CapturingWriterInstance(Arc::clone(&self.0))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// Verifies that `emit_ai_callable_inventory_audit` emits a log line
/// containing the `ai_callable_inventory` event and a non-zero `count` field.
///
/// Uses the live builtin command registry (no synthetic commands) — the Phase 1
/// builtins include at least `chat.post_message` as ai_callable, so count >= 1.
///
/// A scoped tracing subscriber captures JSON output to an in-memory buffer.
/// The assertion is substring-based to avoid coupling to field ordering.
#[test]
fn emit_ai_callable_inventory_audit_writes_structured_event() {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let (writer, captured) = CapturingWriter::new();

    let layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_level(true);

    let subscriber = tracing_subscriber::registry().with(layer);

    // `set_default` installs a thread-local default subscriber for the
    // duration of the guard, then restores the previous one on drop.
    // Safe for parallel test runs — no global mutation.
    let _guard = subscriber.set_default();

    emit_ai_callable_inventory_audit();

    let output = String::from_utf8(captured.lock().unwrap().clone()).unwrap_or_default();

    assert!(
        output.contains("ai_callable_inventory"),
        "expected `ai_callable_inventory` event in captured log output; got:\n{output}"
    );
    // `count` appears in the structured field output (plain fmt: `count=N`).
    assert!(
        output.contains("count="),
        "expected `count=` field in captured log output; got:\n{output}"
    );
    // The live registry has 45 ai_callable builtins (P4-A5, C2, Tracks 1.1-1.3, Track 4.1).
    // Assert `count=0` is NOT present — exact-count is pinned in the unit
    // test `collect_ai_callable_inventory_matches_reviewed_set`.
    assert!(
        !output.contains("count=0"),
        "ai_callable_inventory count must be non-zero; got:\n{output}"
    );
    // The event must reach the `audit` target.
    assert!(
        output.contains("audit"),
        "expected `audit` target in captured log output; got:\n{output}"
    );
}
