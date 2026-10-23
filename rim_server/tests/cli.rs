use std::process::Command;

fn command(dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rim_server"));
    command.current_dir(dir).env_remove("RUST_LOG");
    command.env("RIM_AUDIT_LOG_PATH", dir.join("audit.log"));
    command
}

#[test]
fn unknown_command_prints_usage_without_loading_configuration_or_starting_logging() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let output = command(dir.path())
        .arg("not-a-command")
        .output()
        .expect("run binary");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("stderr");
    assert!(stderr.contains("unknown command"), "{stderr}");
    assert!(stderr.contains("Usage:"), "{stderr}");
    assert_eq!(std::fs::read_dir(dir.path()).expect("directory").count(), 0);
}

#[test]
fn fatal_configuration_error_is_flushed_to_the_audit_file_before_exit() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let output = command(dir.path())
        .env("RIM_SERVER_PORT", "invalid")
        .output()
        .expect("run binary");
    assert_eq!(output.status.code(), Some(1));
    let files: Vec<_> = std::fs::read_dir(dir.path())
        .expect("audit directory")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(files.len(), 1);
    let audit = std::fs::read_to_string(&files[0]).expect("audit file");
    assert!(
        audit.contains("fatal: failed to load configuration"),
        "{audit}"
    );
}

#[test]
fn command_usage_has_no_startup_or_sql_log_noise() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let output = command(dir.path()).arg("ip").output().expect("run binary");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
    let stderr = String::from_utf8(output.stderr).expect("stderr");
    assert!(!stderr.contains("Starting rim-server"), "{stderr}");
    assert!(!stderr.contains("sqlx"), "{stderr}");
}
