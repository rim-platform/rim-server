//! A known-answer sample of the transport seal: the key for "server share" derived from the
//! session key of `tests/vectors/opaque-v1.json`, and a value sealed under it that a client
//! must open.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use rim_auth::opaque::{LABEL_SERVER_SHARE, transport_key, transport_open};

/// `clientSessionKey` of the vector file.
const SESSION_KEY: &str =
    "TvFUNjOQ4cODeoSs-3z9oKX4_xW3GzpVVxXlVHfggKm5fASE06fL6L93X4P1NJbP1NxVbVoQPKGp8Nwa22-2KQ";
/// HKDF-SHA256 of the session key, salt "rim.opaque.transport.v1", info "server share".
const TRANSPORT_KEY: &str = "AcTQV-PAHwCfX5gFAu5hQQeYOzHkvPRgl0xhRjdDd4M";
/// 32 bytes of 0x5a sealed under the transport key with an empty AAD: the 12-byte nonce, the
/// ciphertext and the tag.
const SEALED: &str =
    "lanCDJ9UaKg1UCHI9AYsD6XlZlk7GqeMYNU3fKHD7ezvLbddfAX_XSumltxjRt8AyI5CTi6qCLcxntdt";

fn unb64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).expect("base64url")
}

#[test]
fn the_transport_sample_derives_and_opens() {
    let session_key = unb64(SESSION_KEY);
    let key = transport_key(&session_key, LABEL_SERVER_SHARE).expect("transport key");
    assert_eq!(key.as_slice(), unb64(TRANSPORT_KEY).as_slice());
    let opened =
        transport_open(&session_key, LABEL_SERVER_SHARE, SEALED).expect("the sample opens");
    assert_eq!(opened.as_slice(), &[0x5a; 32]);
    assert!(transport_open(&session_key, "keep key", SEALED).is_none());
}
