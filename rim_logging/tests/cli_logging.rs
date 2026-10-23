use std::fs;
use std::process::Command;

#[test]
fn cli_probe() {
    if std::env::var_os("RIM_LOGGING_PROBE").is_none() {
        return;
    }
    let _handle = rim_logging::init_cli_logging().expect("logging");
    tracing::info!(marker = "info_diagnostic", "info diagnostic");
    tracing::warn!(marker = "warning_diagnostic", "warning diagnostic");
    tracing::info!(target: "audit", event = "ip.unblock", "audit probe");
}

#[test]
fn cli_diagnostics_respect_the_filter_and_audit_is_flushed_on_return() {
    for filter in [None, Some("info"), Some("off")] {
        let dir = tempfile::tempdir().expect("temporary directory");
        let mut command = Command::new(std::env::current_exe().expect("test binary"));
        command
            .args(["--exact", "cli_probe", "--nocapture"])
            .current_dir(dir.path())
            .env("RIM_LOGGING_PROBE", "1")
            .env("RIM_AUDIT_LOG_PATH", dir.path().join("audit.log"))
            .env_remove("RUST_LOG");
        if let Some(filter) = filter {
            command.env("RUST_LOG", filter);
        }
        let output = command.output().expect("run logging probe");
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).expect("stdout");
        let stderr = String::from_utf8(output.stderr).expect("stderr");
        assert!(stdout.contains("ip.unblock"), "{stdout}");
        assert!(!stdout.contains("info_diagnostic"), "{stdout}");
        assert!(!stdout.contains("warning_diagnostic"), "{stdout}");
        assert_eq!(
            stderr.contains("info_diagnostic"),
            filter == Some("info"),
            "{stderr}"
        );
        assert_eq!(
            stderr.contains("warning_diagnostic"),
            filter != Some("off"),
            "{stderr}"
        );
        let audit = fs::read_dir(dir.path())
            .expect("audit directory")
            .map(|entry| fs::read_to_string(entry.expect("entry").path()).expect("audit file"))
            .collect::<String>();
        assert!(audit.contains("ip.unblock"), "{audit}");
        assert!(!audit.contains("warning_diagnostic"), "{audit}");
    }
}
