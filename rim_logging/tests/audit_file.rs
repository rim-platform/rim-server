//! `init_logging` as the binary uses it: one audit event must end up in the audit file.

use std::fs;

#[test]
fn an_audit_event_reaches_the_audit_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    // Set before `init_logging` reads it; this test binary has no other test.
    unsafe {
        std::env::set_var("RIM_AUDIT_LOG_PATH", dir.path().join("audit.log"));
    }

    let handle = rim_logging::init_tracing_json().expect("logging starts once per process");
    tracing::info!(target: "audit", event = "audit_file_test", "written to the audit file");
    tracing::info!(event = "not_audit", "written to stdout only");
    // Dropping the handle flushes the writer.
    drop(handle);

    let text = fs::read_to_string(dir.path().join("audit.log")).expect("audit.log exists");
    assert!(text.contains("audit_file_test"), "audit.log: {text}");
    assert!(!text.contains("not_audit"), "audit.log: {text}");
}
