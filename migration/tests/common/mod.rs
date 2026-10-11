use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// The migration source files in `migration/src`, keyed by file stem (the migration's name).
/// Only `m` + 8 digits + `_...` files count, which leaves out `lib.rs` and `main.rs`.
pub fn migration_sources() -> BTreeMap<String, PathBuf> {
    let src_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));

    let is_migration_stem = |s: &str| {
        let bytes = s.as_bytes();
        bytes.first() == Some(&b'm')
            && s.len() > 9
            && bytes[1..9].iter().all(u8::is_ascii_digit)
            && bytes[9] == b'_'
    };

    fs::read_dir(&src_dir)
        .expect("read migration/src")
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|name| {
            let stem = name.strip_suffix(".rs")?.to_string();
            is_migration_stem(&stem).then(|| (stem, src_dir.join(&name)))
        })
        .collect()
}
