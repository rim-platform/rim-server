use rim_identity::SignedStatement;
use serde::{Deserialize, Serialize};

use crate::error::{StatementError, StatementResult};
use crate::key_log::ServerKeyState;
use crate::signer::ServerSigner;
use crate::statement::{STATEMENT_VERSION, ServerStatement, parse_payload, verify};

/// Largest descriptor `version`.
pub const MAX_DESCRIPTOR_VERSION: u32 = i32::MAX as u32;

/// Longest descriptor `abuseContact`, in bytes.
pub const MAX_ABUSE_CONTACT_BYTES: usize = 512;

/// A role a server runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Host,
    Relay,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Relay => "relay",
        }
    }
}

/// Where a server answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Endpoints {
    pub graphql: String,
    pub websocket: String,
    /// `None` while server-to-server traffic is off.
    pub s2s: Option<String>,
}

/// Limits a server announces to its peers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Limits {
    pub max_body_bytes: u64,
    pub clock_skew_seconds: u32,
    pub max_deliver_items: u32,
}

/// How a server accepts new local accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistrationMode {
    Invite,
    Open,
    Pow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Registration {
    pub mode: RegistrationMode,
    /// Proof-of-work difficulty; only in `pow` mode.
    pub pow_bits: Option<u32>,
}

/// The signed description of a server: roles, endpoints, versions and limits.
///
/// Field order is the payload's: [`sign_descriptor`] serializes it as is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ServerDescriptor {
    pub v: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub server_id: String,
    pub key_seq: u32,
    pub version: u32,
    pub roles: Vec<Role>,
    pub endpoints: Endpoints,
    pub s2s_versions: Vec<u32>,
    pub mls_cipher_suites: Vec<i32>,
    pub limits: Limits,
    pub registration: Registration,
    pub issued_at: String,
    /// Where to report abuse on this server: an email address or an https URL. Left out of the
    /// payload when the operator names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abuse_contact: Option<String>,
}

const DESCRIPTOR_TYPE: &str = "server-descriptor";

impl ServerDescriptor {
    /// A descriptor with the version 2 header filled in.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        server_id: String,
        key_seq: u32,
        version: u32,
        roles: Vec<Role>,
        endpoints: Endpoints,
        mls_cipher_suites: Vec<i32>,
        limits: Limits,
        registration: Registration,
        issued_at: String,
    ) -> Self {
        Self {
            v: STATEMENT_VERSION,
            kind: DESCRIPTOR_TYPE.to_string(),
            server_id,
            key_seq,
            version,
            roles,
            endpoints,
            s2s_versions: vec![1],
            mls_cipher_suites,
            limits,
            registration,
            issued_at,
            abuse_contact: None,
        }
    }

    /// This descriptor naming `abuse_contact` as where to report abuse.
    pub fn with_abuse_contact(mut self, abuse_contact: Option<String>) -> Self {
        self.abuse_contact = abuse_contact;
        self
    }

    /// Whether two descriptors say the same thing, ignoring `version` and `issuedAt`.
    pub fn same_content(&self, other: &Self) -> bool {
        let strip = |d: &Self| Self {
            version: 0,
            issued_at: String::new(),
            ..d.clone()
        };
        strip(self) == strip(other)
    }

    fn check(&self) -> StatementResult<()> {
        if self.v != STATEMENT_VERSION {
            return Err(rim_identity::IdentityError::UnsupportedVersion(self.v).into());
        }
        if self.kind != DESCRIPTOR_TYPE {
            return Err(rim_identity::IdentityError::WrongType {
                expected: DESCRIPTOR_TYPE,
            }
            .into());
        }
        if !(1..=MAX_DESCRIPTOR_VERSION).contains(&self.version) {
            return Err(StatementError::invalid(
                "descriptor version must be 1 to 2^31-1",
            ));
        }
        if self.roles.is_empty() || !self.roles.windows(2).all(|w| w[0] < w[1]) {
            return Err(StatementError::invalid(
                "roles must be a non-empty subset of host, relay, in that order",
            ));
        }
        let pow_ok = match self.registration.mode {
            RegistrationMode::Pow => self.registration.pow_bits.is_some(),
            _ => self.registration.pow_bits.is_none(),
        };
        if !pow_ok {
            return Err(StatementError::invalid(
                "powBits is set exactly in pow registration mode",
            ));
        }
        if let Some(contact) = &self.abuse_contact
            && (contact.trim().is_empty() || contact.len() > MAX_ABUSE_CONTACT_BYTES)
        {
            return Err(StatementError::invalid(format!(
                "abuseContact must be 1 to {MAX_ABUSE_CONTACT_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

/// Sign `descriptor` with the head server key. Refuses a descriptor that breaks the rules
/// [`verify_descriptor`] checks.
pub fn sign_descriptor(
    signer: &ServerSigner,
    descriptor: &ServerDescriptor,
) -> StatementResult<SignedStatement> {
    descriptor.check()?;
    let payload = serde_json::to_string(descriptor)
        .map_err(|e| StatementError::invalid(format!("descriptor does not serialize: {e}")))?;
    Ok(signer.sign_statement(ServerStatement::ServerDescriptor.context(), payload))
}

/// Verify a descriptor against the server's key log: it names the log's server id and head
/// seq, follows the descriptor rules, and is signed by the head server key.
pub fn verify_descriptor(
    state: &ServerKeyState,
    statement: &SignedStatement,
) -> StatementResult<ServerDescriptor> {
    let descriptor: ServerDescriptor =
        parse_payload(ServerStatement::ServerDescriptor, &statement.payload)?;
    descriptor.check()?;
    if descriptor.server_id != state.server_id {
        return Err(StatementError::invalid(
            "descriptor is for a different server",
        ));
    }
    if descriptor.key_seq != state.seq {
        return Err(StatementError::invalid(
            "descriptor is not signed by the head server key",
        ));
    }
    verify(
        ServerStatement::ServerDescriptor,
        &state.server_key,
        statement,
    )?;
    Ok(descriptor)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::key_log::{ServerRotationSigner, sign_inception, sign_rotation};

    fn state() -> (ServerKeyState, ServerSigner) {
        let server = ServerSigner::from_seed([1; 32]);
        let at = chrono::Utc
            .with_ymd_and_hms(2026, 10, 9, 0, 0, 0)
            .single()
            .expect("time");
        let inception = sign_inception(&server, &ServerSigner::from_seed([2; 32]), at);
        (
            ServerKeyState::from_inception(&inception).expect("inception"),
            server,
        )
    }

    fn descriptor(state: &ServerKeyState) -> ServerDescriptor {
        ServerDescriptor::new(
            state.server_id.clone(),
            state.seq,
            1,
            vec![Role::Host, Role::Relay],
            Endpoints {
                graphql: "https://a.example/api".into(),
                websocket: "wss://a.example/api/ws".into(),
                s2s: None,
            },
            vec![79],
            Limits {
                max_body_bytes: 8 * 1024 * 1024,
                clock_skew_seconds: 300,
                max_deliver_items: 256,
            },
            Registration {
                mode: RegistrationMode::Invite,
                pow_bits: None,
            },
            "2026-10-09T00:00:00Z".into(),
        )
    }

    #[test]
    fn descriptor_round_trips_with_s2s_null() {
        let (state, signer) = state();
        let d = descriptor(&state);
        let signed = sign_descriptor(&signer, &d).expect("sign");
        assert!(signed.payload.contains(r#""s2s":null"#));
        assert_eq!(verify_descriptor(&state, &signed), Ok(d));
    }

    #[test]
    fn descriptor_refuses_bad_roles_versions_and_pow() {
        let (state, signer) = state();
        let mut bad = vec![];
        for change in 0..5 {
            let mut d = descriptor(&state);
            match change {
                0 => d.roles = vec![],
                1 => d.roles = vec![Role::Relay, Role::Host],
                2 => d.version = 0,
                3 => d.registration.pow_bits = Some(16),
                _ => d.roles = vec![Role::Host, Role::Host],
            }
            bad.push(d);
        }
        for d in bad {
            assert!(sign_descriptor(&signer, &d).is_err(), "{d:?}");
        }
    }

    #[test]
    fn descriptor_must_name_the_head_key_and_server() {
        let (state, signer) = state();
        let next_key = ServerSigner::from_seed([3; 32]);
        let rotation = sign_rotation(
            &state,
            &signer,
            ServerRotationSigner::Server,
            &next_key.public_key(),
            &state.recovery_key,
            chrono::Utc::now(),
        );
        let rotated = state.apply(&rotation).expect("rotate");
        // Signed at seq 0 by the old key: refused against the rotated log.
        let old = sign_descriptor(&signer, &descriptor(&state)).expect("sign");
        assert!(verify_descriptor(&rotated, &old).is_err());
        // Claims seq 1 but signed by the old key.
        let mut d = descriptor(&rotated);
        d.key_seq = 1;
        let wrong_key = sign_descriptor(&signer, &d).expect("sign");
        assert!(verify_descriptor(&rotated, &wrong_key).is_err());
        let right = sign_descriptor(&next_key, &d).expect("sign");
        assert!(verify_descriptor(&rotated, &right).is_ok());
    }

    #[test]
    fn abuse_contact_is_signed_when_set_and_left_out_when_not() {
        let (state, signer) = state();
        let plain = sign_descriptor(&signer, &descriptor(&state)).expect("sign");
        assert!(!plain.payload.contains("abuseContact"));
        let d = descriptor(&state).with_abuse_contact(Some("abuse@a.example".into()));
        let signed = sign_descriptor(&signer, &d).expect("sign");
        assert!(
            signed.payload.ends_with(
                r#""issuedAt":"2026-10-09T00:00:00Z","abuseContact":"abuse@a.example"}"#
            )
        );
        assert_eq!(verify_descriptor(&state, &signed), Ok(d.clone()));
        assert!(!d.same_content(&descriptor(&state)));
        for bad in [
            String::new(),
            " ".into(),
            "a".repeat(MAX_ABUSE_CONTACT_BYTES + 1),
        ] {
            let d = descriptor(&state).with_abuse_contact(Some(bad));
            assert!(sign_descriptor(&signer, &d).is_err());
        }
    }

    #[test]
    fn same_content_ignores_version_and_issue_time() {
        let (state, _) = state();
        let a = descriptor(&state);
        let mut b = a.clone();
        b.version = 7;
        b.issued_at = "2027-01-01T00:00:00Z".into();
        assert!(a.same_content(&b));
        b.endpoints.s2s = Some("https://a.example/s2s/v1".into());
        assert!(!a.same_content(&b));
    }
}
