use std::fmt;

use config::{Config, ConfigError, Environment};
use dotenvy::dotenv;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[allow(dead_code)]
pub struct RIMConfig {
    pub server_host: String,
    pub server_port: u16,

    pub database_url: String,

    pub bootstrap_secret: Option<String>,

    pub session_ttl_days: u32,

    pub enable_graphiql: bool,

    pub cors_allowed_origin: Option<String>,

    pub data_dir: String,

    pub storage_backend: Option<String>,
    pub storage_local_path: Option<String>,
    pub storage_s3_bucket: Option<String>,
    pub storage_s3_region: Option<String>,
    pub max_workspace_storage_bytes: Option<u64>,
}

impl fmt::Debug for RIMConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RIMConfig")
            .field("server_host", &self.server_host)
            .field("server_port", &self.server_port)
            .field("database_url", &"[REDACTED]")
            .field(
                "bootstrap_secret",
                &self.bootstrap_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("session_ttl_days", &self.session_ttl_days)
            .field("enable_graphiql", &self.enable_graphiql)
            .field("cors_allowed_origin", &self.cors_allowed_origin)
            .field("data_dir", &self.data_dir)
            .field("storage_backend", &self.storage_backend)
            .finish()
    }
}

impl Default for RIMConfig {
    fn default() -> Self {
        Self {
            server_host: "0.0.0.0".to_string(),
            server_port: 8080,
            database_url: "".to_string(),
            bootstrap_secret: None,
            session_ttl_days: 30,
            enable_graphiql: false,
            cors_allowed_origin: None,
            data_dir: "./data".to_string(),
            storage_backend: Some("local".to_string()),
            storage_local_path: None,
            storage_s3_bucket: None,
            storage_s3_region: None,
            max_workspace_storage_bytes: Some(1_073_741_824), // 1 GB default
        }
    }
}

impl RIMConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let c = Config::builder()
            .set_default("server_host", "0.0.0.0")?
            .set_default("server_port", 8080)?
            .set_default("session_ttl_days", 30)?
            .set_default("enable_graphiql", false)?
            .set_default("data_dir", "./data")?
            .set_default("storage_backend", "local")?
            .set_default("max_workspace_storage_bytes", 1_073_741_824_i64)?
            .add_source(Environment::with_prefix("RIM"))
            .add_source(Environment::default());
        c.build()?.try_deserialize()
    }
}

pub fn load_config() -> RIMConfig {
    dotenv().ok();
    RIMConfig::load().unwrap()
}
