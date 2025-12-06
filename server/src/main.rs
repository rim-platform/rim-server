use logging::logging::init_logging;
use crate::server::start_server;

mod server;
pub(crate) mod config;

fn initialize() {
    init_logging().expect("Unable to init logger");
    log::info!("Initializing application...");

    log::info!("Initializing DB");


}

#[actix_web::main]
async fn main() {
    initialize();
    let config = config::load_config();

    start_server(&config).await.unwrap();
}
