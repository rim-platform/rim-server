use crate::server::start_server;
use rim_logging::init_tracing_json;

mod app;
pub(crate) mod config;
mod file_handlers;
mod graphql_support;
mod server;

fn initialize() {
    // Avoid panicking if logger is already initialized (tests, embedding, etc.).
    let _ = init_tracing_json();
    log::info!("Initializing application...");
    log::info!("Initializing DB");
}

#[actix_web::main]
async fn main() {
    initialize();
    let config = config::load_config();

    start_server(&config).await.unwrap();
}
