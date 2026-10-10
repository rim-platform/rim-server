//! Proof-of-work for key-only signups on a relay in `pow` registration mode: an 8-byte nonce
//! such that `SHA-256("rim.pow.v1\0" || relay server id || "\0" || inception payload || nonce)`
//! starts with the relay's `powBits` zero bits. The work is bound to one relay and one
//! principal; checking it costs one hash.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// Context bytes the hash starts with.
pub const POW_CONTEXT: &[u8] = b"rim.pow.v1\0";

/// Length of a proof-of-work nonce, in bytes.
pub const POW_NONCE_LEN: usize = 8;

/// Highest difficulty a server may ask for.
pub const MAX_POW_BITS: u32 = 40;

/// Why a proof-of-work was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PowError {
    #[error("proofOfWork is not 8 bytes of unpadded base64url")]
    Malformed,
    #[error("proofOfWork does not have the required leading zero bits")]
    TooWeak,
}

/// The hash a nonce is judged by.
pub fn pow_hash(server_id: &str, inception_payload: &str, nonce: &[u8; POW_NONCE_LEN]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(POW_CONTEXT);
    hasher.update(server_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(inception_payload.as_bytes());
    hasher.update(nonce);
    hasher.finalize().into()
}

/// How many zero bits a hash starts with.
pub fn leading_zero_bits(hash: &[u8]) -> u32 {
    let mut bits = 0;
    for byte in hash {
        if *byte == 0 {
            bits += 8;
        } else {
            return bits + byte.leading_zeros();
        }
    }
    bits
}

/// Check a nonce as a client sends it (unpadded base64url) against `bits`.
///
/// # Errors
/// [`PowError::Malformed`] when it is not 8 bytes of unpadded base64url,
/// [`PowError::TooWeak`] when its hash has fewer than `bits` leading zero bits.
pub fn check_pow(
    server_id: &str,
    inception_payload: &str,
    nonce: &str,
    bits: u32,
) -> Result<(), PowError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(nonce.trim())
        .map_err(|_| PowError::Malformed)?;
    let nonce: [u8; POW_NONCE_LEN] = bytes.try_into().map_err(|_| PowError::Malformed)?;
    if leading_zero_bits(&pow_hash(server_id, inception_payload, &nonce)) >= bits {
        Ok(())
    } else {
        Err(PowError::TooWeak)
    }
}

/// Find the smallest counter (as a big-endian nonce) that meets `bits`: what a client does,
/// for tests and tools. Expect about `2^bits` hashes.
#[cfg(any(test, feature = "testing"))]
pub fn solve_pow(server_id: &str, inception_payload: &str, bits: u32) -> [u8; POW_NONCE_LEN] {
    let mut counter: u64 = 0;
    loop {
        let nonce = counter.to_be_bytes();
        if leading_zero_bits(&pow_hash(server_id, inception_payload, &nonce)) >= bits {
            return nonce;
        }
        counter = counter.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_leading_zero_bits() {
        assert_eq!(leading_zero_bits(&[0x80]), 0);
        assert_eq!(leading_zero_bits(&[0x01]), 7);
        assert_eq!(leading_zero_bits(&[0x00, 0x2f]), 10);
        assert_eq!(leading_zero_bits(&[0x00, 0x00]), 16);
    }

    #[test]
    fn refuses_malformed_and_weak_nonces() {
        let server = "rims1_x";
        let inception = "{}";
        assert_eq!(
            check_pow(server, inception, "AAAA", 1),
            Err(PowError::Malformed)
        );
        assert_eq!(
            check_pow(server, inception, "AAAAAAAAAAA=", 1),
            Err(PowError::Malformed)
        );
        let nonce = URL_SAFE_NO_PAD.encode(solve_pow(server, inception, 8));
        assert_eq!(check_pow(server, inception, &nonce, 8), Ok(()));
        // Bound to the server and the inception.
        let other = (0..)
            .map(|n: u64| URL_SAFE_NO_PAD.encode(n.to_be_bytes()))
            .find(|n| check_pow(server, inception, n, 8).is_err())
            .expect("a weak nonce");
        assert_eq!(
            check_pow(server, inception, &other, 8),
            Err(PowError::TooWeak)
        );
        assert_eq!(check_pow(server, inception, &nonce, 0), Ok(()));
    }
}
