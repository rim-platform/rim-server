use std::collections::BTreeMap;

use rim_core::server_feature::model::{FeatureState, ServerFeature};
use rim_services::feature::service::FeatureGate;

/// The instance's feature states, computed once at boot from the features the operator
/// disabled and the ones the configuration cannot support.
#[derive(Debug, Clone, Default)]
pub struct InstanceFeatureGate {
    /// Features that are not enabled; any other feature is.
    off: BTreeMap<ServerFeature, FeatureState>,
}

impl InstanceFeatureGate {
    /// Every feature enabled.
    pub fn all_enabled() -> Self {
        Self::default()
    }

    /// `disabled` are switched off by the operator; `unavailable` lack something in the
    /// configuration, with the reason. A feature in both is disabled: the operator's choice is
    /// the one to report.
    pub fn new(disabled: &[ServerFeature], unavailable: &[(ServerFeature, String)]) -> Self {
        let mut off = BTreeMap::new();
        for (feature, reason) in unavailable {
            off.insert(
                *feature,
                FeatureState::Unavailable {
                    reason: reason.clone(),
                },
            );
        }
        for feature in disabled {
            off.insert(*feature, FeatureState::Disabled);
        }
        Self { off }
    }

    /// The disabled features, in [`ServerFeature::ALL`] order.
    pub fn disabled(&self) -> Vec<ServerFeature> {
        self.off
            .iter()
            .filter(|(_, state)| state.is_disabled())
            .map(|(feature, _)| *feature)
            .collect()
    }

    /// The unavailable features with their reasons, in [`ServerFeature::ALL`] order.
    pub fn unavailable(&self) -> Vec<(ServerFeature, String)> {
        self.off
            .iter()
            .filter_map(|(feature, state)| state.reason().map(|r| (*feature, r.to_string())))
            .collect()
    }
}

impl FeatureGate for InstanceFeatureGate {
    fn state(&self, feature: ServerFeature) -> FeatureState {
        self.off
            .get(&feature)
            .cloned()
            .unwrap_or(FeatureState::Enabled)
    }
}

#[cfg(test)]
mod tests {
    use rim_services::ServiceError;

    use super::*;

    fn unavailable_email() -> (ServerFeature, String) {
        (ServerFeature::Email, "SMTP is not configured".to_string())
    }

    #[test]
    fn everything_is_enabled_by_default() {
        let gate = InstanceFeatureGate::all_enabled();
        for feature in ServerFeature::ALL {
            assert_eq!(gate.state(feature), FeatureState::Enabled);
            assert!(gate.require(feature).is_ok());
        }
        assert!(gate.disabled().is_empty());
        assert!(gate.unavailable().is_empty());
    }

    #[test]
    fn disabled_and_unavailable_features_get_their_states() {
        let gate = InstanceFeatureGate::new(
            &[ServerFeature::CustomEmoji, ServerFeature::Attachments],
            &[unavailable_email()],
        );
        assert_eq!(
            gate.state(ServerFeature::Attachments),
            FeatureState::Disabled
        );
        assert_eq!(
            gate.state(ServerFeature::CustomEmoji),
            FeatureState::Disabled
        );
        assert_eq!(
            gate.state(ServerFeature::Email),
            FeatureState::Unavailable {
                reason: "SMTP is not configured".to_string()
            }
        );
        assert_eq!(
            gate.state(ServerFeature::LinkPreviews),
            FeatureState::Enabled
        );
        assert_eq!(
            gate.disabled(),
            vec![ServerFeature::Attachments, ServerFeature::CustomEmoji]
        );
        assert_eq!(gate.unavailable(), vec![unavailable_email()]);
    }

    #[test]
    fn disabled_wins_over_unavailable() {
        let gate = InstanceFeatureGate::new(&[ServerFeature::Email], &[unavailable_email()]);
        assert_eq!(gate.state(ServerFeature::Email), FeatureState::Disabled);
        assert!(gate.unavailable().is_empty());
    }

    #[test]
    fn require_refuses_disabled_and_unavailable_features() {
        let gate = InstanceFeatureGate::new(&[ServerFeature::Attachments], &[unavailable_email()]);
        assert!(gate.require(ServerFeature::CustomEmoji).is_ok());
        for feature in [ServerFeature::Attachments, ServerFeature::Email] {
            match gate.require(feature) {
                Err(ServiceError::FeatureDisabled(f)) => assert_eq!(f, feature),
                other => panic!("{feature}: {other:?}"),
            }
        }
    }
}
