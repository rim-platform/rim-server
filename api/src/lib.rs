use async_graphql::{EmptyMutation, EmptySubscription, Schema};
use crate::query::Query;

pub mod graphql;
pub mod query;

pub fn new_schema() -> Schema<Query, EmptyMutation, EmptySubscription> {
    Schema::new(Query, EmptyMutation, EmptySubscription)
}