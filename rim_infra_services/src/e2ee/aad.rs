//! The authenticated data of a PrivateMessage: cleartext JSON the sender signs inside the
//! encryption, and the only part of an MLS message the server reads. Unknown and duplicate
//! fields are refused and every field must be present.

use std::collections::HashSet;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use uuid::Uuid;

use rim_services::e2ee::{MAX_APPLICATION_AAD_BYTES, MAX_COMMIT_AAD_BYTES};
use rim_services::{ServiceError, ServiceResult};

use crate::e2ee::franking::FRANKING_LEN;

/// Most principals one message may mention.
pub(crate) const MAX_MENTIONS: usize = 100;

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::Validation(format!("authenticated data: {}", message.into()))
}

fn parse<'a, T: Deserialize<'a>>(bytes: &'a [u8], max: usize) -> ServiceResult<T> {
    if bytes.len() > max {
        return Err(invalid(format!("larger than {max} bytes")));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("not UTF-8"))?;
    serde_json::from_str(text).map_err(|e| invalid(e.to_string()))
}

fn check_header(v: u32, kind: &str, expected: &str) -> ServiceResult<()> {
    if v != 1 {
        return Err(invalid(format!("unsupported version {v}")));
    }
    if kind != expected {
        return Err(invalid(format!("expected type `{expected}`")));
    }
    Ok(())
}

fn check_principal(principal_id: &str) -> ServiceResult<()> {
    if rim_identity::is_principal_id(principal_id) {
        Ok(())
    } else {
        Err(invalid(format!("`{principal_id}` is not a principal id")))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawApplication {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    message_id: Uuid,
    franking: String,
    mentions: Vec<String>,
    mention_all: bool,
}

/// The authenticated data of an application message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApplicationAad {
    pub message_id: Uuid,
    /// The franking commitment to the content.
    pub franking: [u8; FRANKING_LEN],
    /// Distinct principal ids, in the sender's order.
    pub mentions: Vec<String>,
    pub mention_all: bool,
}

pub(crate) fn parse_application(bytes: &[u8]) -> ServiceResult<ApplicationAad> {
    let raw: RawApplication = parse(bytes, MAX_APPLICATION_AAD_BYTES)?;
    check_header(raw.v, &raw.kind, "application")?;
    let franking: [u8; FRANKING_LEN] = URL_SAFE_NO_PAD
        .decode(&raw.franking)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| invalid("franking must be 32 bytes of unpadded base64url"))?;
    if raw.mentions.len() > MAX_MENTIONS {
        return Err(invalid(format!("at most {MAX_MENTIONS} mentions")));
    }
    let mut seen = HashSet::new();
    for principal_id in &raw.mentions {
        check_principal(principal_id)?;
        if !seen.insert(principal_id) {
            return Err(invalid("mentions must be distinct"));
        }
    }
    Ok(ApplicationAad {
        message_id: raw.message_id,
        franking,
        mentions: raw.mentions,
        mention_all: raw.mention_all,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawAdd {
    principal_id: String,
    device_id: Uuid,
    key_package_ref: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawRemove {
    principal_id: String,
    device_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommit {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    adds: Vec<RawAdd>,
    removes: Vec<RawRemove>,
}

/// A device a Commit adds, with the key package it uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitAdd {
    pub principal_id: String,
    pub device_id: Uuid,
    pub key_package_ref: Vec<u8>,
}

/// A device a Commit removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitRemove {
    pub principal_id: String,
    pub device_id: Uuid,
}

/// The membership summary of a Commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitAad {
    pub adds: Vec<CommitAdd>,
    pub removes: Vec<CommitRemove>,
}

pub(crate) fn parse_commit(bytes: &[u8]) -> ServiceResult<CommitAad> {
    let raw: RawCommit = parse(bytes, MAX_COMMIT_AAD_BYTES)?;
    check_header(raw.v, &raw.kind, "commit")?;
    let mut adds = Vec::with_capacity(raw.adds.len());
    let mut added = HashSet::new();
    let mut refs = HashSet::new();
    for add in raw.adds {
        check_principal(&add.principal_id)?;
        let key_package_ref = URL_SAFE_NO_PAD
            .decode(&add.key_package_ref)
            .ok()
            .filter(|r| !r.is_empty())
            .ok_or_else(|| invalid("keyPackageRef must be unpadded base64url"))?;
        if !added.insert((add.principal_id.clone(), add.device_id))
            || !refs.insert(key_package_ref.clone())
        {
            return Err(invalid("a device or key package is added twice"));
        }
        adds.push(CommitAdd {
            principal_id: add.principal_id,
            device_id: add.device_id,
            key_package_ref,
        });
    }
    let mut removes = Vec::with_capacity(raw.removes.len());
    let mut removed = HashSet::new();
    for remove in raw.removes {
        check_principal(&remove.principal_id)?;
        if !removed.insert((remove.principal_id.clone(), remove.device_id)) {
            return Err(invalid("a device is removed twice"));
        }
        removes.push(CommitRemove {
            principal_id: remove.principal_id,
            device_id: remove.device_id,
        });
    }
    Ok(CommitAad { adds, removes })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRINCIPAL: &str = "rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo";
    const APPLICATION: &str = r#"{"v":1,"type":"application","messageId":"0199d1a0-0000-7000-8000-000000000002","franking":"cexIhmsoNkTBNQjdxcjU_McrpRch3NO3SxblpQ1ah00","mentions":["rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo"],"mentionAll":false}"#;
    const COMMIT: &str = r#"{"v":1,"type":"commit","adds":[{"principalId":"rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo","deviceId":"00000000-0000-0000-0000-000000000002","keyPackageRef":"wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna29zd3t8"}],"removes":[]}"#;

    fn refused(result: ServiceResult<impl std::fmt::Debug>) -> String {
        match result {
            Err(ServiceError::Validation(message)) => message,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[test]
    fn known_answer_application_data() {
        assert_eq!(APPLICATION.len(), 220);
        let aad = parse_application(APPLICATION.as_bytes()).expect("parse");
        assert_eq!(
            aad.message_id,
            "0199d1a0-0000-7000-8000-000000000002"
                .parse::<Uuid>()
                .unwrap()
        );
        assert_eq!(aad.franking[0], 0x71);
        assert_eq!(aad.mentions, vec![PRINCIPAL.to_string()]);
        assert!(!aad.mention_all);
    }

    #[test]
    fn application_data_rules() {
        let unknown = APPLICATION.replacen('{', r#"{"extra":1,"#, 1);
        assert!(refused(parse_application(unknown.as_bytes())).contains("unknown field"));
        let duplicate = APPLICATION.replacen('{', r#"{"mentionAll":true,"#, 1);
        assert!(refused(parse_application(duplicate.as_bytes())).contains("duplicate field"));
        let missing = APPLICATION.replacen(r#","mentionAll":false"#, "", 1);
        assert!(refused(parse_application(missing.as_bytes())).contains("missing field"));
        let short = APPLICATION.replacen("cexIhmsoNkTBNQjdxcjU_McrpRch3NO3SxblpQ1ah00", "AAAA", 1);
        assert!(refused(parse_application(short.as_bytes())).contains("franking"));
        let not_principal = APPLICATION.replacen(PRINCIPAL, "rimp0_x", 1);
        assert!(refused(parse_application(not_principal.as_bytes())).contains("principal"));
        let twice = APPLICATION.replacen(
            &format!(r#"["{PRINCIPAL}"]"#),
            &format!(r#"["{PRINCIPAL}","{PRINCIPAL}"]"#),
            1,
        );
        assert!(refused(parse_application(twice.as_bytes())).contains("distinct"));
        let wrong_type = APPLICATION.replacen("application", "commit", 1);
        assert!(refused(parse_application(wrong_type.as_bytes())).contains("expected type"));
        let v2 = APPLICATION.replacen(r#""v":1"#, r#""v":2"#, 1);
        assert!(refused(parse_application(v2.as_bytes())).contains("version"));
        let big = format!("{}{}", " ".repeat(MAX_APPLICATION_AAD_BYTES), APPLICATION);
        assert!(refused(parse_application(big.as_bytes())).contains("larger"));
        let null = APPLICATION.replacen(r#""mentionAll":false"#, r#""mentionAll":null"#, 1);
        assert!(refused(parse_application(null.as_bytes())).contains("null"));
    }

    #[test]
    fn known_answer_commit_data() {
        let aad = parse_commit(COMMIT.as_bytes()).expect("parse");
        assert_eq!(aad.adds.len(), 1);
        assert_eq!(aad.adds[0].device_id, Uuid::from_u128(2));
        assert_eq!(
            aad.adds[0].key_package_ref,
            (0xc0..=0xdf).collect::<Vec<u8>>()
        );
        assert!(aad.removes.is_empty());
    }

    #[test]
    fn commit_data_rules() {
        let twice = COMMIT.replacen(
            r#""removes":[]"#,
            r#""removes":[{"principalId":"rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo","deviceId":"00000000-0000-0000-0000-000000000003"},{"principalId":"rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo","deviceId":"00000000-0000-0000-0000-000000000003"}]"#,
            1,
        );
        assert!(refused(parse_commit(twice.as_bytes())).contains("removed twice"));
        let extra = COMMIT.replacen(r#""deviceId""#, r#""extra":1,"deviceId""#, 1);
        assert!(refused(parse_commit(extra.as_bytes())).contains("unknown field"));
        let no_removes = COMMIT.replacen(r#","removes":[]"#, "", 1);
        assert!(refused(parse_commit(no_removes.as_bytes())).contains("missing field"));
        let bad_ref = COMMIT.replacen("wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna29zd3t8", "!!", 1);
        assert!(refused(parse_commit(bad_ref.as_bytes())).contains("keyPackageRef"));
        let big = format!("{}{}", " ".repeat(MAX_COMMIT_AAD_BYTES), COMMIT);
        assert!(refused(parse_commit(big.as_bytes())).contains("larger"));
    }
}
