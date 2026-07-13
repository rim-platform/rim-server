//! On-call schedules: rotation layers, ordered participants, overrides, and the pure "who is on
//! call now" resolver. See `docs/oncall.md` for the timezone/DST design and override precedence.

pub mod model;
pub mod repository;
pub mod resolve;
