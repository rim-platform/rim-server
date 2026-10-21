//! `init_logging` as the binary uses it: one audit event must end up in the day's audit file.

use std::fs;

#[test]
fn an_audit_event_reaches_the_days_audit_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    // Set before `init_logging` reads it; this test binary has no other test.
    unsafe {
        std::env::set_var("RIM_AUDIT_LOG_PATH", dir.path().join("audit.log"));
    }

    let handle = rim_logging::init_logging().expect("logging starts once per process");
    tracing::info!(target: "audit", event = "audit_file_test", "written to the audit file");
    tracing::info!(event = "not_audit", "written to stdout only");
    // Dropping the handle flushes the writer.
    drop(handle);

    let files: Vec<String> = fs::read_dir(dir.path())
        .expect("read dir")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .into_string()
                .expect("utf8")
        })
        .collect();
    let [name] = files.as_slice() else {
        panic!("one audit file expected: {files:?}");
    };
    assert!(rim_logging::audit_file_day(name).is_some(), "{name}");
    let text = fs::read_to_string(dir.path().join(name)).expect("audit file exists");
    assert!(text.contains("audit_file_test"), "{name}: {text}");
    assert!(!text.contains("not_audit"), "{name}: {text}");
}
