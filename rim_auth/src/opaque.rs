//! The account password check: OPAQUE (RFC 9807) with `opaque-ke`, and the transport sealing
//! of what the server returns over a finished login.
//!
//! The suite is [`RimOpaque`]: ristretto255 for the OPRF and the key exchange, triple
//! Diffie-Hellman with SHA-512, and Argon2id (m = 65536 KiB, t = 3, p = 4) as the key-stretching
//! function the client runs. The server never runs the key-stretching function; it keeps the
//! registration record, which lets it check a password without learning it. Every login uses
//! the context [`CONTEXT`]; the credential identifier is the principal id as UTF-8 bytes, and
//! both identities are the default (the two public keys).
//!
//! Messages travel as bytes; the helpers here take and return bytes so callers never handle
//! `opaque-ke` types, and map every failure to an error without detail.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{Context, anyhow};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use generic_array::{ArrayLength, GenericArray};
use hkdf::Hkdf;
use opaque_ke::errors::InternalError;
use opaque_ke::{
    CredentialFinalization, CredentialRequest, RegistrationRequest, RegistrationUpload,
    ServerLogin, ServerLoginParameters, ServerRegistration,
};
use rand_core::{CryptoRng, OsRng, RngCore};
use sha2::Sha256;
use zeroize::Zeroizing;

/// The context every login binds into its transcript.
pub const CONTEXT: &[u8] = b"rim-opaque-v1";

/// Length of a serialized [`ServerSetup`]: the OPRF seed, the server's key pair and the key the
/// server answers unknown principals with.
pub const SERVER_SETUP_LEN: usize = 128;

/// Length of the session key both sides hold after a login.
pub const SESSION_KEY_LEN: usize = 64;

/// Salt of the transport key derivation.
const TRANSPORT_SALT: &[u8] = b"rim.opaque.transport.v1";

/// Length of an AES-256-GCM nonce.
const GCM_NONCE_LEN: usize = 12;

/// Transport label of the server share returned after a login.
pub const LABEL_SERVER_SHARE: &str = "server share";

/// Argon2id parameters of the key-stretching function: 64 MiB, 3 passes, 4 lanes, and the
/// output length the caller asks for.
const KSF_PARAMS: argon2::Params = match argon2::Params::new(65536, 3, 4, None) {
    Ok(params) => params,
    Err(_) => panic!("the key-stretching parameters are invalid"),
};

/// The key-stretching function of [`RimOpaque`]: Argon2id with an all-zero salt, as RFC 9807
/// runs it. Its `Default` holds the suite's parameters.
pub struct RimKsf(argon2::Argon2<'static>);

impl Default for RimKsf {
    fn default() -> Self {
        Self(argon2::Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            KSF_PARAMS,
        ))
    }
}

#[cfg(any(test, feature = "testing"))]
impl RimKsf {
    /// Argon2id with the smallest parameters (8 KiB, 1 pass, 1 lane), for tests that play the
    /// client. The server never runs the key-stretching function, so it accepts a record made
    /// with any setting; a client that registered with this one must also log in with it.
    ///
    /// # Panics
    /// Never: the parameters are Argon2's minimums.
    pub fn cheap() -> Self {
        const CHEAP: argon2::Params = match argon2::Params::new(8, 1, 1, None) {
            Ok(params) => params,
            Err(_) => panic!("the cheap key-stretching parameters are invalid"),
        };
        Self(argon2::Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            CHEAP,
        ))
    }
}

impl opaque_ke::ksf::Ksf for RimKsf {
    fn hash<L: ArrayLength<u8>>(
        &self,
        input: GenericArray<u8, L>,
    ) -> Result<GenericArray<u8, L>, InternalError> {
        let mut out = GenericArray::default();
        self.0
            .hash_password_into(&input, &[0; argon2::RECOMMENDED_SALT_LEN], &mut out)
            .map_err(|_| InternalError::KsfError)?;
        Ok(out)
    }
}

/// The OPAQUE cipher suite of RIM accounts.
pub struct RimOpaque;

impl opaque_ke::CipherSuite for RimOpaque {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::TripleDh<opaque_ke::Ristretto255, sha2::Sha512>;
    type Ksf = RimKsf;
}

/// The server's long-term OPAQUE secrets: the OPRF seed and its key pair.
pub type ServerSetup = opaque_ke::ServerSetup<RimOpaque>;

/// The parameters of every server login: [`CONTEXT`] and the default identities.
fn login_parameters() -> ServerLoginParameters<'static, 'static> {
    ServerLoginParameters {
        context: Some(CONTEXT),
        identifiers: Default::default(),
    }
}

/// A new server setup from the OS random generator.
pub fn generate_server_setup() -> ServerSetup {
    ServerSetup::new(&mut OsRng)
}

/// The bytes of `setup`, [`SERVER_SETUP_LEN`] long.
pub fn server_setup_bytes(setup: &ServerSetup) -> Zeroizing<Vec<u8>> {
    Zeroizing::new(setup.serialize().to_vec())
}

/// The server setup `bytes` hold.
///
/// # Errors
/// When `bytes` is not a serialized server setup of this suite.
pub fn server_setup_from_bytes(bytes: &[u8]) -> anyhow::Result<ServerSetup> {
    ServerSetup::deserialize(bytes).map_err(|_| anyhow!("not an OPAQUE server setup"))
}

/// Answer a client's `RegistrationRequest` for the principal `credential_id`. Returns the
/// `RegistrationResponse`. The server keeps no state between this and the upload.
///
/// # Errors
/// When `request` is not a registration request of this suite.
pub fn server_registration_start(
    setup: &ServerSetup,
    request: &[u8],
    credential_id: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let request = RegistrationRequest::<RimOpaque>::deserialize(request)
        .map_err(|_| anyhow!("not an OPAQUE registration request"))?;
    let started = ServerRegistration::<RimOpaque>::start(setup, request, credential_id)
        .map_err(|_| anyhow!("cannot answer the OPAQUE registration request"))?;
    Ok(started.message.serialize().to_vec())
}

/// Turn a client's `RegistrationUpload` into the registration record the server stores.
///
/// # Errors
/// When `upload` is not a registration upload of this suite.
pub fn server_registration_finish(upload: &[u8]) -> anyhow::Result<Vec<u8>> {
    let upload = RegistrationUpload::<RimOpaque>::deserialize(upload)
        .map_err(|_| anyhow!("not an OPAQUE registration upload"))?;
    Ok(ServerRegistration::finish(upload).serialize().to_vec())
}

/// What the server keeps and sends after a login start.
pub struct ServerLoginStart {
    /// The `CredentialResponse` for the client.
    pub response: Vec<u8>,
    /// The server's login state, which [`server_login_finish`] needs. It holds the session key.
    pub state: Zeroizing<Vec<u8>>,
}

/// Answer a client's `CredentialRequest` for the principal `credential_id`, with its stored
/// `record`, or with `None` for a principal that has none: the answer then looks the same, and
/// no finalization will verify against it.
///
/// # Errors
/// When `request` or `record` does not parse.
pub fn server_login_start(
    setup: &ServerSetup,
    record: Option<&[u8]>,
    request: &[u8],
    credential_id: &[u8],
) -> anyhow::Result<ServerLoginStart> {
    server_login_start_with_rng(&mut OsRng, setup, record, request, credential_id)
}

/// [`server_login_start`] drawing its randomness from `rng`, for known-answer tests.
///
/// # Errors
/// When `request` or `record` does not parse.
pub fn server_login_start_with_rng<R: RngCore + CryptoRng>(
    rng: &mut R,
    setup: &ServerSetup,
    record: Option<&[u8]>,
    request: &[u8],
    credential_id: &[u8],
) -> anyhow::Result<ServerLoginStart> {
    let record = record
        .map(ServerRegistration::<RimOpaque>::deserialize)
        .transpose()
        .map_err(|_| anyhow!("the stored OPAQUE record does not parse"))?;
    let request = CredentialRequest::<RimOpaque>::deserialize(request)
        .map_err(|_| anyhow!("not an OPAQUE credential request"))?;
    let started = ServerLogin::start(
        rng,
        setup,
        record,
        request,
        credential_id,
        login_parameters(),
    )
    .map_err(|_| anyhow!("cannot answer the OPAQUE credential request"))?;
    Ok(ServerLoginStart {
        response: started.message.serialize().to_vec(),
        state: Zeroizing::new(started.state.serialize().to_vec()),
    })
}

/// Check a client's `CredentialFinalization` against the login `state`. Returns the session
/// key, [`SESSION_KEY_LEN`] bytes, which only a client that knew the password also holds.
///
/// # Errors
/// When the state or the finalization does not parse, or the finalization does not verify: a
/// wrong password, or a principal without a record.
pub fn server_login_finish(
    state: &[u8],
    finalization: &[u8],
) -> anyhow::Result<Zeroizing<[u8; SESSION_KEY_LEN]>> {
    let state = ServerLogin::<RimOpaque>::deserialize(state)
        .map_err(|_| anyhow!("the OPAQUE login state does not parse"))?;
    let finalization = CredentialFinalization::<RimOpaque>::deserialize(finalization)
        .map_err(|_| anyhow!("not an OPAQUE credential finalization"))?;
    let finished = state
        .finish(finalization, login_parameters())
        .map_err(|_| anyhow!("the OPAQUE login does not verify"))?;
    let mut key = Zeroizing::new([0u8; SESSION_KEY_LEN]);
    key.copy_from_slice(finished.session_key.as_slice());
    Ok(key)
}

/// The transport key for `label`: HKDF-SHA256 with the session key as input key material,
/// salt `"rim.opaque.transport.v1"` and the label as info, 32 bytes.
///
/// # Errors
/// Never in practice: 32 bytes is a valid HKDF-SHA256 output length.
pub fn transport_key(session_key: &[u8], label: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(TRANSPORT_SALT), session_key)
        .expand(label.as_bytes(), key.as_mut())
        .map_err(|_| anyhow!("cannot derive the transport key"))?;
    Ok(key)
}

/// `value` sealed for the client under the transport key for `label`: AES-256-GCM with a
/// random 12-byte nonce and no associated data, spelled as unpadded base64url of the nonce
/// followed by the ciphertext and tag.
///
/// # Errors
/// When the OS random generator or the cipher fails.
pub fn transport_seal(session_key: &[u8], label: &str, value: &[u8]) -> anyhow::Result<String> {
    let mut nonce = [0u8; GCM_NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| anyhow!("entropy failure: {e}"))?;
    transport_seal_with_nonce(session_key, label, value, nonce)
}

fn transport_seal_with_nonce(
    session_key: &[u8],
    label: &str,
    value: &[u8],
    nonce: [u8; GCM_NONCE_LEN],
) -> anyhow::Result<String> {
    let key = transport_key(session_key, label)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_slice()).context("transport key length")?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: value,
                aad: &[],
            },
        )
        .map_err(|_| anyhow!("cannot seal for the client"))?;
    let mut sealed = nonce.to_vec();
    sealed.extend_from_slice(&ciphertext);
    Ok(URL_SAFE_NO_PAD.encode(sealed))
}

/// The value [`transport_seal`] sealed under the transport key for `label`; `None` when it was
/// sealed under another key or label, or is damaged. Clients open with this; the server uses
/// it only in tests.
pub fn transport_open(session_key: &[u8], label: &str, sealed: &str) -> Option<Zeroizing<Vec<u8>>> {
    let sealed = URL_SAFE_NO_PAD.decode(sealed).ok()?;
    if sealed.len() <= GCM_NONCE_LEN {
        return None;
    }
    let (nonce, ciphertext) = sealed.split_at(GCM_NONCE_LEN);
    let key = transport_key(session_key, label).ok()?;
    let cipher = Aes256Gcm::new_from_slice(key.as_slice()).ok()?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: &[],
            },
        )
        .ok()
        .map(Zeroizing::new)
}

/// The client side of the protocol with [`RimKsf::cheap`], for tests that play the client.
#[cfg(any(test, feature = "testing"))]
pub mod client {
    use opaque_ke::{
        ClientLogin, ClientLoginFinishParameters, ClientRegistration,
        ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse,
    };
    use rand_core::OsRng;

    use super::{CONTEXT, RimKsf, RimOpaque};

    /// A password registration between its request and its upload.
    pub struct Registration {
        state: ClientRegistration<RimOpaque>,
        password: Vec<u8>,
    }

    impl Registration {
        /// Start registering `password`. Returns the flow and the `RegistrationRequest`.
        ///
        /// # Panics
        /// When `opaque-ke` refuses to blind the password, which it does not for any input.
        pub fn start(password: &[u8]) -> (Self, Vec<u8>) {
            let started = ClientRegistration::<RimOpaque>::start(&mut OsRng, password)
                .expect("blind the password");
            let request = started.message.serialize().to_vec();
            (
                Self {
                    state: started.state,
                    password: password.to_vec(),
                },
                request,
            )
        }

        /// Finish with the server's `RegistrationResponse`. Returns the `RegistrationUpload`
        /// and the export key; `None` when the response does not parse or is refused.
        pub fn finish(self, response: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
            let ksf = RimKsf::cheap();
            let finished = self
                .state
                .finish(
                    &mut OsRng,
                    &self.password,
                    RegistrationResponse::deserialize(response).ok()?,
                    ClientRegistrationFinishParameters::new(Default::default(), Some(&ksf)),
                )
                .ok()?;
            Some((
                finished.message.serialize().to_vec(),
                finished.export_key.to_vec(),
            ))
        }
    }

    /// What a finished client login holds.
    pub struct LoggedIn {
        /// The `CredentialFinalization` for the server.
        pub finalization: Vec<u8>,
        pub export_key: Vec<u8>,
        pub session_key: Vec<u8>,
    }

    /// A password login between its request and its finalization.
    pub struct Login {
        state: ClientLogin<RimOpaque>,
        password: Vec<u8>,
    }

    impl Login {
        /// Start logging in with `password`. Returns the flow and the `CredentialRequest`.
        ///
        /// # Panics
        /// When `opaque-ke` refuses to blind the password, which it does not for any input.
        pub fn start(password: &[u8]) -> (Self, Vec<u8>) {
            let started =
                ClientLogin::<RimOpaque>::start(&mut OsRng, password).expect("blind the password");
            let request = started.message.serialize().to_vec();
            (
                Self {
                    state: started.state,
                    password: password.to_vec(),
                },
                request,
            )
        }

        /// Finish with the server's `CredentialResponse`; `None` for a wrong password, a
        /// principal without a record, or a response that does not parse.
        pub fn finish(self, response: &[u8]) -> Option<LoggedIn> {
            let ksf = RimKsf::cheap();
            let finished = self
                .state
                .finish(
                    &mut OsRng,
                    &self.password,
                    CredentialResponse::deserialize(response).ok()?,
                    ClientLoginFinishParameters::new(Some(CONTEXT), Default::default(), Some(&ksf)),
                )
                .ok()?;
            Some(LoggedIn {
                finalization: finished.message.serialize().to_vec(),
                export_key: finished.export_key.to_vec(),
                session_key: finished.session_key.to_vec(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use opaque_ke::{
        ClientLogin, ClientLoginFinishParameters, ClientRegistration,
        ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse,
    };

    use super::*;

    const PRINCIPAL: &[u8] = b"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";

    /// The server's answer to a finalization: the session key, or why it refused.
    type ServerFinish = anyhow::Result<Zeroizing<[u8; SESSION_KEY_LEN]>>;

    /// Register `password` through the helpers with the cheap setting; returns the record.
    fn register(setup: &ServerSetup, password: &[u8]) -> Vec<u8> {
        let ksf = RimKsf::cheap();
        let start = ClientRegistration::<RimOpaque>::start(&mut OsRng, password).unwrap();
        let response =
            server_registration_start(setup, &start.message.serialize(), PRINCIPAL).unwrap();
        let finish = start
            .state
            .finish(
                &mut OsRng,
                password,
                RegistrationResponse::deserialize(&response).unwrap(),
                ClientRegistrationFinishParameters::new(Default::default(), Some(&ksf)),
            )
            .unwrap();
        server_registration_finish(&finish.message.serialize()).unwrap()
    }

    /// Log in with `password` against `record`; the client's session key and the server's
    /// answer to the finalization, or `None` when the client already refused.
    fn login(
        setup: &ServerSetup,
        record: Option<&[u8]>,
        password: &[u8],
    ) -> Option<(Vec<u8>, ServerFinish)> {
        let ksf = RimKsf::cheap();
        let start = ClientLogin::<RimOpaque>::start(&mut OsRng, password).unwrap();
        let server =
            server_login_start(setup, record, &start.message.serialize(), PRINCIPAL).unwrap();
        let finish = start
            .state
            .finish(
                &mut OsRng,
                password,
                CredentialResponse::deserialize(&server.response).unwrap(),
                ClientLoginFinishParameters::new(Some(CONTEXT), Default::default(), Some(&ksf)),
            )
            .ok()?;
        Some((
            finish.session_key.to_vec(),
            server_login_finish(&server.state, &finish.message.serialize()),
        ))
    }

    #[test]
    fn a_registered_password_logs_in_with_the_same_session_key_on_both_sides() {
        let setup = generate_server_setup();
        assert_eq!(server_setup_bytes(&setup).len(), SERVER_SETUP_LEN);
        let setup = server_setup_from_bytes(&server_setup_bytes(&setup)).unwrap();
        let record = register(&setup, b"correct horse");
        let (client_key, server_key) = login(&setup, Some(&record), b"correct horse").unwrap();
        assert_eq!(client_key, server_key.unwrap().to_vec());
    }

    #[test]
    fn a_wrong_password_or_a_missing_record_fails() {
        let setup = generate_server_setup();
        let record = register(&setup, b"correct horse");
        assert!(login(&setup, Some(&record), b"wrong horse").is_none());
        assert!(login(&setup, None, b"correct horse").is_none());
    }

    #[test]
    fn a_finalization_from_another_login_does_not_verify() {
        let setup = generate_server_setup();
        let record = register(&setup, b"pw");
        let ksf = RimKsf::cheap();
        let first = ClientLogin::<RimOpaque>::start(&mut OsRng, b"pw").unwrap();
        let first_server =
            server_login_start(&setup, Some(&record), &first.message.serialize(), PRINCIPAL)
                .unwrap();
        let second = ClientLogin::<RimOpaque>::start(&mut OsRng, b"pw").unwrap();
        let second_server = server_login_start(
            &setup,
            Some(&record),
            &second.message.serialize(),
            PRINCIPAL,
        )
        .unwrap();
        let finish = second
            .state
            .finish(
                &mut OsRng,
                b"pw",
                CredentialResponse::deserialize(&second_server.response).unwrap(),
                ClientLoginFinishParameters::new(Some(CONTEXT), Default::default(), Some(&ksf)),
            )
            .unwrap();
        assert!(server_login_finish(&first_server.state, &finish.message.serialize()).is_err());
        assert!(server_login_finish(&second_server.state, &finish.message.serialize()).is_ok());
    }

    #[test]
    fn garbage_messages_are_refused() {
        let setup = generate_server_setup();
        assert!(server_setup_from_bytes(&[1; 10]).is_err());
        assert!(server_registration_start(&setup, b"short", PRINCIPAL).is_err());
        assert!(server_registration_finish(b"short").is_err());
        assert!(server_login_start(&setup, None, b"short", PRINCIPAL).is_err());
        assert!(server_login_finish(b"short", b"short").is_err());
    }

    #[test]
    fn a_transport_sealed_value_opens_only_under_its_key_and_label() {
        let key = [5u8; SESSION_KEY_LEN];
        let sealed = transport_seal(&key, LABEL_SERVER_SHARE, b"share").unwrap();
        assert_eq!(
            transport_open(&key, LABEL_SERVER_SHARE, &sealed)
                .as_deref()
                .map(Vec::as_slice),
            Some(&b"share"[..])
        );
        assert!(transport_open(&key, "keep key", &sealed).is_none());
        assert!(transport_open(&[6u8; SESSION_KEY_LEN], LABEL_SERVER_SHARE, &sealed).is_none());
        assert!(transport_open(&key, LABEL_SERVER_SHARE, "AAAA").is_none());
        assert_ne!(
            transport_seal(&key, LABEL_SERVER_SHARE, b"share").unwrap(),
            sealed,
            "a fresh nonce each time"
        );
    }

    #[test]
    fn the_transport_seal_has_a_fixed_shape() {
        let sealed =
            transport_seal_with_nonce(&[1u8; SESSION_KEY_LEN], "server share", &[2u8; 32], [3; 12])
                .unwrap();
        let bytes = URL_SAFE_NO_PAD.decode(&sealed).unwrap();
        assert_eq!(bytes.len(), 12 + 32 + 16);
        assert_eq!(&bytes[..12], &[3u8; 12]);
    }
}
