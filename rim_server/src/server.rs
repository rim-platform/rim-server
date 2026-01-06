use crate::config::RIMConfig;
use actix_web::{App, HttpServer, guard, web};
use rim_api::graphql::index_graphiql;
use rim_api::new_schema;
use async_graphql_actix_web::GraphQL;
use log::{debug, info};

pub async fn start_server(config: &RIMConfig) -> std::io::Result<()> {
    info!("Starting RIM server...");

    debug!("Configuration: {:?}", config);
    let bind_host = format!("{}:{}", config.server_host, config.server_port);
    HttpServer::new(move || {
        let schema = new_schema();
        App::new()
            .service(
                web::resource("/")
                    .guard(guard::Post())
                    .to(GraphQL::new(schema.clone())),
            )
            .service(index_graphiql)
    })
    .bind(bind_host)?
    .run()
    .await
}
