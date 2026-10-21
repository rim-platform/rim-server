//! The RIM server: configuration, the HTTP routes, the composition root and the CLI commands.
//!
//! The binary (`main.rs`) and the two-server tests (`rim_api/tests/s2s_e2e.rs`) are the
//! consumers of the public modules.

pub mod abuse_cmd;
pub mod account_cmd;
pub mod app;
pub mod audit_cmd;
pub(crate) mod audit_file;
mod body;
pub mod config;
mod edge;
mod file_handlers;
pub mod franking_key_cmd;
mod graphql_support;
mod health;
pub mod invite_cmd;
pub mod ip_cmd;
pub(crate) mod metrics;
pub mod operator_cmd;
pub(crate) mod origin;
mod plugin_bundle_handlers;
mod remote_handlers;
mod s2s;
pub mod server;
pub mod server_key_cmd;
pub(crate) mod terms;
pub(crate) mod ws;

pub use s2s::{S2sSettings, S2sState};
