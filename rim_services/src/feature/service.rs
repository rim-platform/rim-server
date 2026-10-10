use rim_core::server_feature::model::{FeatureState, ServerFeature};

use crate::{ServiceError, ServiceResult};

/// The instance's feature switches, fixed at boot: which optional modules work here.
pub trait FeatureGate: Send + Sync {
    /// The state of `feature` on this instance.
    fn state(&self, feature: ServerFeature) -> FeatureState;

    /// `Ok` when `feature` is enabled; [`ServiceError::FeatureDisabled`] when it is disabled or
    /// unavailable.
    fn require(&self, feature: ServerFeature) -> ServiceResult<()> {
        if self.state(feature).is_enabled() {
            Ok(())
        } else {
            Err(ServiceError::FeatureDisabled(feature))
        }
    }

    /// Every feature with its state, in [`ServerFeature::ALL`] order.
    fn states(&self) -> Vec<(ServerFeature, FeatureState)> {
        ServerFeature::ALL
            .into_iter()
            .map(|feature| (feature, self.state(feature)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed;

    impl FeatureGate for Fixed {
        fn state(&self, feature: ServerFeature) -> FeatureState {
            match feature {
                ServerFeature::Attachments => FeatureState::Disabled,
                ServerFeature::Email => FeatureState::Unavailable {
                    reason: "no SMTP".to_string(),
                },
                _ => FeatureState::Enabled,
            }
        }
    }

    #[test]
    fn require_passes_only_an_enabled_feature() {
        assert!(Fixed.require(ServerFeature::CustomEmoji).is_ok());
        assert!(matches!(
            Fixed.require(ServerFeature::Attachments),
            Err(ServiceError::FeatureDisabled(ServerFeature::Attachments))
        ));
        assert!(matches!(
            Fixed.require(ServerFeature::Email),
            Err(ServiceError::FeatureDisabled(ServerFeature::Email))
        ));
    }

    #[test]
    fn states_lists_every_feature_in_order() {
        let states = Fixed.states();
        let features: Vec<ServerFeature> = states.iter().map(|(f, _)| *f).collect();
        assert_eq!(features, ServerFeature::ALL.to_vec());
        assert_eq!(states[0].1, FeatureState::Disabled);
    }
}
