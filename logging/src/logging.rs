use std::env;
use chrono::Local;
use dotenvy::dotenv;
use log::{Record, Level, Metadata, SetLoggerError, LevelFilter};

struct RIMLogger;

impl log::Log for RIMLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Trace
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            println!(
                "{} {} - {}",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.args(),
            );
        }
    }

    fn flush(&self) {}
}

static LOGGER: RIMLogger = RIMLogger;

pub fn init_logging() -> Result<(), SetLoggerError> {
    dotenv().ok();
    let level = match env::var("RIM_LOG_LEVEL") {
        Ok(level) => match level.to_lowercase().as_str() {
            "error" => LevelFilter::Error,
            "warn" => LevelFilter::Warn,
            "info" => LevelFilter::Info,
            "debug" => LevelFilter::Debug,
            "trace" => LevelFilter::Trace,
            _ => {
                eprintln!("Unknown log level '{}', using INFO", level);
                LevelFilter::Info
            }
        }
        Err(_) => LevelFilter::Info,
    };

    log::set_logger(&LOGGER)
        .map(|_| log::set_max_level(level))
}
