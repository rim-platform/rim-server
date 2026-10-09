//! Identity documents (`GET /s2s/v1/principals/<id>`): built for local users, verified when
//! fetched or pushed, and stored for remote users.

use std::collections::HashSet;

use chrono::Utc;
use rim_core::e2ee::repository::E2eeRepository;
use rim_core::host::model::{NewRemoteUser, UserKind};
use rim_core::host::repository::HostRepository;
use rim_core::key_identity::model::{KeyLogEntryType, NewDeviceKey, NewKeyLogEntry};
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::user::model::UserId;
use rim_identity::{
    DeviceCert, IdentityError, KeyState, ProfileCard, PublicKey, RotationSigner, SignedStatement,
    UnverifiedRevocation, verify_device_cert, verify_profile_card,
};
use rim_services::relay::dto::{IdentityDevice, IdentityDocument, SignedJson};
use rim_services::{ServiceError, ServiceResult};

/// Most entries a key log in a document may have.
pub(crate) const MAX_LOG_ENTRIES: usize = 256;
/// Most devices a document may list.
pub(crate) const MAX_DEVICES: usize = 64;
/// How many characters of the principal id follow `r_` in a remote user's username.
const REMOTE_USERNAME_CHARS: usize = 30;

fn statement(s: &SignedJson) -> SignedStatement {
    SignedStatement {
        payload: s.payload.clone(),
        signature: s.signature.clone(),
    }
}

fn json(payload: &str, signature: &str) -> SignedJson {
    SignedJson {
        payload: payload.to_string(),
        signature: signature.to_string(),
    }
}

/// `r_` and the first 30 characters after `rimp1_`.
pub(crate) fn remote_username(principal_id: &str) -> String {
    let rest = principal_id
        .strip_prefix(rim_identity::PRINCIPAL_ID_PREFIX)
        .unwrap_or(principal_id);
    format!(
        "r_{}",
        rest.chars().take(REMOTE_USERNAME_CHARS).collect::<String>()
    )
}

/// A device of a verified document.
#[derive(Debug, Clone)]
pub(crate) struct VerifiedDevice {
    pub cert: DeviceCert,
    pub certificate: SignedJson,
    /// A revocation that verified.
    pub revocation: Option<SignedJson>,
    pub mls_active: bool,
}

/// A document whose log, certificates, revocations and card verified.
#[derive(Debug, Clone)]
pub(crate) struct VerifiedIdentity {
    pub head: KeyState,
    /// Every entry, inception first, with the state after it and its signer.
    pub entries: Vec<(SignedJson, KeyState, Option<RotationSigner>)>,
    pub devices: Vec<VerifiedDevice>,
    /// The card, when it verifies against the head; a card signed before a rotation is left
    /// out.
    pub card: Option<(ProfileCard, SignedJson)>,
}

fn refused(what: &str, err: IdentityError) -> String {
    format!("{what}: {err}")
}

/// Verify a document: the log from its inception (naming `principal_id`), each certificate
/// against the log at its `keySeq` with no recovery rotation after it, each revocation, and
/// the card against the head.
pub(crate) fn verify_identity(doc: &IdentityDocument) -> Result<VerifiedIdentity, String> {
    if doc.key_log.is_empty() || doc.key_log.len() > MAX_LOG_ENTRIES {
        return Err(format!("a key log has 1 to {MAX_LOG_ENTRIES} entries"));
    }
    if doc.devices.len() > MAX_DEVICES {
        return Err(format!("a document lists at most {MAX_DEVICES} devices"));
    }
    let mut entries = Vec::with_capacity(doc.key_log.len());
    let mut state = KeyState::from_inception(&statement(&doc.key_log[0]))
        .map_err(|e| refused("inception", e))?;
    entries.push((doc.key_log[0].clone(), state.clone(), None));
    for entry in &doc.key_log[1..] {
        let (next, signer) = state
            .apply(&statement(entry))
            .map_err(|e| refused("key log", e))?;
        state = next;
        entries.push((entry.clone(), state.clone(), Some(signer)));
    }
    if state.principal_id != doc.principal_id {
        return Err("the key log is another principal's".to_string());
    }

    let mut seen = HashSet::new();
    let mut devices = Vec::with_capacity(doc.devices.len());
    for device in &doc.devices {
        if !seen.insert(device.device_id) {
            return Err("a device is listed twice".to_string());
        }
        let cert = verify_certificate(&entries, &device.certificate)?;
        if cert.device_id != device.device_id {
            return Err("a certificate names another device".to_string());
        }
        let revocation = match &device.revocation {
            Some(revocation) => {
                verify_revocation(&entries, &cert, revocation)?;
                Some(revocation.clone())
            }
            None => None,
        };
        devices.push(VerifiedDevice {
            cert,
            certificate: device.certificate.clone(),
            revocation,
            mls_active: device.mls_active,
        });
    }

    let card = match &doc.card {
        Some(card) => match verify_profile_card(&state, &statement(card)) {
            Ok(verified) => Some((verified, card.clone())),
            Err(IdentityError::StaleKey { .. }) => None,
            Err(e) => return Err(refused("card", e)),
        },
        None => None,
    };
    Ok(VerifiedIdentity {
        head: state,
        entries,
        devices,
        card,
    })
}

fn verify_certificate(
    entries: &[(SignedJson, KeyState, Option<RotationSigner>)],
    certificate: &SignedJson,
) -> Result<DeviceCert, String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct KeySeq {
        key_seq: u32,
    }
    let key_seq = serde_json::from_str::<KeySeq>(&certificate.payload)
        .map_err(|e| format!("certificate: {e}"))?
        .key_seq as usize;
    let (_, state, _) = entries
        .get(key_seq)
        .ok_or_else(|| "a certificate names a key seq past the log".to_string())?;
    if entries[key_seq + 1..]
        .iter()
        .any(|(_, _, signer)| *signer == Some(RotationSigner::Recovery))
    {
        return Err("a certificate predates a recovery rotation".to_string());
    }
    verify_device_cert(state, &statement(certificate)).map_err(|e| refused("certificate", e))
}

fn verify_revocation(
    entries: &[(SignedJson, KeyState, Option<RotationSigner>)],
    cert: &DeviceCert,
    revocation: &SignedJson,
) -> Result<(), String> {
    let unverified =
        UnverifiedRevocation::read(&statement(revocation)).map_err(|e| refused("revocation", e))?;
    if unverified.device_id != cert.device_id {
        return Err("a revocation names another device".to_string());
    }
    let from = cert.key_seq as usize;
    for (_, state, _) in entries[from..].iter().rev() {
        if unverified.clone().verify(state, &cert.device_key).is_ok() {
            return Ok(());
        }
    }
    Err("a revocation does not verify".to_string())
}

/// A local user's document, or `None` when the user has no key identity.
pub(crate) async fn local_document(
    key_identity_repo: &dyn KeyIdentityRepository,
    e2ee_repo: &dyn E2eeRepository,
    user_id: UserId,
    principal_id: &str,
) -> ServiceResult<Option<IdentityDocument>> {
    let log = key_identity_repo.list_key_log(user_id).await?;
    if log.is_empty() {
        return Ok(None);
    }
    let active: HashSet<uuid::Uuid> = e2ee_repo
        .mls_active_device_ids(user_id, crate::e2ee::active_since(Utc::now()))
        .await?
        .into_iter()
        .collect();
    let devices = key_identity_repo
        .list_devices(user_id)
        .await?
        .into_iter()
        .take(MAX_DEVICES)
        .map(|d| IdentityDevice {
            device_id: d.device_id,
            mls_active: d.revoked_at.is_none() && active.contains(&d.device_id),
            certificate: json(&d.cert_payload, &d.cert_signature),
            revocation: match (&d.revocation_payload, &d.revocation_signature) {
                (Some(p), Some(s)) => Some(json(p, s)),
                _ => None,
            },
        })
        .collect();
    let card = key_identity_repo
        .get_profile_card(user_id)
        .await?
        .map(|c| json(&c.payload, &c.signature));
    Ok(Some(IdentityDocument {
        principal_id: principal_id.to_string(),
        key_log: log
            .into_iter()
            .map(|e| json(&e.payload, &e.signature))
            .collect(),
        devices,
        card,
    }))
}

/// The repositories storing a remote identity, inside the caller's transaction.
pub(crate) struct IdentityStore<'a> {
    pub host_repo: &'a dyn HostRepository,
    pub key_identity_repo: &'a dyn KeyIdentityRepository,
}

/// What storing a remote identity did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stored {
    pub user_id: UserId,
    pub created: bool,
    /// Whether devices were added or revoked, so encrypted groups refresh.
    pub devices_changed: bool,
}

/// Store a verified identity as a remote user: create the user, or extend their log, add new
/// devices, apply revocations and take a newer card. `None` when the principal is a local
/// user here, whose identity is never replaced from outside.
///
/// # Errors
/// `Validation` when the log does not extend the stored one.
pub(crate) async fn store_remote(
    store: &IdentityStore<'_>,
    verified: &VerifiedIdentity,
    home_relay: Option<&str>,
) -> ServiceResult<Option<Stored>> {
    let principal_id = &verified.head.principal_id;
    let display_name = verified
        .card
        .as_ref()
        .map(|(card, _)| card.display_name.clone());
    let new_entries = |from: usize| -> Vec<NewKeyLogEntry> {
        verified.entries[from..]
            .iter()
            .zip(from as u32..)
            .map(|((s, _, _), seq)| NewKeyLogEntry {
                seq,
                entry_type: if seq == 0 {
                    KeyLogEntryType::Inception
                } else {
                    KeyLogEntryType::Rotate
                },
                payload: s.payload.clone(),
                signature: s.signature.clone(),
            })
            .collect()
    };

    let (user_id, created) = match store.host_repo.find_user(principal_id).await? {
        Some((_, UserKind::Local)) => return Ok(None),
        Some((user_id, UserKind::Remote)) => (user_id, false),
        None => {
            let user_id = store
                .host_repo
                .create_remote_user(NewRemoteUser {
                    principal_id: principal_id.clone(),
                    identity_key: verified.head.identity_key.to_string(),
                    username: remote_username(principal_id),
                    display_name: display_name
                        .clone()
                        .unwrap_or_else(|| remote_username(principal_id)),
                    home_relay: home_relay.map(str::to_string),
                })
                .await?;
            store
                .key_identity_repo
                .insert_key_log(user_id, new_entries(0))
                .await?;
            (user_id, true)
        }
    };
    if !store.key_identity_repo.lock_user(user_id).await? {
        return Err(ServiceError::NotFound);
    }

    let mut devices_changed = false;
    if !created {
        let stored = store.key_identity_repo.list_key_log(user_id).await?;
        if stored.len() > verified.entries.len() {
            // The relay serves an older view; keep what is stored.
            return Ok(Some(Stored {
                user_id,
                created,
                devices_changed,
            }));
        }
        let extends = stored
            .iter()
            .zip(&verified.entries)
            .all(|(s, (v, _, _))| s.payload == v.payload && s.signature == v.signature);
        if !extends {
            return Err(ServiceError::Validation(
                "the key log does not extend the stored one".to_string(),
            ));
        }
        let mut recovered = false;
        for entry in new_entries(stored.len()) {
            let index = entry.seq as usize;
            let (_, state, signer) = &verified.entries[index];
            recovered |= *signer == Some(RotationSigner::Recovery);
            store
                .key_identity_repo
                .append_rotation(user_id, entry, &state.identity_key.to_string())
                .await?;
        }
        if recovered {
            devices_changed |= store.key_identity_repo.revoke_all_devices(user_id).await? > 0;
        }
        store
            .host_repo
            .update_remote_user(
                user_id,
                display_name.clone(),
                home_relay.map(str::to_string),
            )
            .await?;
    }

    for device in &verified.devices {
        let row = match store
            .key_identity_repo
            .get_device(user_id, device.cert.device_id)
            .await?
        {
            Some(row) => row,
            None => {
                devices_changed = true;
                store
                    .key_identity_repo
                    .add_device(NewDeviceKey {
                        user_id,
                        device_id: device.cert.device_id,
                        public_key: device.cert.device_key.to_string(),
                        key_hash: device.cert.device_key.key_hash().to_vec(),
                        name: device.cert.name.clone(),
                        cert_payload: device.certificate.payload.clone(),
                        cert_signature: device.certificate.signature.clone(),
                        key_seq: device.cert.key_seq,
                    })
                    .await?
            }
        };
        let revoked = match &device.revocation {
            Some(revocation) if row.revoked_at.is_none() => {
                devices_changed |= store
                    .key_identity_repo
                    .revoke_device(row.id, &revocation.payload, &revocation.signature)
                    .await?;
                true
            }
            _ => row.revoked_at.is_some(),
        };
        let active = device.mls_active && !revoked;
        store
            .host_repo
            .set_device_mls_active(row.id, active)
            .await?;
    }

    if let Some((card, signed)) = &verified.card
        && let Ok(version) = u32::try_from(card.version)
    {
        store
            .key_identity_repo
            .upsert_profile_card(user_id, version, &signed.payload, &signed.signature)
            .await?;
    }
    Ok(Some(Stored {
        user_id,
        created,
        devices_changed,
    }))
}

/// The tagged key of a stored device, for verifying what it signed.
pub(crate) fn device_key(public_key: &str) -> ServiceResult<PublicKey> {
    PublicKey::parse(public_key, "deviceKey")
        .map_err(|e| ServiceError::internal(anyhow::anyhow!("stored device key: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_identity::testing::{TestIdentity, signing_key};
    use uuid::Uuid;

    fn signed(s: &SignedStatement) -> SignedJson {
        json(&s.payload, &s.signature)
    }

    fn document(id: &TestIdentity, state: &KeyState) -> IdentityDocument {
        let cert = id.device_cert(state, Uuid::from_u128(1), &signing_key(20), "Laptop");
        let card = id.profile_card(
            state,
            1,
            "Alice",
            &["https://relay.example"],
            &[(Uuid::from_u128(1), &signing_key(20))],
        );
        IdentityDocument {
            principal_id: state.principal_id.clone(),
            key_log: vec![signed(&id.inception())],
            devices: vec![IdentityDevice {
                device_id: Uuid::from_u128(1),
                certificate: signed(&cert),
                revocation: None,
                mls_active: true,
            }],
            card: Some(signed(&card)),
        }
    }

    #[test]
    fn a_document_verifies_with_its_device_and_card() {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let verified = verify_identity(&document(&id, &state)).expect("verify");
        assert_eq!(verified.head.principal_id, state.principal_id);
        assert_eq!(verified.devices.len(), 1);
        assert_eq!(
            verified.card.as_ref().map(|(c, _)| c.display_name.as_str()),
            Some("Alice")
        );
    }

    #[test]
    fn a_document_naming_another_principal_or_a_foreign_certificate_is_refused() {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let mut doc = document(&id, &state);
        doc.principal_id = "rimp1_someoneelse".to_string();
        assert!(verify_identity(&doc).is_err());

        let other = TestIdentity::new(2);
        let other_state = KeyState::from_inception(&other.inception()).expect("inception");
        let mut doc = document(&id, &state);
        doc.devices[0].certificate = signed(&other.device_cert(
            &other_state,
            Uuid::from_u128(1),
            &signing_key(20),
            "Laptop",
        ));
        assert!(verify_identity(&doc).is_err());

        let mut doc = document(&id, &state);
        doc.devices[0].device_id = Uuid::from_u128(9);
        assert!(verify_identity(&doc).is_err());
    }

    #[test]
    fn a_revocation_must_verify() {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let mut doc = document(&id, &state);
        doc.devices[0].revocation = Some(signed(&id.revoke_device(&state, Uuid::from_u128(1))));
        let verified = verify_identity(&doc).expect("verify");
        assert!(verified.devices[0].revocation.is_some());

        let other = TestIdentity::new(2);
        let other_state = KeyState::from_inception(&other.inception()).expect("inception");
        doc.devices[0].revocation = Some(signed(
            &other.revoke_device(&other_state, Uuid::from_u128(1)),
        ));
        assert!(verify_identity(&doc).is_err());
    }

    #[test]
    fn remote_usernames_take_thirty_characters_after_the_prefix() {
        let name = remote_username("rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0");
        assert_eq!(name, "r_txDLbnipi5i3kCJtJwBOUixzv_MY8O");
        assert_eq!(name.len(), 32);
    }
}
