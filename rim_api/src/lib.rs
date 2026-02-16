use crate::mutation::Mutation;
use crate::query::Query;
use async_graphql::{EmptySubscription, Schema};

use std::sync::Arc;

use rim_services::auth::service::AuthService;

pub type AppSchema = Schema<Query, Mutation, EmptySubscription>;

pub mod auth_intent;
pub mod graphql;
pub mod mutation;
pub mod query;

pub fn build_schema(auth_service: Arc<dyn AuthService>) -> AppSchema {
    Schema::build(Query, Mutation, EmptySubscription)
        .data(auth_service)
        .finish()
}
