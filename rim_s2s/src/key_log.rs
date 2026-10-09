use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rim_identity::{PublicKey, SignedStatement};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{StatementError, StatementResult};
use crate::signer::ServerSigner;
use crate::statement::{STATEMENT_VERSION, ServerStatement, parse_payload, verify};

/// Prefix of a server id.
pub const SERVER_ID_PREFIX: &str = "rims1_";

/// Domain separator hashed in front of the inception payload to form the server id.
const SERVER_ID_DOMAIN: &[u8] = b"rim.server.v1\0";

/// Most entries a server key log may have.
pub const MAX_LOG_ENTRIES: usize = 256;

/// Length of a server id: the prefix plus 43 characters of unpadded base64url SHA-256.
const SERVER_ID_LEN: usize = SERVER_ID_PREFIX.len() + 43;

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// The server id fixed by an inception payload: `rims1_` plus the unpadded base64url SHA-256 of
/// a domain separator and the exact payload bytes.
pub fn server_id_for_inception(payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SERVER_ID_DOMAIN);
    hasher.update(payload.as_bytes());
    format!("{SERVER_ID_PREFIX}{}", b64(&hasher.finalize()))
}

/// Whether `s` has the shape of a server id. Says nothing about whether it exists.
pub fn is_server_id(s: &str) -> bool {
    s.len() == SERVER_ID_LEN
        && s.strip_prefix(SERVER_ID_PREFIX).is_some_and(|rest| {
            URL_SAFE_NO_PAD
                .decode(rest)
                .is_ok_and(|bytes| bytes.len() == 32)
        })
}

/// Unpadded base64url SHA-256 of a key's tagged text, as `device_key.key_hash` and invites spell
/// it.
pub fn key_hash(key: &PublicKey) -> String {
    b64(&key.key_hash())
}

/// Who signed a server key rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerRotationSigner {
    /// The current server key. The recovery key stays; peers keep accepting the previous key
    /// for a grace period.
    Server,
    /// The current recovery key: the server key was lost or stolen. The recovery key may
    /// change; peers stop accepting the previous key at once.
    Recovery,
}

impl ServerRotationSigner {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Recovery => "recovery",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "server" => Some(Self::Server),
            "recovery" => Some(Self::Recovery),
            _ => None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InceptionOut<'a> {
    v: u32,
    #[serde(rename = "type")]
    kind: &'a str,
    seq: u32,
    server_key: String,
    recovery_key: String,
    created_at: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RotationOut<'a> {
    v: u32,
    #[serde(rename = "type")]
    kind: &'a str,
    server_id: &'a str,
    seq: u32,
    prev: String,
    server_key: String,
    recovery_key: String,
    signer: ServerRotationSigner,
    created_at: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawInception {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    seq: u32,
    server_key: String,
    recovery_key: String,
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawRotation {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    server_id: String,
    seq: u32,
    prev: String,
    server_key: String,
    recovery_key: String,
    signer: ServerRotationSigner,
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
}

/// RFC 3339 text for a statement timestamp, to the second.
pub fn timestamp(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn check_header(v: u32, kind: &str, expected: &'static str) -> StatementResult<()> {
    if v != STATEMENT_VERSION {
        return Err(rim_identity::IdentityError::UnsupportedVersion(v).into());
    }
    if kind != expected {
        return Err(rim_identity::IdentityError::WrongType { expected }.into());
    }
    Ok(())
}

/// A server key log's inception, signed by its own server key.
pub fn sign_inception(
    server: &ServerSigner,
    recovery: &ServerSigner,
    created_at: DateTime<Utc>,
) -> SignedStatement {
    let payload = serde_json::to_string(&InceptionOut {
        v: STATEMENT_VERSION,
        kind: "server-inception",
        seq: 0,
        server_key: server.public_key().to_string(),
        recovery_key: recovery.public_key().to_string(),
        created_at: &timestamp(created_at),
    })
    .unwrap_or_default();
    server.sign_statement(ServerStatement::ServerKey.context(), payload)
}

/// The rotation that extends `state` with `new_server` (and `new_recovery`), signed by `signer`:
/// the current server key for [`ServerRotationSigner::Server`], the current recovery key for
/// [`ServerRotationSigner::Recovery`]. [`ServerKeyState::apply`] checks the rules.
pub fn sign_rotation(
    state: &ServerKeyState,
    signer: &ServerSigner,
    kind: ServerRotationSigner,
    new_server: &PublicKey,
    new_recovery: &PublicKey,
    created_at: DateTime<Utc>,
) -> SignedStatement {
    let payload = serde_json::to_string(&RotationOut {
        v: STATEMENT_VERSION,
        kind: "server-rotate",
        server_id: &state.server_id,
        seq: state.seq + 1,
        prev: state.next_prev(),
        server_key: new_server.to_string(),
        recovery_key: new_recovery.to_string(),
        signer: kind,
        created_at: &timestamp(created_at),
    })
    .unwrap_or_default();
    signer.sign_statement(ServerStatement::ServerKey.context(), payload)
}

/// A server key log verified from its inception to its head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerKeyState {
    pub server_id: String,
    /// Seq of the head entry; `keyid` and descriptors name it.
    pub seq: u32,
    pub server_key: PublicKey,
    pub recovery_key: PublicKey,
    /// The server key before the head entry, and who signed the head entry; `None` at seq 0.
    pub previous: Option<(PublicKey, ServerRotationSigner)>,
    head_hash: [u8; 32],
}

impl ServerKeyState {
    /// The `prev` value the next rotation carries: unpadded base64url SHA-256 of the head
    /// entry's payload.
    pub fn next_prev(&self) -> String {
        b64(&self.head_hash)
    }

    /// Verify an inception (seq 0, self-signed by its server key) and start a log.
    pub fn from_inception(entry: &SignedStatement) -> StatementResult<Self> {
        let raw: RawInception = parse_payload(ServerStatement::ServerKey, &entry.payload)?;
        check_header(raw.v, &raw.kind, "server-inception")?;
        if raw.seq != 0 {
            return Err(rim_identity::IdentityError::SequenceMismatch {
                expected: 0,
                got: raw.seq,
            }
            .into());
        }
        let server_key = PublicKey::parse(&raw.server_key, "serverKey")?;
        let recovery_key = PublicKey::parse(&raw.recovery_key, "recoveryKey")?;
        if server_key == recovery_key {
            return Err(StatementError::invalid(
                "serverKey and recoveryKey must differ",
            ));
        }
        verify(ServerStatement::ServerKey, &server_key, entry)?;
        Ok(Self {
            server_id: server_id_for_inception(&entry.payload),
            seq: 0,
            server_key,
            recovery_key,
            previous: None,
            head_hash: sha256(entry.payload.as_bytes()),
        })
    }

    /// Verify `entry` as the next rotation of this log and return the new state.
    pub fn apply(&self, entry: &SignedStatement) -> StatementResult<Self> {
        let raw: RawRotation = parse_payload(ServerStatement::ServerKey, &entry.payload)?;
        check_header(raw.v, &raw.kind, "server-rotate")?;
        if raw.server_id != self.server_id {
            return Err(StatementError::invalid(
                "rotation is for a different server",
            ));
        }
        let expected = self.seq + 1;
        if raw.seq != expected {
            return Err(rim_identity::IdentityError::SequenceMismatch {
                expected,
                got: raw.seq,
            }
            .into());
        }
        if raw.prev != self.next_prev() {
            return Err(rim_identity::IdentityError::ChainMismatch.into());
        }
        let server_key = PublicKey::parse(&raw.server_key, "serverKey")?;
        let recovery_key = PublicKey::parse(&raw.recovery_key, "recoveryKey")?;
        if server_key == self.server_key {
            return Err(StatementError::invalid(
                "a rotation must change the server key",
            ));
        }
        if server_key == recovery_key {
            return Err(StatementError::invalid(
                "serverKey and recoveryKey must differ",
            ));
        }
        match raw.signer {
            ServerRotationSigner::Server => {
                if recovery_key != self.recovery_key {
                    return Err(StatementError::invalid(
                        "only the recovery key can change the recovery key",
                    ));
                }
                verify(ServerStatement::ServerKey, &self.server_key, entry)?;
            }
            ServerRotationSigner::Recovery => {
                verify(ServerStatement::ServerKey, &self.recovery_key, entry)?;
            }
        }
        Ok(Self {
            server_id: self.server_id.clone(),
            seq: raw.seq,
            server_key,
            recovery_key,
            previous: Some((self.server_key.clone(), raw.signer)),
            head_hash: sha256(entry.payload.as_bytes()),
        })
    }

    /// Verify a whole log, oldest entry first.
    pub fn replay(entries: &[SignedStatement]) -> StatementResult<Self> {
        if entries.len() > MAX_LOG_ENTRIES {
            return Err(StatementError::invalid(format!(
                "a server key log may have at most {MAX_LOG_ENTRIES} entries"
            )));
        }
        let (first, rest) = entries
            .split_first()
            .ok_or_else(|| StatementError::invalid("server key log is empty"))?;
        let mut state = Self::from_inception(first)?;
        for entry in rest {
            state = state.apply(entry)?;
        }
        Ok(state)
    }

    /// Who signed the head entry: `None` for an inception.
    pub fn head_signer(&self) -> Option<ServerRotationSigner> {
        self.previous.as_ref().map(|(_, signer)| *signer)
    }
}

/// How a fetched server key log relates to a pinned one, compared payload by payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogRelation {
    /// Same entries.
    Equal,
    /// The pinned entries followed by more.
    Extends,
    /// A prefix of the pinned log: the peer serves an older view. The pin stays.
    Behind,
    /// An entry differs: the peer's history was rewritten.
    Forked,
}

/// Compare a fetched log with a pinned one. Only payloads are compared: the chain hashes
/// payloads, and each was verified when it was pinned.
pub fn compare_logs(pinned: &[SignedStatement], fetched: &[SignedStatement]) -> LogRelation {
    let common = pinned.len().min(fetched.len());
    if pinned[..common]
        .iter()
        .zip(&fetched[..common])
        .any(|(a, b)| a.payload != b.payload)
    {
        return LogRelation::Forked;
    }
    match fetched.len().cmp(&pinned.len()) {
        std::cmp::Ordering::Equal => LogRelation::Equal,
        std::cmp::Ordering::Greater => LogRelation::Extends,
        std::cmp::Ordering::Less => LogRelation::Behind,
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 9, 0, 0, 0)
            .single()
            .expect("time")
    }

    fn signer(seed: u8) -> ServerSigner {
        ServerSigner::from_seed([seed; 32])
    }

    fn log() -> (Vec<SignedStatement>, ServerKeyState) {
        let inception = sign_inception(&signer(1), &signer(2), at());
        let state = ServerKeyState::from_inception(&inception).expect("inception");
        (vec![inception], state)
    }

    #[test]
    fn inception_fixes_the_server_id() {
        let (log, state) = log();
        assert!(is_server_id(&state.server_id));
        assert_eq!(state.server_id, server_id_for_inception(&log[0].payload));
        assert_eq!(state.seq, 0);
        assert_eq!(state.head_signer(), None);
    }

    #[test]
    fn inception_must_be_self_signed() {
        let (log, _) = log();
        let forged =
            signer(2).sign_statement(ServerStatement::ServerKey.context(), &*log[0].payload);
        assert!(ServerKeyState::from_inception(&forged).is_err());
    }

    #[test]
    fn inception_refuses_version_1_unknown_fields_and_equal_keys() {
        let (log, _) = log();
        for payload in [
            log[0].payload.replace(r#""v":2"#, r#""v":1"#),
            log[0].payload.replacen('{', r#"{"x":1,"#, 1),
        ] {
            let entry = signer(1).sign_statement(ServerStatement::ServerKey.context(), payload);
            assert!(ServerKeyState::from_inception(&entry).is_err());
        }
        let same = sign_inception(&signer(1), &signer(1), at());
        assert!(ServerKeyState::from_inception(&same).is_err());
    }

    #[test]
    fn server_rotation_keeps_the_recovery_key_and_records_the_previous_key() {
        let (_, state) = log();
        let entry = sign_rotation(
            &state,
            &signer(1),
            ServerRotationSigner::Server,
            &signer(3).public_key(),
            &signer(2).public_key(),
            at(),
        );
        let next = state.apply(&entry).expect("rotate");
        assert_eq!(next.seq, 1);
        assert_eq!(next.server_key, signer(3).public_key());
        assert_eq!(
            next.previous,
            Some((signer(1).public_key(), ServerRotationSigner::Server))
        );

        let changes_recovery = sign_rotation(
            &state,
            &signer(1),
            ServerRotationSigner::Server,
            &signer(3).public_key(),
            &signer(4).public_key(),
            at(),
        );
        assert!(state.apply(&changes_recovery).is_err());
    }

    #[test]
    fn recovery_rotation_is_signed_by_the_recovery_key() {
        let (_, state) = log();
        let entry = sign_rotation(
            &state,
            &signer(2),
            ServerRotationSigner::Recovery,
            &signer(3).public_key(),
            &signer(4).public_key(),
            at(),
        );
        let next = state.apply(&entry).expect("recover");
        assert_eq!(next.recovery_key, signer(4).public_key());
        assert_eq!(next.head_signer(), Some(ServerRotationSigner::Recovery));

        let by_server_key = sign_rotation(
            &state,
            &signer(1),
            ServerRotationSigner::Recovery,
            &signer(3).public_key(),
            &signer(4).public_key(),
            at(),
        );
        assert!(state.apply(&by_server_key).is_err());
    }

    #[test]
    fn rotation_must_change_the_key_and_chain() {
        let (_, state) = log();
        let same_key = sign_rotation(
            &state,
            &signer(1),
            ServerRotationSigner::Server,
            &signer(1).public_key(),
            &signer(2).public_key(),
            at(),
        );
        assert!(state.apply(&same_key).is_err());

        let entry = sign_rotation(
            &state,
            &signer(1),
            ServerRotationSigner::Server,
            &signer(3).public_key(),
            &signer(2).public_key(),
            at(),
        );
        let next = state.apply(&entry).expect("rotate");
        // The same entry does not extend the new head.
        assert!(next.apply(&entry).is_err());
    }

    #[test]
    fn replay_refuses_empty_and_oversized_logs() {
        assert!(ServerKeyState::replay(&[]).is_err());
        let (log, _) = log();
        let long = vec![log[0].clone(); MAX_LOG_ENTRIES + 1];
        assert!(ServerKeyState::replay(&long).is_err());
    }

    #[test]
    fn compare_logs_tells_equal_extension_behind_and_fork() {
        let (log, state) = log();
        let rotate = |by: u8, to: u8| {
            sign_rotation(
                &state,
                &signer(by),
                ServerRotationSigner::Server,
                &signer(to).public_key(),
                &signer(2).public_key(),
                at(),
            )
        };
        let a = vec![log[0].clone(), rotate(1, 3)];
        let b = vec![log[0].clone(), rotate(1, 4)];
        assert_eq!(compare_logs(&a, &a), LogRelation::Equal);
        assert_eq!(compare_logs(&log, &a), LogRelation::Extends);
        assert_eq!(compare_logs(&a, &log), LogRelation::Behind);
        assert_eq!(compare_logs(&a, &b), LogRelation::Forked);
    }

    #[test]
    fn is_server_id_refuses_principal_ids_and_garbage() {
        assert!(!is_server_id(
            "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0"
        ));
        assert!(!is_server_id("rims1_short"));
        assert!(!is_server_id(""));
    }
}
