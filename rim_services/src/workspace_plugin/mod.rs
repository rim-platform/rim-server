pub mod command;
pub mod dto;
pub mod service;

use rim_core::server::model::ServerId;

/// The media type of a plugin bundle file.
pub const PLUGIN_BUNDLE_MEDIA_TYPE: &str = "application/vnd.rim.plugin-bundle+json";

/// Largest bundle file, in bytes.
pub const MAX_PLUGIN_BUNDLE_BYTES: usize = 8 * 1024 * 1024;

/// Where members download a bundle the workspace keeps: the default `bundleUrl` of an entry.
pub fn plugin_bundle_path(server_id: ServerId, sha256: &str) -> String {
    format!("/api/plugin-bundles/{server_id}/{sha256}")
}
