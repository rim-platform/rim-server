//! Replays the OPAQUE known-answer vectors in `tests/vectors/opaque-v1.json`.
//!
//! Every random draw comes from a ChaCha20 generator (`rand_chacha` 0.3) seeded with 32 copies
//! of one byte, a fresh generator per step, so a client implementation that runs the same
//! steps with the same seeds produces the same bytes. Run with `RIM_WRITE_OPAQUE_VECTORS=1` to
//! write the file again from the current code.

use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse,
};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use serde::{Deserialize, Serialize};

use rim_auth::opaque::{
    CONTEXT, LABEL_SERVER_SHARE, RimOpaque, ServerSetup, server_login_finish,
    server_login_start_with_rng, server_registration_finish, server_registration_start,
    server_setup_bytes, server_setup_from_bytes, transport_key, transport_open, transport_seal,
};

#[derive(Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Seeds {
    server_setup: u8,
    registration_start: u8,
    registration_finish: u8,
    login_start: u8,
    server_login_start: u8,
    login_finish: u8,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Transport {
    label: String,
    key: String,
    plaintext: String,
    sealed: String,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Vectors {
    description: String,
    suite: String,
    context: String,
    password: String,
    credential_id: String,
    seeds: Seeds,
    server_setup: String,
    registration_request: String,
    registration_response: String,
    registration_upload: String,
    credential_request: String,
    credential_response: String,
    credential_finalization: String,
    export_key: String,
    session_key: String,
    transport: Transport,
}

const SEEDS: Seeds = Seeds {
    server_setup: 1,
    registration_start: 2,
    registration_finish: 3,
    login_start: 4,
    server_login_start: 5,
    login_finish: 6,
};

const PASSWORD: &str = "correct horse battery staple";
const CREDENTIAL_ID: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";

fn rng(seed: u8) -> ChaCha20Rng {
    ChaCha20Rng::from_seed([seed; 32])
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn unb64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).expect("base64url")
}

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/opaque-v1.json")
}

/// Run registration and login with the seeded generators and the suite's own key-stretching
/// parameters. `sealed` is the transport sample to keep; a new one is made when `None`.
fn compute(sealed: Option<String>) -> Vectors {
    let password = PASSWORD.as_bytes();
    let credential_id = CREDENTIAL_ID.as_bytes();
    let setup = ServerSetup::new(&mut rng(SEEDS.server_setup));

    let registration =
        ClientRegistration::<RimOpaque>::start(&mut rng(SEEDS.registration_start), password)
            .expect("registration start");
    let registration_request = registration.message.serialize().to_vec();
    let registration_response =
        server_registration_start(&setup, &registration_request, credential_id)
            .expect("server registration start");
    let registered = registration
        .state
        .finish(
            &mut rng(SEEDS.registration_finish),
            password,
            RegistrationResponse::deserialize(&registration_response).expect("response"),
            ClientRegistrationFinishParameters::default(),
        )
        .expect("registration finish");
    let registration_upload = registered.message.serialize().to_vec();
    let record = server_registration_finish(&registration_upload).expect("record");

    let login = ClientLogin::<RimOpaque>::start(&mut rng(SEEDS.login_start), password)
        .expect("login start");
    let credential_request = login.message.serialize().to_vec();
    let server = server_login_start_with_rng(
        &mut rng(SEEDS.server_login_start),
        &setup,
        Some(&record),
        &credential_request,
        credential_id,
    )
    .expect("server login start");
    let logged_in = login
        .state
        .finish(
            &mut rng(SEEDS.login_finish),
            password,
            CredentialResponse::deserialize(&server.response).expect("credential response"),
            ClientLoginFinishParameters::new(Some(CONTEXT), Default::default(), None),
        )
        .expect("login finish");
    assert_eq!(
        logged_in.export_key, registered.export_key,
        "registration and login yield one export key"
    );
    let credential_finalization = logged_in.message.serialize().to_vec();
    let server_key =
        server_login_finish(&server.state, &credential_finalization).expect("server login finish");
    assert_eq!(server_key.as_slice(), logged_in.session_key.as_slice());

    let session_key = logged_in.session_key.to_vec();
    let plaintext = [0x5au8; 32];
    let sealed = sealed.unwrap_or_else(|| {
        transport_seal(&session_key, LABEL_SERVER_SHARE, &plaintext).expect("seal")
    });
    Vectors {
        description: "RIM OPAQUE v1: one registration and one login. Every random draw comes \
                      from ChaCha20Rng (rand_chacha 0.3) seeded with 32 copies of the step's \
                      seed byte, a fresh generator per step. Byte strings are unpadded base64url."
            .to_string(),
        suite: "OPRF ristretto255-SHA512, KE TripleDH ristretto255 SHA-512, KSF Argon2id \
                m=65536 t=3 p=4 with a zero salt"
            .to_string(),
        context: String::from_utf8(CONTEXT.to_vec()).expect("utf8"),
        password: PASSWORD.to_string(),
        credential_id: CREDENTIAL_ID.to_string(),
        seeds: SEEDS,
        server_setup: b64(&server_setup_bytes(&setup)),
        registration_request: b64(&registration_request),
        registration_response: b64(&registration_response),
        registration_upload: b64(&registration_upload),
        credential_request: b64(&credential_request),
        credential_response: b64(&server.response),
        credential_finalization: b64(&credential_finalization),
        export_key: b64(&registered.export_key),
        session_key: b64(&session_key),
        transport: Transport {
            label: LABEL_SERVER_SHARE.to_string(),
            key: b64(transport_key(&session_key, LABEL_SERVER_SHARE)
                .expect("transport key")
                .as_slice()),
            plaintext: b64(&plaintext),
            sealed,
        },
    }
}

#[test]
fn opaque_vectors_replay() {
    if std::env::var_os("RIM_WRITE_OPAQUE_VECTORS").is_some() {
        let vectors = compute(None);
        let text = serde_json::to_string_pretty(&vectors).expect("json") + "\n";
        std::fs::write(path(), text).expect("write the vector file");
        return;
    }
    let text = std::fs::read_to_string(path()).expect("read the vector file");
    let stored: Vectors = serde_json::from_str(&text).expect("vector file");
    let computed = compute(Some(stored.transport.sealed.clone()));
    assert_eq!(computed, stored);

    // The server-side helpers alone reproduce their parts from the stored bytes.
    let setup = server_setup_from_bytes(&unb64(&stored.server_setup)).expect("setup");
    assert_eq!(
        server_registration_start(
            &setup,
            &unb64(&stored.registration_request),
            stored.credential_id.as_bytes()
        )
        .expect("registration start"),
        unb64(&stored.registration_response)
    );
    let opened = transport_open(
        &unb64(&stored.session_key),
        &stored.transport.label,
        &stored.transport.sealed,
    )
    .expect("the stored sample opens");
    assert_eq!(
        opened.as_slice(),
        unb64(&stored.transport.plaintext).as_slice()
    );
}
