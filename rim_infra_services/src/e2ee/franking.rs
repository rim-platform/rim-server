//! Message franking: the sender's HMAC commitment to a message's content, and the server's
//! receipt over that commitment, the message's ids and its send time. A report reveals the
//! content and the franking key, and the server checks both HMACs again.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

/// Length of a franking key, a commitment and a receipt.
pub(crate) const FRANKING_LEN: usize = 32;

const COMMITMENT_CONTEXT: &[u8] = b"rim.e2ee.franking.v1\0";
const RECEIPT_CONTEXT: &[u8] = b"rim.e2ee.franking-receipt.v1\0";

/// What a receipt covers besides the commitment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReceiptFields {
    pub message_id: Uuid,
    pub conversation_id: Uuid,
    pub sender_user_id: Uuid,
    /// The sender's client device id.
    pub sender_device_id: Uuid,
    pub sent_at_us: i64,
}

fn mac(key: &[u8]) -> HmacSha256 {
    // HMAC accepts keys of any length.
    HmacSha256::new_from_slice(key).unwrap_or_else(|_| unreachable!("HMAC takes any key length"))
}

/// `HMAC-SHA256(franking_key, "rim.e2ee.franking.v1\0" || content)`.
pub(crate) fn commitment(franking_key: &[u8; FRANKING_LEN], content: &[u8]) -> [u8; FRANKING_LEN] {
    let mut mac = mac(franking_key);
    mac.update(COMMITMENT_CONTEXT);
    mac.update(content);
    mac.finalize().into_bytes().into()
}

/// The bytes a receipt is computed over: the context, the four ids as 16 raw bytes each, the
/// send time as a big-endian i64, and the commitment.
pub(crate) fn receipt_input(fields: &ReceiptFields, commitment: &[u8]) -> Vec<u8> {
    let mut input = Vec::with_capacity(RECEIPT_CONTEXT.len() + 4 * 16 + 8 + commitment.len());
    input.extend_from_slice(RECEIPT_CONTEXT);
    input.extend_from_slice(fields.message_id.as_bytes());
    input.extend_from_slice(fields.conversation_id.as_bytes());
    input.extend_from_slice(fields.sender_user_id.as_bytes());
    input.extend_from_slice(fields.sender_device_id.as_bytes());
    input.extend_from_slice(&fields.sent_at_us.to_be_bytes());
    input.extend_from_slice(commitment);
    input
}

/// The server's receipt under its franking key `server_key`.
pub(crate) fn receipt(
    server_key: &[u8],
    fields: &ReceiptFields,
    commitment: &[u8],
) -> [u8; FRANKING_LEN] {
    let mut mac = mac(server_key);
    mac.update(&receipt_input(fields, commitment));
    mac.finalize().into_bytes().into()
}

/// Whether `claimed` is the receipt for these fields, compared in constant time.
pub(crate) fn verify_receipt(
    server_key: &[u8],
    fields: &ReceiptFields,
    commitment: &[u8],
    claimed: &[u8],
) -> bool {
    let mut mac = mac(server_key);
    mac.update(&receipt_input(fields, commitment));
    mac.verify_slice(claimed).is_ok()
}

/// `sent_at` in microseconds since the Unix epoch.
pub(crate) fn sent_at_us(sent_at: DateTime<Utc>) -> i64 {
    sent_at.timestamp_micros()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn range(start: u8) -> [u8; 32] {
        std::array::from_fn(|i| start + i as u8)
    }

    const CONTENT: &str = r#"{"v":1,"type":"message","id":"0199d1a0-0000-7000-8000-000000000002","rev":0,"threadRootId":null,"body":"hi <@rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo>","mentions":["rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo"],"mentionAll":false,"replyTo":null,"forwardedFrom":null,"attachments":[{"blobSha256":"EHFcZ6hUOIdDi32ptdrCXM2PTHQwKF46-0Ls56yQrac","secret":"oKGio6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr8","name":"hello.txt","mime":"text/plain","size":18}],"linkPreviews":[]}"#;
    const COMMITMENT: &str = "71ec48866b283644c13508ddc5c8d4fcc72ba51721dcd3b74b16e5a50d5a874d";
    const RECEIPT_INPUT: &str = "72696d2e653265652e6672616e6b696e672d726563656970742e7631000199d1a00000700080000000000000020199d1a00000700080000000000000010199d1a00000700080000000000000aa0000000000000000000000000000000100065d5d078a824071ec48866b283644c13508ddc5c8d4fcc72ba51721dcd3b74b16e5a50d5a874d";
    const RECEIPT: &str = "9c25cd6659b97074696a9445ad323e061debaaf5f97af5ed2f2cd46150d3db75";

    fn fields() -> ReceiptFields {
        ReceiptFields {
            message_id: "0199d1a0-0000-7000-8000-000000000002".parse().unwrap(),
            conversation_id: "0199d1a0-0000-7000-8000-000000000001".parse().unwrap(),
            sender_user_id: "0199d1a0-0000-7000-8000-0000000000aa".parse().unwrap(),
            sender_device_id: Uuid::from_u128(1),
            sent_at_us: 1_791_504_000_123_456,
        }
    }

    #[test]
    fn known_answer_commitment_and_receipt() {
        assert_eq!(CONTENT.len(), 480);
        let c = commitment(&range(0x60), CONTENT.as_bytes());
        assert_eq!(hex(&c), COMMITMENT);

        let sent_at = DateTime::parse_from_rfc3339("2026-10-09T00:00:00.123456Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(sent_at_us(sent_at), fields().sent_at_us);
        let input = receipt_input(&fields(), &c);
        assert_eq!(input.len(), 133);
        assert_eq!(hex(&input), RECEIPT_INPUT);
        let r = receipt(&range(0x80), &fields(), &c);
        assert_eq!(hex(&r), RECEIPT);
        assert!(verify_receipt(&range(0x80), &fields(), &c, &r));
    }

    #[test]
    fn any_changed_input_fails_the_receipt() {
        let key = range(0x80);
        let c = commitment(&range(0x60), CONTENT.as_bytes());
        let r = receipt(&key, &fields(), &c);
        let tampered = commitment(&range(0x60), CONTENT.replace("hi", "ho").as_bytes());
        assert!(!verify_receipt(&key, &fields(), &tampered, &r));
        let wrong_franking_key = commitment(&range(0x61), CONTENT.as_bytes());
        assert!(!verify_receipt(&key, &fields(), &wrong_franking_key, &r));
        assert!(!verify_receipt(&range(0x81), &fields(), &c, &r));
        let mut other = fields();
        other.sent_at_us += 1;
        assert!(!verify_receipt(&key, &other, &c, &r));
        let mut other = fields();
        other.sender_device_id = Uuid::from_u128(2);
        assert!(!verify_receipt(&key, &other, &c, &r));
        assert!(!verify_receipt(&key, &fields(), &c, &r[..31]));
    }
}
