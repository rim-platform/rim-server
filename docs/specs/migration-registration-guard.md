# Guard against dropped migration registrations (sea-migrate-generate hazard)

**Author:** iron-man · **Implementer:** hulk · **Requested by:** nick-fury · **Priority:** low (but recurring)
**Type:** test-harness guard. No production code, no schema.

## Problem
`sea migrate generate <name>` (sea-orm-cli) rewrites `migration/src/lib.rs` to register the new
migration, and in doing so has **twice this session** silently dropped adjacent `Box::new(...)`
registrations (the m20260331–m20260402 cluster). A dropped registration means those migrations never run
on a fresh DB → every db-test suite breaks and `sea migrate up` produces a wrong schema. hulk caught both
by eye and restored via Read, but it's a recurring tooling hazard with no automated tripwire.

## Design
Add a **no-DB test in the `migration` crate** that asserts the set of migration source files on disk
exactly equals the set registered in `Migrator::migrations()`. A dropped `Box::new(...)` (or a `mod` line)
then fails this test loudly, naming the missing migration — it runs in the normal `cargo test` pass, no
`DATABASE_URL` required (it only reads the source directory and constructs the migration vec).

**Why compare `MigrationTrait::name()` to file stems (not parse lib.rs text):** `DeriveMigrationName`
derives each migration's `name()` from its module path, which equals the file stem (e.g. file
`m20260425_000001_create_command_audit_log.rs` → `name() == "m20260425_000001_create_command_audit_log"`).
So a set-equality check between `read_dir` stems and `Migrator::migrations().map(|m| m.name())` is robust
and needs no brittle source-text scraping. Set equality catches both directions: a file that exists but
isn't registered (the drop bug) **and** a registration whose file was removed.

New file `migration/tests/registration_complete.rs`:
```rust
use std::collections::BTreeSet;
use std::fs;

use migration::{Migrator, MigratorTrait};

/// Every `m<timestamp>_*.rs` migration source file must be registered in `Migrator::migrations()`
/// (both the `mod` line and the `Box::new(...)` entry). Guards against `sea migrate generate`
/// silently dropping adjacent registrations when it rewrites `lib.rs`. No DB required.
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
```

Confirm `Migrator`/`MigratorTrait` are exported from the `migration` crate root (they are — `lib.rs` does
`pub use sea_orm_migration::prelude::*` and defines `pub struct Migrator`). If `name()` isn't in scope,
it's on `MigrationName`/`MigrationTrait` via the prelude; adjust the import to whatever the crate
re-exports.

## Acceptance
- Passing today: run `cargo test -p migration` — green against the current (correct) tree.
- Failing when it should: temporarily comment out one `Box::new(...)` line in `lib.rs`, re-run, confirm the
  test fails naming that migration; restore it.
- No `DATABASE_URL`, no Postgres — this runs in the normal workspace test pass, so CI catches a dropped
  registration on the very next push.

## Docs
Doc comment on the test (above). Optionally a one-line note in CLAUDE.md's migration section: "after
`sea migrate generate`, `cargo test -p migration` verifies no registration was dropped."

## Rollback
Pure test addition; revert is a code revert.
