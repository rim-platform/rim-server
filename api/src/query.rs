use async_graphql::*;

pub struct Query;

#[Object]
impl Query {
    async fn hello_world(&self, _ctx: &Context<'_>) -> String {
        "Hello world!".to_string()
    }
}
