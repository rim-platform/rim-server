use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::encoding::PublicKey;
use crate::error::{IdentityError, IdentityResult};
use crate::key_log::KeyState;
use crate::statement::{Purpose, SignedStatement, check_header, check_label, parse, verify};

/// Longest display name on a card, in characters (the same limit as account display names).
pub const MAX_DISPLAY_NAME_CHARS: usize = 80;
/// Most relay URLs a card may list.
pub const MAX_RELAYS: usize = 8;
/// Longest relay URL, in bytes.
pub const MAX_RELAY_URL_LEN: usize = 256;
/// Most devices a card may list.
pub const MAX_CARD_DEVICES: usize = 32;
/// Largest card version, so it fits a GraphQL `Int`.
pub const MAX_CARD_VERSION: u64 = i32::MAX as u64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawCardDevice {
    device_id: Uuid,
    device_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawProfileCard {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    principal_id: String,
    version: u64,
    key_seq: u32,
    display_name: String,
    relays: Vec<String>,
    devices: Vec<RawCardDevice>,
    #[allow(dead_code)]
    issued_at: DateTime<Utc>,
}

/// One device listed on a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardDevice {
    pub device_id: Uuid,
    pub device_key: PublicKey,
}

/// A verified profile card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileCard {
    pub principal_id: String,
    pub version: u64,
    pub key_seq: u32,
    pub display_name: String,
    pub relays: Vec<String>,
    pub devices: Vec<CardDevice>,
}

fn check_relay(url: &str) -> IdentityResult<()> {
    let has_scheme = url.starts_with("https://") || url.starts_with("http://");
    if !has_scheme
        || url.len() > MAX_RELAY_URL_LEN
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(IdentityError::Invalid(format!(
            "relays must be http(s) URLs of at most {MAX_RELAY_URL_LEN} bytes"
        )));
    }
    Ok(())
}

/// Verify a profile card against the head of the principal's key log.
///
/// The card must be signed by the current identity key and name the current `keySeq`. Whether
/// its `version` is newer than a stored card is the caller's check. The listed devices are not
/// checked against any device table: a card is portable between servers, and readers verify
/// each device through its certificate.
pub fn verify_profile_card(
    state: &KeyState,
    card: &SignedStatement,
) -> IdentityResult<ProfileCard> {
    let raw: RawProfileCard = parse(Purpose::ProfileCard, &card.payload)?;
    let version = check_header(raw.v, &raw.kind, "profile-card")?;
    if raw.principal_id != state.principal_id {
        return Err(IdentityError::PrincipalMismatch);
    }
    if raw.key_seq != state.seq {
        return Err(IdentityError::StaleKey {
            current: state.seq,
            got: raw.key_seq,
        });
    }
    if raw.version == 0 || raw.version > MAX_CARD_VERSION {
        return Err(IdentityError::Invalid(format!(
            "version must be 1 to {MAX_CARD_VERSION}"
        )));
    }
    let display_name = check_label(&raw.display_name, "displayName", MAX_DISPLAY_NAME_CHARS)?;

    if raw.relays.len() > MAX_RELAYS {
        return Err(IdentityError::Invalid(format!(
            "a card lists at most {MAX_RELAYS} relays"
        )));
    }
    let mut seen_relays = HashSet::new();
    for relay in &raw.relays {
        check_relay(relay)?;
        if !seen_relays.insert(relay.as_str()) {
            return Err(IdentityError::Invalid("relays must be unique".to_string()));
        }
    }

    if raw.devices.len() > MAX_CARD_DEVICES {
        return Err(IdentityError::Invalid(format!(
            "a card lists at most {MAX_CARD_DEVICES} devices"
        )));
    }
    let mut seen_devices = HashSet::new();
    let mut devices = Vec::with_capacity(raw.devices.len());
    for device in raw.devices {
        if !seen_devices.insert(device.device_id) {
            return Err(IdentityError::Invalid(
                "device ids on a card must be unique".to_string(),
            ));
        }
        devices.push(CardDevice {
            device_id: device.device_id,
            device_key: PublicKey::parse_for(version, &device.device_key, "deviceKey")?,
        });
    }

    verify(Purpose::ProfileCard, version, &state.identity_key, card)?;
    Ok(ProfileCard {
        principal_id: raw.principal_id,
        version: raw.version,
        key_seq: raw.key_seq,
        display_name,
        relays: raw.relays,
        devices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement::StatementVersion::{V1, V2};
    use crate::statement::sign;
    use crate::testing::{SigningKey, TestIdentity, key_text, profile_card_payload, signing_key};

    fn setup() -> (TestIdentity, KeyState) {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        (id, state)
    }

    #[test]
    fn card_verifies_and_lists_devices() {
        let (id, state) = setup();
        let card = id.profile_card(
            &state,
            3,
            "Jakub",
            &["https://relay.rim.chat"],
            &[(Uuid::from_u128(1), &signing_key(20))],
        );
        let verified = verify_profile_card(&state, &card).expect("card");
        assert_eq!(verified.version, 3);
        assert_eq!(verified.display_name, "Jakub");
        assert_eq!(verified.relays, vec!["https://relay.rim.chat".to_string()]);
        assert_eq!(verified.devices.len(), 1);
    }

    #[test]
    fn card_must_be_signed_by_identity_key() {
        let (id, state) = setup();
        let payload = profile_card_payload(V1, &state, 1, "Jakub", &[], &[]);
        let card = sign(Purpose::ProfileCard, V1, &id.recovery, payload);
        assert_eq!(
            verify_profile_card(&state, &card),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn card_rejects_bad_relays_versions_and_duplicates() {
        let (id, state) = setup();
        for relays in [
            vec!["ftp://x"],
            vec!["https://a b"],
            vec!["https://a", "https://a"],
        ] {
            let card = id.profile_card(&state, 1, "J", &relays, &[]);
            assert!(
                matches!(
                    verify_profile_card(&state, &card),
                    Err(IdentityError::Invalid(_))
                ),
                "{relays:?}"
            );
        }
        let card = id.profile_card(&state, 0, "J", &[], &[]);
        assert!(matches!(
            verify_profile_card(&state, &card),
            Err(IdentityError::Invalid(_))
        ));
        let device = signing_key(20);
        let card = id.profile_card(
            &state,
            1,
            "J",
            &[],
            &[(Uuid::from_u128(1), &device), (Uuid::from_u128(1), &device)],
        );
        assert!(matches!(
            verify_profile_card(&state, &card),
            Err(IdentityError::Invalid(_))
        ));
    }

    #[test]
    fn card_under_old_key_is_stale() {
        let (mut id, state) = setup();
        let card = id.profile_card(&state, 1, "J", &[], &[]);
        let rotation = id.rotate_with_identity(&state, 2);
        let (next, _) = state.apply(&rotation).expect("rotate");
        assert!(matches!(
            verify_profile_card(&next, &card),
            Err(IdentityError::StaleKey { .. })
        ));
    }

    #[test]
    fn version_2_card_fits_32_hybrid_sized_devices() {
        let (mut id, state) = setup();
        id.version = V2;
        let keys: Vec<SigningKey> = (0..MAX_CARD_DEVICES as u8)
            .map(|i| signing_key(i + 20))
            .collect();
        let devices: Vec<(Uuid, &SigningKey)> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (Uuid::from_u128(i as u128 + 1), k))
            .collect();
        let card = id.profile_card(&state, 1, "J", &["https://relay.rim.chat"], &devices);
        let verified = verify_profile_card(&state, &card).expect("v2 card");
        assert_eq!(verified.devices.len(), MAX_CARD_DEVICES);

        // A hybrid Ed25519 + ML-DSA-44 key is 32 + 1,312 bytes; tagged, about 1.8 KB of text.
        // Pad each listed key to that size and check the card still fits the cap.
        let hybrid_key_text = format!("ed25519-mldsa44:{}", "A".repeat((32 + 1312) * 4 / 3 + 1));
        let mut padded = card.payload.clone();
        for key in &keys {
            padded = padded.replace(&key_text(V2, key), &hybrid_key_text);
        }
        assert!(padded.len() < Purpose::ProfileCard.max_payload_len());
    }
}
