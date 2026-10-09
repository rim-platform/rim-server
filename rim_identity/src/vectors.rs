//! Known-answer vectors, also printed in `docs/specs/key-identity-v1.md` (appendix A) so client
//! implementations can check their signing and hashing against the server's.

use uuid::Uuid;

use crate::device::verify_device_cert;
use crate::key_log::KeyState;
use crate::login::UnverifiedLogin;
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
    assert_eq!(key_text(&id.identity), IDENTITY_KEY);
    assert_eq!(key_text(&id.recovery), RECOVERY_KEY);

    let inception = id.inception();
    assert_eq!(inception.signature, INCEPTION_SIGNATURE);
    let state = KeyState::from_inception(&inception).expect("inception");
    assert_eq!(state.principal_id, PRINCIPAL_ID);
    assert_eq!(state.next_prev(), NEXT_PREV);

    let device = signing_key(20);
    assert_eq!(key_text(&device), DEVICE_KEY);
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
