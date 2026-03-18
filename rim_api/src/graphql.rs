use actix_web::{HttpResponse, Result, get};
use async_graphql::http::GraphiQLSource;

#[get("/api")]
pub async fn index_graphiql() -> Result<HttpResponse> {
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(
            GraphiQLSource::build()
                .endpoint("/api")
                .subscription_endpoint("/api")
                .finish(),
        ))
}
