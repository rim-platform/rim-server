use std::collections::BTreeSet;

use migration::{Migrator, MigratorTrait};

mod common;

/// Every `m<timestamp>_*.rs` migration source file must be registered in `Migrator::migrations()`
/// (both the `mod` line and the `Box::new(...)` entry). Guards against `sea migrate generate`
/// silently dropping adjacent registrations when it rewrites `lib.rs`. No DB required: this only
/// reads the source directory and constructs the migration vec, so it runs in the normal
/// `cargo test` pass and fails loudly, naming the offender.
#[test]
fn every_migration_file_is_registered() {
    let files_on_disk: BTreeSet<String> = common::migration_sources().into_keys().collect();

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
