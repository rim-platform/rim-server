//! Captures the audit lines a service emits, for the tests that check them.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;

/// The lines one capture received.
pub(crate) type AuditBuf = Arc<Mutex<Vec<u8>>>;

thread_local! {
    /// Where this thread's `audit` lines go while a capture is active.
    static AUDIT_CAPTURE: RefCell<Option<AuditBuf>> = const { RefCell::new(None) };
}

/// Hands the formatter a writer into the calling thread's capture buffer.
struct ThreadAuditWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ThreadAuditWriter {
    type Writer = AuditBufWriter;
    fn make_writer(&'a self) -> Self::Writer {
        AuditBufWriter(AUDIT_CAPTURE.with(|capture| capture.borrow().clone()))
    }
}

/// Appends to a capture buffer; discards the line when the thread has none.
struct AuditBufWriter(Option<AuditBuf>);

impl std::io::Write for AuditBufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(capture) = &self.0 {
            capture.lock().unwrap().extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Ends this thread's capture when dropped.
pub(crate) struct AuditCaptureGuard;

impl Drop for AuditCaptureGuard {
    fn drop(&mut self) {
        AUDIT_CAPTURE.with(|capture| capture.borrow_mut().take());
    }
}

/// Captures this thread's `audit` lines into the returned buffer while the guard lives.
/// The subscriber is the process-wide default, installed once: tracing caches each callsite's
/// interest from the subscribers every thread can see when it is first reached, so a
/// thread-local subscriber misses a callsite another test's thread reached first.
pub(crate) fn capture_audit_lines() -> (AuditCaptureGuard, AuditBuf) {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let layer = tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_ansi(false)
            .with_writer(ThreadAuditWriter)
            .with_filter(tracing_subscriber::filter::filter_fn(|m| {
                m.target() == "audit"
            }));
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer))
            .expect("no other global subscriber in this test binary");
        tracing::callsite::rebuild_interest_cache();
    });
    let buf = AuditBuf::default();
    AUDIT_CAPTURE.with(|capture| *capture.borrow_mut() = Some(Arc::clone(&buf)));
    (AuditCaptureGuard, buf)
}

/// What the capture received so far.
pub(crate) fn drain_buf(buf: &AuditBuf) -> String {
    String::from_utf8(buf.lock().unwrap().clone()).expect("audit emits are UTF-8 JSON")
}
