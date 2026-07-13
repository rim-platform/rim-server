use std::collections::BTreeSet;
use std::fs;

use migration::{Migrator, MigratorTrait};

/// Every `m<timestamp>_*.rs` migration source file must be registered in `Migrator::migrations()`
/// (both the `mod` line and the `Box::new(...)` entry). Guards against `sea migrate generate`
/// silently dropping adjacent registrations when it rewrites `lib.rs` — a tooling hazard that has
/// bitten this repo repeatedly (each drop was caught by hand via manual `Read`-diff before this
/// test existed; see `docs/specs/migration-registration-guard.md`). No DB required: this only
/// reads the source directory and constructs the migration vec, so it runs in the normal
/// `cargo test` pass and fails loudly, naming the offender, on the very next push after a drop.
#[test]
fn every_migration_file_is_registered() {
    let src_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");

    // Migration files are `m` + 8 digits + `_...` — this excludes `lib.rs` and `main.rs`
    // (main.rs also starts with `m`, so match the digit pattern, not just the leading `m`).
    let is_migration_stem = |s: &str| {
        let bytes = s.as_bytes();
        bytes.first() == Some(&b'm')
            && s.len() > 9
            && bytes[1..9].iter().all(u8::is_ascii_digit)
            && bytes[9] == b'_'
    };

    let files_on_disk: BTreeSet<String> = fs::read_dir(src_dir)
        .expect("read migration/src")
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".rs").map(str::to_string))
        .filter(|stem| is_migration_stem(stem))
        .collect();

    let registered: BTreeSet<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();

    let unregistered: Vec<&String> = files_on_disk.difference(&registered).collect();
    let orphaned: Vec<&String> = registered.difference(&files_on_disk).collect();

    assert!(
        unregistered.is_empty(),
        "migration files exist on disk but are NOT registered in migration/src/lib.rs \
         (sea-migrate-generate likely dropped them): {unregistered:?}"
    );
    assert!(
        orphaned.is_empty(),
        "migrations are registered but their source file is missing: {orphaned:?}"
    );
}
