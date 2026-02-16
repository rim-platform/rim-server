use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use getrandom::fill;
use sha2::{Digest, Sha256};

pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    fill(&mut bytes).expect("getrandom failed");
    let token = URL_SAFE_NO_PAD.encode(bytes);
    format!("rim_sess_{token}")
}

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let out = hasher.finalize();
    hex_encode(&out)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut res = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        res.push(HEX[(b >> 4) as usize] as char);
        res.push(HEX[(b & 0x0f) as usize] as char);
    }
    res
}
