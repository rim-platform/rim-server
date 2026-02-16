use crate::server::start_server;
use rim_logging::logging::init_logging;

pub(crate) mod config;
mod server;

fn initialize() {
    // Avoid panicking if logger is already initialized (tests, embedding, etc.).
    let _ = init_logging();
    log::info!("Initializing application...");
    log::info!("Initializing DB");
}

#[actix_web::main]
async fn main() {
    initialize();
    let config = config::load_config();

    start_server(&config).await.unwrap();
}
