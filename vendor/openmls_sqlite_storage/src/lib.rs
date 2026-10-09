//! An empty stand-in for `openmls_sqlite_storage` 0.3.0.
//!
//! `openmls` names that crate as an optional dependency. Cargo resolves optional dependencies
//! into the lock file even when no feature enables them, and its `rusqlite` 0.37 links a
//! `libsqlite3-sys` that conflicts with the one SeaORM's optional SQLite driver links. RIM keeps
//! no MLS state on the server and never enables `openmls`'s `sqlite-provider` feature, so the
//! root manifest patches the crate with this empty one.
