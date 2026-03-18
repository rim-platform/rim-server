use config::{Config, ConfigError, Environment};
use dotenvy::dotenv;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct RIMConfig {
    pub server_host: String,
    pub server_port: u16,

    pub database_url: String,

    pub bootstrap_secret: Option<String>,

    pub session_ttl_days: u32,

    pub enable_graphiql: bool,

    pub cors_allowed_origin: Option<String>,
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
            .add_source(Environment::with_prefix("RIM"))
            .add_source(Environment::default());
        c.build()?.try_deserialize()
    }
}

pub fn load_config() -> RIMConfig {
    dotenv().ok();
    RIMConfig::load().unwrap()
}
