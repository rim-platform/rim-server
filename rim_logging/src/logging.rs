use dotenvy::dotenv;
use std::env;
use std::error::Error as StdError;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::UtcTime;

pub fn init_logging() -> Result<(), Box<dyn StdError + Send + Sync + 'static>> {
    dotenv().ok();

    // Capture `log::info!` etc. and route into tracing.
    tracing_log::LogTracer::init()?;

    // Best practice: honor `RUST_LOG` if present, otherwise fall back to `RIM_LOG_LEVEL`.
    let filter_str = env::var("RUST_LOG")
        .or_else(|_| env::var("RIM_LOG_LEVEL"))
        .unwrap_or_else(|_| "info".to_string());
    let filter = EnvFilter::try_new(filter_str).unwrap_or_else(|_| EnvFilter::new("info"));

    let format = env::var("RIM_LOG_FORMAT")
        .unwrap_or_else(|_| "json".to_string())
        .to_lowercase();

    // JSON is the default for production and log aggregation.
    if format == "pretty" {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .pretty()
            .with_timer(UtcTime::rfc_3339())
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .try_init()?;
    } else {
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
    }

    Ok(())
}
