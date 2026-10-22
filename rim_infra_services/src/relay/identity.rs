//! Identity documents (`GET /s2s/v1/principals/<id>`): built for local users, verified when
//! fetched or pushed, and stored for remote users.

use std::cmp::Reverse;
use std::collections::HashSet;

use chrono::Utc;
use rim_core::auth::repository::SessionRepository;
use rim_core::e2ee::repository::E2eeRepository;
use rim_core::host::model::{NewRemoteUser, UserKind};
use rim_core::host::repository::{DelegationRepository, HostRepository};
use rim_core::key_identity::model::{
    DeviceKey, DeviceKeyId, KeyLogEntryType, NewDeviceKey, NewKeyLogEntry,
};
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::user::model::UserId;
use rim_identity::{
    DeviceCert, IdentityError, KeyState, ProfileCard, PublicKey, RotationSigner, SignedStatement,
    UnverifiedRevocation, entry_signer, verify_device_cert, verify_profile_card,
};
use rim_services::relay::dto::{IdentityDevice, IdentityDocument, SignedJson};
use rim_services::{ServiceError, ServiceResult};

use crate::display_name;
use crate::reserved_names::ReservedNames;

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
    /// The listed devices whose certificates no recovery voided.
    pub devices: Vec<VerifiedDevice>,
    /// The card, when it verifies against the head; a card signed before a rotation is left
    /// out.
    pub card: Option<(ProfileCard, SignedJson)>,
}

fn refused(what: &str, err: IdentityError) -> String {
    format!("{what}: {err}")
}

/// Whether a recovery rotation voids a certificate signed at `key_seq`: some entry after
/// `key_seq` in a log whose entries were signed by `signers` (inception first) is signed by
/// the recovery key.
fn voided_by_recovery(signers: &[Option<RotationSigner>], key_seq: usize) -> bool {
    signers
        .iter()
        .rposition(|signer| *signer == Some(RotationSigner::Recovery))
        .is_some_and(|last| key_seq < last)
}

/// Verify a document: the log from its inception (naming `principal_id`), each certificate
/// against the log at its `keySeq`, each revocation, and the card against the head. A device
/// whose certificate a recovery voided is checked like the others and then left out.
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

    let signers: Vec<_> = entries.iter().map(|(_, _, signer)| *signer).collect();
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
        if voided_by_recovery(&signers, cert.key_seq as usize) {
            continue;
        }
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

/// The devices a document lists, in the order given: those whose certificates no recovery
/// voided, and when more than [`MAX_DEVICES`] remain, the unrevoked ones first, then the most
/// recently revoked.
fn listed_devices(signers: &[Option<RotationSigner>], devices: Vec<DeviceKey>) -> Vec<DeviceKey> {
    let mut devices: Vec<DeviceKey> = devices
        .into_iter()
        .filter(|d| !voided_by_recovery(signers, d.key_seq as usize))
        .collect();
    if devices.len() > MAX_DEVICES {
        let mut order: Vec<usize> = (0..devices.len()).collect();
        order.sort_by_key(|&i| {
            (
                devices[i].revoked_at.is_some(),
                Reverse(devices[i].revoked_at),
                Reverse(i),
            )
        });
        let keep: HashSet<usize> = order.into_iter().take(MAX_DEVICES).collect();
        devices = devices
            .into_iter()
            .enumerate()
            .filter(|(i, _)| keep.contains(i))
            .map(|(_, d)| d)
            .collect();
    }
    devices
}

/// A local user's document, carrying their `username` here, or `None` when the user has no key
/// identity.
///
/// # Errors
/// What the repositories return, and `Internal` when a stored key log entry cannot be read.
pub(crate) async fn local_document(
    key_identity_repo: &dyn KeyIdentityRepository,
    e2ee_repo: &dyn E2eeRepository,
    user_id: UserId,
    principal_id: &str,
    username: Option<String>,
) -> ServiceResult<Option<IdentityDocument>> {
    let log = key_identity_repo.list_key_log(user_id).await?;
    if log.is_empty() {
        return Ok(None);
    }
    let signers = log
        .iter()
        .map(|e| {
            entry_signer(&SignedStatement {
                payload: e.payload.clone(),
                signature: e.signature.clone(),
            })
            .map_err(|e| ServiceError::internal(anyhow::anyhow!("stored key log entry: {e}")))
        })
        .collect::<ServiceResult<Vec<_>>>()?;
    let active: HashSet<uuid::Uuid> = e2ee_repo
        .mls_active_device_ids(user_id, crate::e2ee::active_since(Utc::now()))
        .await?
        .into_iter()
        .collect();
    let devices = listed_devices(&signers, key_identity_repo.list_devices(user_id).await?)
        .into_iter()
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
        username,
    }))
}

/// The repositories storing a remote identity, inside the caller's transaction.
pub(crate) struct IdentityStore<'a> {
    pub host_repo: &'a dyn HostRepository,
    pub key_identity_repo: &'a dyn KeyIdentityRepository,
    /// Ends the sessions of a recovered user or a revoked device.
    pub session_repo: &'a dyn SessionRepository,
    /// Ends the delegations of a recovered user's devices or a revoked device.
    pub delegation_repo: &'a dyn DelegationRepository,
    /// Names a card's display name may not take here.
    pub reserved_names: &'a ReservedNames,
}

/// The display name stored for a remote user from their card's `card_name`: cleaned like a
/// local one and cut to the column's width, or their username here (`r_…`) when nothing visible
/// is left or the name is reserved on this server.
pub(crate) fn card_display_name(
    card_name: &str,
    principal_id: &str,
    reserved_names: &ReservedNames,
) -> String {
    let name = display_name::clean_truncated(card_name);
    if name.is_empty() || reserved_names.is_reserved(&name) {
        return remote_username(principal_id);
    }
    name
}

/// What storing a remote identity did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stored {
    pub user_id: UserId,
    pub created: bool,
    /// Whether devices were added or revoked, so encrypted groups refresh.
    pub devices_changed: bool,
    /// Whether a recovery entry new here was applied, which revoked every device and ended
    /// every session of the user.
    pub recovered: bool,
    /// Devices a revocation in the document newly revoked; their sessions ended.
    pub revoked_devices: Vec<DeviceKeyId>,
    /// How many sessions the recovery and the revocations ended.
    pub ended_sessions: u64,
    /// How many delegations the recovery and the revocations ended.
    pub ended_delegations: u64,
}

/// Store a verified identity as a remote user: create the user, or extend their log, add new
/// devices, apply revocations and take a newer card. `None` when the principal is a local
/// user here, whose identity is never replaced from outside.
///
/// A recovery entry new here revokes every device of the user, ends every session of the user
/// and every delegation of their devices; a revocation new here ends the sessions and
/// delegations of its device.
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
        .map(|(card, _)| card_display_name(&card.display_name, principal_id, store.reserved_names));
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

    let mut result = Stored {
        user_id,
        created,
        devices_changed: false,
        recovered: false,
        revoked_devices: Vec::new(),
        ended_sessions: 0,
        ended_delegations: 0,
    };
    if !created {
        let stored = store.key_identity_repo.list_key_log(user_id).await?;
        if stored.len() > verified.entries.len() {
            // The relay serves an older view; keep what is stored.
            return Ok(Some(result));
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
            result.recovered = true;
            result.devices_changed |=
                store.key_identity_repo.revoke_all_devices(user_id).await? > 0;
            for device in store.key_identity_repo.list_devices(user_id).await? {
                result.ended_delegations += store.delegation_repo.end_for_device(device.id).await?;
            }
            result.ended_sessions += store.session_repo.revoke_all_for_user(user_id).await?;
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
                result.devices_changed = true;
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
                if store
                    .key_identity_repo
                    .revoke_device(row.id, &revocation.payload, &revocation.signature)
                    .await?
                {
                    result.devices_changed = true;
                    result.revoked_devices.push(row.id);
                    result.ended_delegations +=
                        store.delegation_repo.end_for_device(row.id).await?;
                    result.ended_sessions +=
                        store.session_repo.revoke_all_for_device(row.id).await?;
                }
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
    Ok(Some(result))
}

/// The tagged key of a stored device, for verifying what it signed.
pub(crate) fn device_key(public_key: &str) -> ServiceResult<PublicKey> {
    PublicKey::parse(public_key, "deviceKey")
        .map_err(|e| ServiceError::internal(anyhow::anyhow!("stored device key: {e}")))
}

/// Signed identity documents for tests.
#[cfg(test)]
pub(crate) mod testing {
    use rim_identity::testing::{TestIdentity, signing_key};
    use uuid::Uuid;

    use super::*;

    pub(crate) fn signed(s: &SignedStatement) -> SignedJson {
        json(&s.payload, &s.signature)
    }

    /// The identity of `id` at `state` with one device and a profile card, all verifying.
    pub(crate) fn document(id: &TestIdentity, state: &KeyState) -> IdentityDocument {
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
            username: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::{DateTime, Duration};
    use rim_core::auth::repository::MockSessionRepository;
    use rim_core::e2ee::repository::MockE2eeRepository;
    use rim_core::host::repository::{MockDelegationRepository, MockHostRepository};
    use rim_core::key_identity::model::KeyLogEntry;
    use rim_core::key_identity::repository::MockKeyIdentityRepository;
    use rim_identity::testing::{TestIdentity, signing_key};
    use uuid::Uuid;

    use super::testing::{document, signed};
    use super::*;

    /// A stored device row for `cert`, revoked at `revoked_at` when given.
    fn row(
        user_id: UserId,
        device_id: Uuid,
        cert: &SignedStatement,
        key_seq: u32,
        revoked_at: Option<DateTime<Utc>>,
    ) -> DeviceKey {
        DeviceKey {
            id: DeviceKeyId::new(),
            user_id,
            device_id,
            public_key: String::new(),
            name: "Device".to_string(),
            cert_payload: cert.payload.clone(),
            cert_signature: cert.signature.clone(),
            key_seq,
            added_at: Utc::now(),
            last_used_at: None,
            revoked_at,
            revocation_payload: None,
            revocation_signature: None,
        }
    }

    fn log_entry(user_id: UserId, seq: u32, entry: &SignedStatement) -> KeyLogEntry {
        KeyLogEntry {
            user_id,
            seq,
            entry_type: if seq == 0 {
                KeyLogEntryType::Inception
            } else {
                KeyLogEntryType::Rotate
            },
            payload: entry.payload.clone(),
            signature: entry.signature.clone(),
            created_at: Utc::now(),
        }
    }

    /// The document `local_document` builds from `log` and `devices`, every unrevoked device
    /// uploading key packages.
    async fn exported(
        principal_id: &str,
        log: Vec<KeyLogEntry>,
        devices: Vec<DeviceKey>,
    ) -> IdentityDocument {
        let active: Vec<Uuid> = devices
            .iter()
            .filter(|d| d.revoked_at.is_none())
            .map(|d| d.device_id)
            .collect();
        let mut keys = MockKeyIdentityRepository::new();
        keys.expect_list_key_log().returning(move |_| {
            let log = log.clone();
            Box::pin(async move { Ok(log) })
        });
        keys.expect_list_devices().returning(move |_| {
            let devices = devices.clone();
            Box::pin(async move { Ok(devices) })
        });
        keys.expect_get_profile_card()
            .returning(|_| Box::pin(async { Ok(None) }));
        let mut e2ee = MockE2eeRepository::new();
        e2ee.expect_mls_active_device_ids().returning(move |_, _| {
            let active = active.clone();
            Box::pin(async move { Ok(active) })
        });
        local_document(&keys, &e2ee, UserId::new(), principal_id, None)
            .await
            .expect("document")
            .expect("a key identity")
    }

    /// An identity with `old` devices certified at seq 0, revoked by a recovery at seq 1 that
    /// enrolled one new device: the log, the stored rows and the new device's id.
    fn recovered(old: usize) -> (String, Vec<KeyLogEntry>, Vec<DeviceKey>, Uuid) {
        let user_id = UserId::new();
        let mut id = TestIdentity::new(1);
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).expect("inception");
        let recovered_at = Utc::now();
        let mut devices: Vec<DeviceKey> = (0..old)
            .map(|n| {
                let device_id = Uuid::from_u128(n as u128 + 1);
                let cert = id.device_cert(&state, device_id, &signing_key(20), "Old");
                row(user_id, device_id, &cert, 0, Some(recovered_at))
            })
            .collect();
        let recovery = id.recover(&state, 2);
        let next = state.apply(&recovery).expect("recovery").0;
        let new_id = Uuid::from_u128(1000);
        let cert = id.device_cert(&next, new_id, &signing_key(21), "New");
        devices.push(row(user_id, new_id, &cert, 1, None));
        let log = vec![
            log_entry(user_id, 0, &inception),
            log_entry(user_id, 1, &recovery),
        ];
        (state.principal_id, log, devices, new_id)
    }

    #[test]
    fn a_recovery_voids_certificates_signed_before_it() {
        use RotationSigner::{Identity, Recovery};
        let signers = [None, Some(Identity), Some(Recovery), Some(Identity)];
        assert!(voided_by_recovery(&signers, 0));
        assert!(voided_by_recovery(&signers, 1));
        assert!(!voided_by_recovery(&signers, 2));
        assert!(!voided_by_recovery(&signers, 3));
        assert!(!voided_by_recovery(&[None, Some(Identity)], 0));
    }

    #[tokio::test]
    async fn a_recovered_identity_exports_only_its_new_device_and_verifies() {
        let (principal_id, log, devices, new_id) = recovered(1);
        let doc = exported(&principal_id, log, devices).await;
        assert_eq!(doc.devices.len(), 1);
        assert_eq!(doc.devices[0].device_id, new_id);
        assert!(doc.devices[0].mls_active);
        let verified = verify_identity(&doc).expect("verify");
        assert_eq!(verified.head.seq, 1);
        assert_eq!(verified.devices.len(), 1);
        assert_eq!(verified.devices[0].cert.device_id, new_id);
    }

    #[tokio::test]
    async fn a_recovered_identity_keeps_its_new_device_past_the_device_cap() {
        let (principal_id, log, devices, new_id) = recovered(MAX_DEVICES + 6);
        let doc = exported(&principal_id, log, devices).await;
        let ids: Vec<Uuid> = doc.devices.iter().map(|d| d.device_id).collect();
        assert_eq!(ids, vec![new_id]);
        assert!(verify_identity(&doc).is_ok());
    }

    #[test]
    fn past_the_cap_unrevoked_devices_stay_then_the_most_recently_revoked() {
        let user_id = UserId::new();
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let cert = id.device_cert(&state, Uuid::from_u128(1), &signing_key(20), "Device");
        let start = Utc::now();
        let total = MAX_DEVICES + 6;
        let devices: Vec<DeviceKey> = (0..total)
            .map(|n| {
                let revoked_at =
                    (n != 0 && n != total - 1).then(|| start + Duration::minutes(n as i64));
                row(user_id, Uuid::from_u128(n as u128), &cert, 0, revoked_at)
            })
            .collect();
        let kept: Vec<u128> = listed_devices(&[None], devices)
            .iter()
            .map(|d| d.device_id.as_u128())
            .collect();
        let expected: Vec<u128> = std::iter::once(0).chain(7..total as u128).collect();
        assert_eq!(kept, expected);
    }

    #[test]
    fn a_voided_certificate_is_left_out_and_a_bad_signature_is_still_refused() {
        let mut id = TestIdentity::new(1);
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).expect("inception");
        let old = id.device_cert(&state, Uuid::from_u128(1), &signing_key(20), "Old");
        let recovery = id.recover(&state, 2);
        let next = state.apply(&recovery).expect("recovery").0;
        let new = id.device_cert(&next, Uuid::from_u128(2), &signing_key(21), "New");
        let device = |device_id: u128, cert: &SignedStatement| IdentityDevice {
            device_id: Uuid::from_u128(device_id),
            certificate: signed(cert),
            revocation: None,
            mls_active: true,
        };
        let mut doc = IdentityDocument {
            principal_id: next.principal_id.clone(),
            key_log: vec![signed(&inception), signed(&recovery)],
            devices: vec![device(1, &old), device(2, &new)],
            card: None,
            username: None,
        };
        let verified = verify_identity(&doc).expect("verify");
        let ids: Vec<Uuid> = verified.devices.iter().map(|d| d.cert.device_id).collect();
        assert_eq!(ids, vec![Uuid::from_u128(2)]);

        doc.devices[0].certificate.signature = new.signature.clone();
        assert!(verify_identity(&doc).is_err());
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

    /// What storing ended: the users whose sessions all ended, the devices whose sessions
    /// ended, and the devices whose delegations ended.
    #[derive(Default)]
    struct Ended {
        user_sessions: Mutex<Vec<UserId>>,
        device_sessions: Mutex<Vec<DeviceKeyId>>,
        delegations: Mutex<Vec<DeviceKeyId>>,
        /// The display name each update of the remote user stored.
        display_names: Mutex<Vec<Option<String>>>,
    }

    impl Ended {
        fn user_sessions(&self) -> Vec<UserId> {
            self.user_sessions.lock().expect("lock").clone()
        }

        fn device_sessions(&self) -> Vec<DeviceKeyId> {
            self.device_sessions.lock().expect("lock").clone()
        }

        fn delegations(&self) -> Vec<DeviceKeyId> {
            self.delegations.lock().expect("lock").clone()
        }

        fn display_names(&self) -> Vec<Option<String>> {
            self.display_names.lock().expect("lock").clone()
        }
    }

    /// Store `doc` for the remote user `user_id`, held here with `log` and the device rows
    /// `devices`. Every session query ends one session, every delegation query one delegation.
    async fn store_held(
        user_id: UserId,
        log: &[SignedStatement],
        devices: Vec<DeviceKey>,
        doc: &IdentityDocument,
    ) -> (Stored, Arc<Ended>) {
        let verified = verify_identity(doc).expect("verify");
        let ended = Arc::new(Ended::default());

        let mut host = MockHostRepository::new();
        host.expect_find_user()
            .returning(move |_| Box::pin(async move { Ok(Some((user_id, UserKind::Remote))) }));
        let e = Arc::clone(&ended);
        host.expect_update_remote_user()
            .returning(move |_, display_name, _| {
                e.display_names.lock().expect("lock").push(display_name);
                Box::pin(async { Ok(()) })
            });
        host.expect_set_device_mls_active()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let mut keys = MockKeyIdentityRepository::new();
        keys.expect_lock_user()
            .returning(|_| Box::pin(async { Ok(true) }));
        let stored_log: Vec<KeyLogEntry> = log
            .iter()
            .zip(0..)
            .map(|(entry, seq)| log_entry(user_id, seq, entry))
            .collect();
        keys.expect_list_key_log().returning(move |_| {
            let log = stored_log.clone();
            Box::pin(async move { Ok(log) })
        });
        keys.expect_append_rotation()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));
        let unrevoked = devices.iter().filter(|d| d.revoked_at.is_none()).count() as u64;
        keys.expect_revoke_all_devices()
            .returning(move |_| Box::pin(async move { Ok(unrevoked) }));
        let rows = devices.clone();
        keys.expect_list_devices().returning(move |_| {
            let rows = rows.clone();
            Box::pin(async move { Ok(rows) })
        });
        keys.expect_get_device().returning(move |_, device_id| {
            let row = devices.iter().find(|d| d.device_id == device_id).cloned();
            Box::pin(async move { Ok(row) })
        });
        keys.expect_add_device().returning(move |new| {
            let cert = SignedStatement {
                payload: new.cert_payload.clone(),
                signature: new.cert_signature.clone(),
            };
            let added = row(user_id, new.device_id, &cert, new.key_seq, None);
            Box::pin(async move { Ok(added) })
        });
        keys.expect_revoke_device()
            .returning(|_, _, _| Box::pin(async { Ok(true) }));
        keys.expect_upsert_profile_card()
            .returning(|_, _, _, _| Box::pin(async { Ok(true) }));

        let mut sessions = MockSessionRepository::new();
        let e = Arc::clone(&ended);
        sessions
            .expect_revoke_all_for_user()
            .returning(move |user| {
                e.user_sessions.lock().expect("lock").push(user);
                Ok(1)
            });
        let e = Arc::clone(&ended);
        sessions
            .expect_revoke_all_for_device()
            .returning(move |device| {
                e.device_sessions.lock().expect("lock").push(device);
                Ok(1)
            });
        let mut delegations = MockDelegationRepository::new();
        let e = Arc::clone(&ended);
        delegations
            .expect_end_for_device()
            .returning(move |device| {
                e.delegations.lock().expect("lock").push(device);
                Box::pin(async { Ok(1) })
            });

        let stored = store_remote(
            &IdentityStore {
                host_repo: &host,
                key_identity_repo: &keys,
                session_repo: &sessions,
                delegation_repo: &delegations,
                reserved_names: &ReservedNames::parse("rim,admin").expect("names"),
            },
            &verified,
            None,
        )
        .await
        .expect("store")
        .expect("a remote user");
        (stored, ended)
    }

    #[tokio::test]
    async fn a_recovery_new_here_ends_every_session_and_delegation_of_the_user() {
        let user_id = UserId::new();
        let mut id = TestIdentity::new(1);
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).expect("inception");
        let laptop = id.device_cert(&state, Uuid::from_u128(1), &signing_key(20), "Laptop");
        let phone = id.device_cert(&state, Uuid::from_u128(2), &signing_key(21), "Phone");
        let rows = vec![
            row(user_id, Uuid::from_u128(1), &laptop, 0, None),
            row(user_id, Uuid::from_u128(2), &phone, 0, None),
        ];
        let old: Vec<DeviceKeyId> = rows.iter().map(|d| d.id).collect();
        let recovery = id.recover(&state, 2);
        let next = state.apply(&recovery).expect("recovery").0;
        let new = id.device_cert(&next, Uuid::from_u128(3), &signing_key(22), "New");
        let doc = IdentityDocument {
            principal_id: next.principal_id.clone(),
            key_log: vec![signed(&inception), signed(&recovery)],
            devices: vec![IdentityDevice {
                device_id: Uuid::from_u128(3),
                certificate: signed(&new),
                revocation: None,
                mls_active: true,
            }],
            card: None,
            username: None,
        };

        let (stored, ended) = store_held(user_id, &[inception], rows, &doc).await;
        assert!(stored.recovered);
        assert!(stored.devices_changed);
        assert!(stored.revoked_devices.is_empty());
        assert_eq!(ended.user_sessions(), vec![user_id]);
        assert_eq!(ended.delegations(), old);
        assert!(ended.device_sessions().is_empty());
        assert_eq!(stored.ended_sessions, 1);
        assert_eq!(stored.ended_delegations, 2);
    }

    #[tokio::test]
    async fn a_revocation_new_here_ends_only_its_devices_sessions_and_delegations() {
        let user_id = UserId::new();
        let id = TestIdentity::new(1);
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).expect("inception");
        let laptop = id.device_cert(&state, Uuid::from_u128(1), &signing_key(20), "Laptop");
        let phone = id.device_cert(&state, Uuid::from_u128(2), &signing_key(21), "Phone");
        let rows = vec![
            row(user_id, Uuid::from_u128(1), &laptop, 0, None),
            row(user_id, Uuid::from_u128(2), &phone, 0, None),
        ];
        let phone_row = rows[1].id;
        let mut doc = document(&id, &state);
        doc.devices.push(IdentityDevice {
            device_id: Uuid::from_u128(2),
            certificate: signed(&phone),
            revocation: Some(signed(&id.revoke_device(&state, Uuid::from_u128(2)))),
            mls_active: false,
        });

        let (stored, ended) = store_held(user_id, &[inception], rows, &doc).await;
        assert!(!stored.recovered);
        assert!(stored.devices_changed);
        assert_eq!(stored.revoked_devices, vec![phone_row]);
        assert_eq!(ended.device_sessions(), vec![phone_row]);
        assert_eq!(ended.delegations(), vec![phone_row]);
        assert!(ended.user_sessions().is_empty());
        assert_eq!((stored.ended_sessions, stored.ended_delegations), (1, 1));
    }

    #[tokio::test]
    async fn an_identity_with_nothing_new_ends_nothing() {
        let user_id = UserId::new();
        let id = TestIdentity::new(1);
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).expect("inception");
        let laptop = id.device_cert(&state, Uuid::from_u128(1), &signing_key(20), "Laptop");
        let phone = id.device_cert(&state, Uuid::from_u128(2), &signing_key(21), "Phone");
        let rows = vec![
            row(user_id, Uuid::from_u128(1), &laptop, 0, None),
            row(user_id, Uuid::from_u128(2), &phone, 0, Some(Utc::now())),
        ];
        let mut doc = document(&id, &state);
        doc.devices.push(IdentityDevice {
            device_id: Uuid::from_u128(2),
            certificate: signed(&phone),
            revocation: Some(signed(&id.revoke_device(&state, Uuid::from_u128(2)))),
            mls_active: false,
        });

        let (stored, ended) = store_held(user_id, &[inception], rows, &doc).await;
        assert!(!stored.recovered);
        assert!(!stored.devices_changed);
        assert!(stored.revoked_devices.is_empty());
        assert!(ended.user_sessions().is_empty());
        assert!(ended.device_sessions().is_empty());
        assert!(ended.delegations().is_empty());
        assert_eq!((stored.ended_sessions, stored.ended_delegations), (0, 0));
    }

    #[test]
    fn remote_usernames_take_thirty_characters_after_the_prefix() {
        let name = remote_username("rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0");
        assert_eq!(name, "r_txDLbnipi5i3kCJtJwBOUixzv_MY8O");
        assert_eq!(name.len(), 32);
    }

    #[test]
    fn a_card_display_name_is_cleaned_cut_and_never_reserved() {
        let reserved = ReservedNames::parse("rim,admin,support").expect("names");
        let principal = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
        let fallback = remote_username(principal);
        for (card, stored) in [
            ("Ada \u{200B} Lovelace", "Ada Lovelace".to_string()),
            ("\u{202E}nimda", "nimda".to_string()),
            (&"x".repeat(80), "x".repeat(64)),
            ("Ad\u{200B}min", fallback.clone()),
            ("\u{2066}RIM\u{2069} Support", fallback.clone()),
            ("\u{410}dmin", fallback.clone()),
            ("\u{FEFF}", fallback.clone()),
        ] {
            assert_eq!(
                card_display_name(card, principal, &reserved),
                stored,
                "{card:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_stored_card_takes_its_cleaned_name_or_the_username_when_reserved() {
        let user_id = UserId::new();
        let id = TestIdentity::new(1);
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).expect("inception");
        let laptop = id.device_cert(&state, Uuid::from_u128(1), &signing_key(20), "Laptop");
        for (card_name, stored) in [
            ("Ali\u{200B}ce", "Alice".to_string()),
            ("A\u{AD}dmin", remote_username(&state.principal_id)),
        ] {
            let mut doc = document(&id, &state);
            doc.card = Some(signed(&id.profile_card(
                &state,
                1,
                card_name,
                &["https://relay.example"],
                &[(Uuid::from_u128(1), &signing_key(20))],
            )));
            let rows = vec![row(user_id, Uuid::from_u128(1), &laptop, 0, None)];
            let (_, ended) =
                store_held(user_id, std::slice::from_ref(&inception), rows, &doc).await;
            assert_eq!(ended.display_names(), vec![Some(stored)], "{card_name:?}");
        }
    }
}
