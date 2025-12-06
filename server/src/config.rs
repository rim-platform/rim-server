use serde::Deserialize;
use config::{Config, ConfigError, Environment};
use dotenvy::dotenv;

#[derive(Clone, Debug, Deserialize)]
pub struct RIMConfig {
    pub server_host: String,
    pub server_port: u16,
}

impl Default for RIMConfig {
    fn default() -> Self {
        Self { server_host: "0.0.0.0".to_string(), server_port: 8080 }
    }
}

impl RIMConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let c = Config::builder().add_source(
            Environment::with_prefix("RIM")
        );
        Ok(c.build()?.try_deserialize()?)
    }
}

pub fn load_config() -> RIMConfig {
    dotenv().ok();
    RIMConfig::load().unwrap()
}