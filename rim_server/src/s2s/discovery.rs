//! `GET /.well-known/rim-server`: this server's key log and signed descriptor.

use actix_web::{HttpResponse, web};
use rim_identity::SignedStatement;
use serde_json::{Value, json};

use crate::s2s::S2sState;

fn statement(s: &SignedStatement) -> Value {
    json!({ "payload": s.payload, "signature": s.signature })
}

/// The discovery document, unauthenticated and cacheable for five minutes by anyone.
pub(crate) async fn handle(state: web::Data<S2sState>) -> HttpResponse {
    let identity = state.identity.current();
    let body = json!({
        "rim": "server-discovery",
        "keyLog": identity.log.iter().map(statement).collect::<Vec<_>>(),
        "descriptor": statement(&identity.descriptor),
    });
    HttpResponse::Ok()
        .insert_header(("cache-control", "max-age=300"))
        .insert_header(("access-control-allow-origin", "*"))
        .json(body)
}
