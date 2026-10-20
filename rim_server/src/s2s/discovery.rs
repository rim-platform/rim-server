//! `GET /.well-known/rim-server`: this server's key log, signed descriptor, features and
//! whether registration is full.

use std::sync::Arc;

use actix_web::{HttpResponse, web};
use rim_identity::SignedStatement;
use rim_services::auth::service::AuthService;
use rim_services::feature::service::FeatureGate;
use serde_json::{Map, Value, json};

use crate::s2s::S2sState;

fn statement(s: &SignedStatement) -> Value {
    json!({ "payload": s.payload, "signature": s.signature })
}

/// Every optional feature by name, with its state and, for an unavailable one, the reason.
/// Unsigned and advisory: the server enforces the gate itself, so a changed copy only changes
/// what a client offers.
fn features(gate: &dyn FeatureGate) -> Value {
    let mut features = Map::new();
    for (feature, state) in gate.states() {
        let mut entry = json!({ "state": state.as_str() });
        if let Some(reason) = state.reason() {
            entry["reason"] = json!(reason);
        }
        features.insert(feature.as_str().to_string(), entry);
    }
    Value::Object(features)
}

/// Whether the server takes new accounts: `full` once it holds as many local accounts as it
/// accepts. Unsigned and advisory beside the descriptor's `registration`, which says how to sign
/// up: every signup counts afresh.
fn registration(full: bool) -> Value {
    json!({ "full": full })
}

/// The discovery document, unauthenticated and cacheable for five minutes by anyone.
pub(crate) async fn handle(
    state: web::Data<S2sState>,
    gate: web::Data<Arc<dyn FeatureGate>>,
    auth: web::Data<Arc<dyn AuthService>>,
) -> HttpResponse {
    let identity = state.identity.current();
    let full = auth.registration_full().await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "could not tell whether registration is full");
        false
    });
    let body = json!({
        "rim": "server-discovery",
        "keyLog": identity.log.iter().map(statement).collect::<Vec<_>>(),
        "descriptor": statement(&identity.descriptor),
        "features": features(gate.get_ref().as_ref()),
        "registration": registration(full),
    });
    HttpResponse::Ok()
        .insert_header(("cache-control", "max-age=300"))
        .insert_header(("access-control-allow-origin", "*"))
        .json(body)
}

#[cfg(test)]
mod tests {
    use rim_core::server_feature::model::ServerFeature;
    use rim_infra_services::feature_gate::InstanceFeatureGate;

    use super::*;

    #[test]
    fn registration_says_whether_it_is_full() {
        assert_eq!(registration(true), json!({ "full": true }));
        assert_eq!(registration(false), json!({ "full": false }));
    }

    #[test]
    fn features_name_every_feature_with_its_state_and_reason() {
        let gate = InstanceFeatureGate::new(
            &[ServerFeature::CustomEmoji],
            &[(ServerFeature::Email, "no SMTP".to_string())],
        );
        let value = features(&gate);
        assert_eq!(value["custom_emoji"], json!({ "state": "disabled" }));
        assert_eq!(
            value["email"],
            json!({ "state": "unavailable", "reason": "no SMTP" })
        );
        assert_eq!(value["attachments"], json!({ "state": "enabled" }));
        assert_eq!(
            value.as_object().map(Map::len),
            Some(ServerFeature::ALL.len())
        );
    }
}
