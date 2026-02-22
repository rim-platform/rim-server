use dotenvy::dotenv;
use std::error::Error as StdError;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::UtcTime;

pub fn init_logging() -> Result<(), Box<dyn StdError + Send + Sync + 'static>> {
    dotenv().ok();

    // Capture `log::info!` etc. and route into tracing.
    tracing_log::LogTracer::init()?;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_timer(UtcTime::rfc_3339())
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_current_span(true)
        .with_span_list(true)
        .try_init()?;

    Ok(())
}

pub fn init_tracing_json() -> Result<(), Box<dyn StdError + Send + Sync + 'static>> {
    init_logging()
}
