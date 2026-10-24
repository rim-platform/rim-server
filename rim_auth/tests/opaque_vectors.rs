//! Replays the OPAQUE known-answer vectors in `tests/vectors/opaque-v1.json`, which the client
//! implementation keeps a byte-identical copy of.
//!
//! One ChaCha20 generator (`rand_chacha` 0.3) seeded with 32 bytes of 0x07 supplies every random
//! draw, in this order: the server setup, the registration start, the registration finish, the
//! login start, the server's login start and the login finish. The client runs the suite's own
//! key-stretching parameters. `write_opaque_vectors`, ignored by default, writes the file again
//! from the current code.

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
    CONTEXT, RimOpaque, ServerSetup, server_login_finish, server_login_start_with_rng,
    server_registration_finish, server_registration_start, server_setup_bytes,
};

#[derive(Serialize, Deserialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Vectors {
    suite: String,
    seed: String,
    password: String,
    credential_id: String,
    server_setup: String,
    registration_request: String,
    registration_response: String,
    registration_upload: String,
    registration_export_key: String,
    password_file: String,
    credential_request: String,
    credential_response: String,
    credential_finalization: String,
    login_export_key: String,
    client_session_key: String,
    server_session_key: String,
}

const SEED: [u8; 32] = [7; 32];
const PASSWORD: &str = "correct horse battery staple";
const CREDENTIAL_ID: &str = "rimp1_vectortest";

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/opaque-v1.json")
}

/// One registration and one login with the seeded generator.
fn compute() -> Vectors {
    let mut rng = ChaCha20Rng::from_seed(SEED);
    let password = PASSWORD.as_bytes();
    let credential_id = CREDENTIAL_ID.as_bytes();

    let setup = ServerSetup::new(&mut rng);
    let registration =
        ClientRegistration::<RimOpaque>::start(&mut rng, password).expect("registration start");
    let registration_request = registration.message.serialize().to_vec();
    let registration_response =
        server_registration_start(&setup, &registration_request, credential_id)
            .expect("server registration start");
    let registered = registration
        .state
        .finish(
            &mut rng,
            password,
            RegistrationResponse::deserialize(&registration_response).expect("response"),
            ClientRegistrationFinishParameters::default(),
        )
        .expect("registration finish");
    let registration_upload = registered.message.serialize().to_vec();
    let password_file = server_registration_finish(&registration_upload).expect("password file");

    let login = ClientLogin::<RimOpaque>::start(&mut rng, password).expect("login start");
    let credential_request = login.message.serialize().to_vec();
    let server = server_login_start_with_rng(
        &mut rng,
        &setup,
        Some(&password_file),
        &credential_request,
        credential_id,
    )
    .expect("server login start");
    let logged_in = login
        .state
        .finish(
            &mut rng,
            password,
            CredentialResponse::deserialize(&server.response).expect("credential response"),
            ClientLoginFinishParameters::new(Some(CONTEXT), Default::default(), None),
        )
        .expect("login finish");
    let credential_finalization = logged_in.message.serialize().to_vec();
    let server_session_key =
        server_login_finish(&server.state, &credential_finalization).expect("server login finish");

    Vectors {
        suite: "rim-opaque-v1".to_string(),
        seed: SEED.iter().map(|b| format!("{b:02x}")).collect(),
        password: PASSWORD.to_string(),
        credential_id: CREDENTIAL_ID.to_string(),
        server_setup: b64(&server_setup_bytes(&setup)),
        registration_request: b64(&registration_request),
        registration_response: b64(&registration_response),
        registration_upload: b64(&registration_upload),
        registration_export_key: b64(&registered.export_key),
        password_file: b64(&password_file),
        credential_request: b64(&credential_request),
        credential_response: b64(&server.response),
        credential_finalization: b64(&credential_finalization),
        login_export_key: b64(&logged_in.export_key),
        client_session_key: b64(&logged_in.session_key),
        server_session_key: b64(server_session_key.as_slice()),
    }
}

/// The file's text for `vectors`: pretty JSON with two-space indents and one final newline.
fn text(vectors: &Vectors) -> String {
    serde_json::to_string_pretty(vectors).expect("json") + "\n"
}

#[test]
#[ignore = "writes tests/vectors/opaque-v1.json"]
fn write_opaque_vectors() {
    std::fs::write(path(), text(&compute())).expect("write the vector file");
}

#[test]
fn opaque_vectors_replay() {
    let stored = std::fs::read_to_string(path()).expect("read the vector file");
    let parsed: Vectors = serde_json::from_str(&stored).expect("vector file");
    let computed = compute();
    assert_eq!(computed, parsed);
    assert_eq!(
        text(&computed),
        stored,
        "the file is byte for byte what the code writes"
    );
    assert_eq!(computed.registration_export_key, computed.login_export_key);
    assert_eq!(computed.client_session_key, computed.server_session_key);
    assert_eq!(computed.registration_upload, computed.password_file);
}
