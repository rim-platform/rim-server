//! The RIM server: configuration, the HTTP routes, the composition root and the CLI commands.
//!
//! The binary (`main.rs`) and the two-server tests (`rim_api/tests/s2s_e2e.rs`) are the
//! consumers of the public modules.

pub mod abuse_cmd;
mod alert_handlers;
pub mod app;
pub mod bootstrap_cmd;
pub mod config;
mod file_handlers;
pub mod franking_key_cmd;
mod graphql_support;
mod health;
mod plugin_bundle_handlers;
mod remote_handlers;
mod s2s;
pub mod server;
pub mod server_key_cmd;

pub use s2s::{S2sSettings, S2sState};
