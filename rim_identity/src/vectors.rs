//! Known-answer vectors, also printed in `docs/specs/key-identity-v1.md` (appendix A) so client
//! implementations can check their signing and hashing against the server's.

use uuid::Uuid;

use crate::device::verify_device_cert;
use crate::key_log::KeyState;
use crate::login::UnverifiedLogin;
use crate::statement::StatementVersion::{V1, V2};
use crate::testing::{TestIdentity, device_login, key_text, signing_key};

const IDENTITY_KEY: &str = "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w";
const RECOVERY_KEY: &str = "1i8Bah79Hk_feT60LNhEceG6nwzwTRKHtcxx9hYofLg";
const INCEPTION_SIGNATURE: &str =
    "vf3zjxbHUJ8l9Oe6ayFGZZszVH1c_FVgO04F_4PqkfsYu9h8jAgbPKhAIAYHlQxg-7yAO5w1OEMmcAGcXptVDQ";
const PRINCIPAL_ID: &str = "rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo";
const NEXT_PREV: &str = "o1oFYkMXfjtQJnG6as5GURR0cnzeYNYv5Br-LaeI78I";
const DEVICE_KEY: &str = "IIKL9cW9ystoSGMzbCAvtVmdpIvlWWYVdCFwcFvsqfc";
const DEVICE_CERT_SIGNATURE: &str =
    "tt6UNl3tRCwupWIYEfnWzqW9qVqDsJju2dA7d2CdNqdIZ0piVeP_Q6Y1lLfA4YN-26tsdaFea-rM_I8Qy9H6Cg";
const LOGIN_PAYLOAD: &str = r#"{"v":1,"type":"login","challengeId":"00000000-0000-0000-0000-000000000009","nonce":"bm9uY2U","audience":"localhost:8080","principalId":"rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo","deviceId":"00000000-0000-0000-0000-000000000001"}"#;
const LOGIN_SIGNATURE: &str =
    "jB_BFFAn7e-FMh8wCSWhnCcPzMjoe1fTKwlCoGfE0p1OYkhZWjV4J1c_AgeIhKpWOnLYyB0PqstAEYmaCITXBw";

#[test]
fn known_answer_vectors() {
    let id = TestIdentity::new(1);
    assert_eq!(key_text(V1, &id.identity), IDENTITY_KEY);
    assert_eq!(key_text(V1, &id.recovery), RECOVERY_KEY);

    let inception = id.inception();
    assert_eq!(inception.signature, INCEPTION_SIGNATURE);
    let state = KeyState::from_inception(&inception).expect("inception");
    assert_eq!(state.principal_id, PRINCIPAL_ID);
    assert_eq!(state.next_prev(), NEXT_PREV);

    let device = signing_key(20);
    assert_eq!(key_text(V1, &device), DEVICE_KEY);
    let cert = id.device_cert(&state, Uuid::from_u128(1), &device, "Laptop");
    assert_eq!(cert.signature, DEVICE_CERT_SIGNATURE);
    let cert = verify_device_cert(&state, &cert).expect("cert");

    let login = device_login(
        &device,
        &state,
        Uuid::from_u128(1),
        Uuid::from_u128(9),
        "bm9uY2U",
        "localhost:8080",
    );
    assert_eq!(login.payload, LOGIN_PAYLOAD);
    assert_eq!(login.signature, LOGIN_SIGNATURE);
    UnverifiedLogin::read(&login)
        .expect("read")
        .verify(&cert.device_key)
        .expect("login");
}

const V2_INCEPTION_PAYLOAD: &str = r#"{"v":2,"type":"inception","seq":0,"identityKey":"ed25519:iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w","recoveryKey":"ed25519:1i8Bah79Hk_feT60LNhEceG6nwzwTRKHtcxx9hYofLg","createdAt":"2026-10-09T00:00:00Z"}"#;
const V2_INCEPTION_SIGNATURE: &str = "ed25519:kGvSf1hR28cdKP2Smwmy2BHEdP7wmKwy0At-DtrUs8mYhfzF0jjHTjpeX4DAzXv5HuCVwFEYWM1DjN-6wkU2Ag";
const V2_PRINCIPAL_ID: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
const V2_DEVICE_CERT_SIGNATURE: &str = "ed25519:0HjRzn6gQ2q-vCHSkdOPK2ykGGBUpW2fYdDuEdTzf6OxsWzjXeNNpRSVgzPxOEfbYzZ9o9DWqp4NxcTEDhOaDA";
const V2_LOGIN_SIGNATURE: &str = "ed25519:3FSH7M9uz0sP42GA0gKPso6JmGSx32Y1WZ-oKBW0VcVWANGEEjkDjaEtDpHNIjdEgC5fFA2jbfzSmuzhW7k1Dg";
const V2_ROTATION_PAYLOAD: &str = r#"{"v":2,"type":"rotate","principalId":"rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo","seq":1,"prev":"o1oFYkMXfjtQJnG6as5GURR0cnzeYNYv5Br-LaeI78I","identityKey":"ed25519:gTl3Dqh9F19Wo1Rmw0x-zMuNipG07jeiXfYPW4_Js5Q","recoveryKey":"ed25519:1i8Bah79Hk_feT60LNhEceG6nwzwTRKHtcxx9hYofLg","signer":"identity","createdAt":"2026-10-09T00:00:00Z"}"#;
const V2_ROTATION_SIGNATURE: &str = "ed25519:D2moo9U9OjVL9zed4lM0A5cHoGQOVEtUfgKfAmOn0P18x74NEbh4FL4f3PbulQzoVuGAGOEJUZq3R2ZNpeUdCg";

/// The same keys and fields as [`known_answer_vectors`], written as version 2 statements.
#[test]
fn known_answer_vectors_v2() {
    let id = TestIdentity::with_version(1, V2);
    assert_eq!(
        key_text(V2, &id.identity),
        format!("ed25519:{IDENTITY_KEY}")
    );

    let inception = id.inception();
    assert_eq!(inception.payload, V2_INCEPTION_PAYLOAD);
    assert_eq!(inception.signature, V2_INCEPTION_SIGNATURE);
    let state = KeyState::from_inception(&inception).expect("inception");
    assert_eq!(state.principal_id, V2_PRINCIPAL_ID);

    let device = signing_key(20);
    let cert = id.device_cert(&state, Uuid::from_u128(1), &device, "Laptop");
    assert_eq!(cert.signature, V2_DEVICE_CERT_SIGNATURE);
    let cert = verify_device_cert(&state, &cert).expect("cert");
    assert_eq!(cert.device_key.to_string(), format!("ed25519:{DEVICE_KEY}"));

    let login = device_login(
        &device,
        &state,
        Uuid::from_u128(1),
        Uuid::from_u128(9),
        "bm9uY2U",
        "localhost:8080",
    );
    assert_eq!(login.signature, V2_LOGIN_SIGNATURE);
    UnverifiedLogin::read(&login)
        .expect("read")
        .verify(&cert.device_key)
        .expect("login");
}

/// A version 2 rotation (to the key from seed 2) extending the version 1 log above.
#[test]
fn known_answer_mixed_log() {
    let mut id = TestIdentity::new(1);
    let state = KeyState::from_inception(&id.inception()).expect("inception");
    id.version = V2;
    let rotation = id.rotate_with_identity(&state, 2);
    assert_eq!(rotation.payload, V2_ROTATION_PAYLOAD);
    assert_eq!(rotation.signature, V2_ROTATION_SIGNATURE);
    let (next, _) = state.apply(&rotation).expect("rotate");
    assert_eq!(next.principal_id, PRINCIPAL_ID);
}
