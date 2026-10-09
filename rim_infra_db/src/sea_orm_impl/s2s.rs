use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::{peer_server, server_descriptor, server_key_log_entry};
use rim_core::key_identity::model::KeyLogEntryType;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::s2s::model::{
    PeerPin, PeerRotationSigner, PeerServer, PeerStatus, ServerDescriptorRecord,
    ServerKeyLogRecord, SignedRecord,
};
use rim_core::s2s::repository::{
    PeerServerRepository, S2sNonceRepository, ServerIdentityRepository,
};
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, FromQueryResult, QueryResult,
    Statement, TransactionError, Value,
};

use crate::DbHandle;
use crate::error::map_db_err;

fn to_i32(value: u32) -> RepoResult<i32> {
    i32::try_from(value).map_err(|_| RepoError::new(RepoErrorKind::InvalidData))
}

fn to_u32(value: i32) -> RepoResult<u32> {
    u32::try_from(value).map_err(|_| RepoError::new(RepoErrorKind::InvalidData))
}

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

fn map_txn_err(err: TransactionError<DbErr>) -> RepoError {
    match err {
        TransactionError::Connection(e) | TransactionError::Transaction(e) => map_db_err(e),
    }
}

/// This server's key log and descriptor on `server_key_log_entry` and `server_descriptor`.
pub struct SeaOrmServerIdentityRepository {
    db: DbHandle,
}

impl SeaOrmServerIdentityRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ServerIdentityRepository for SeaOrmServerIdentityRepository {
    async fn key_log(&self) -> RepoResult<Vec<ServerKeyLogRecord>> {
        let rows = self
            .db
            .query_all_raw(stmt(
                "SELECT seq, entry_type, payload, signature, created_at \
                 FROM server_key_log_entry ORDER BY seq",
                vec![],
            ))
            .await
            .map_err(map_db_err)?;
        rows.iter()
            .map(|row| {
                let m =
                    server_key_log_entry::Model::from_query_result(row, "").map_err(map_db_err)?;
                Ok(ServerKeyLogRecord {
                    seq: to_u32(m.seq)?,
                    entry_type: KeyLogEntryType::parse(&m.entry_type)
                        .ok_or_else(|| RepoError::new(RepoErrorKind::InvalidData))?,
                    statement: SignedRecord {
                        payload: m.payload,
                        signature: m.signature,
                    },
                })
            })
            .collect()
    }

    async fn append_key_log_entry(&self, entry: ServerKeyLogRecord) -> RepoResult<()> {
        self.db
            .execute_raw(stmt(
                "INSERT INTO server_key_log_entry (seq, entry_type, payload, signature) \
                 VALUES ($1, $2, $3, $4)",
                vec![
                    to_i32(entry.seq)?.into(),
                    entry.entry_type.as_str().into(),
                    entry.statement.payload.into(),
                    entry.statement.signature.into(),
                ],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn descriptor(&self) -> RepoResult<Option<ServerDescriptorRecord>> {
        let row = self
            .db
            .query_one_raw(stmt(
                "SELECT id, version, payload, signature, updated_at \
                 FROM server_descriptor WHERE id = 1",
                vec![],
            ))
            .await
            .map_err(map_db_err)?;
        row.map(|row| {
            let m = server_descriptor::Model::from_query_result(&row, "").map_err(map_db_err)?;
            Ok(ServerDescriptorRecord {
                version: to_u32(m.version)?,
                statement: SignedRecord {
                    payload: m.payload,
                    signature: m.signature,
                },
            })
        })
        .transpose()
    }

    async fn store_descriptor(&self, descriptor: ServerDescriptorRecord) -> RepoResult<bool> {
        let result = self
            .db
            .execute_raw(stmt(
                "INSERT INTO server_descriptor (id, version, payload, signature) \
                 VALUES (1, $1, $2, $3) \
                 ON CONFLICT (id) DO UPDATE SET version = EXCLUDED.version, \
                 payload = EXCLUDED.payload, signature = EXCLUDED.signature, updated_at = now() \
                 WHERE server_descriptor.version < EXCLUDED.version",
                vec![
                    to_i32(descriptor.version)?.into(),
                    descriptor.statement.payload.into(),
                    descriptor.statement.signature.into(),
                ],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected() == 1)
    }
}

const PEER_COLUMNS: &str = "server_id, origin, head_seq, head_key, previous_key, rotated_at, \
     rotation_signer, descriptor_version, descriptor_payload, descriptor_signature, status, \
     first_seen_at, fetched_at, last_request_at";

fn to_peer(row: &QueryResult) -> RepoResult<PeerServer> {
    let m = peer_server::Model::from_query_result(row, "").map_err(map_db_err)?;
    let invalid = || RepoError::new(RepoErrorKind::InvalidData);
    Ok(PeerServer {
        server_id: m.server_id,
        origin: m.origin,
        head_seq: to_u32(m.head_seq)?,
        head_key: m.head_key,
        previous_key: m.previous_key,
        rotated_at: m.rotated_at.map(Into::into),
        rotation_signer: m
            .rotation_signer
            .as_deref()
            .map(|s| PeerRotationSigner::parse(s).ok_or_else(invalid))
            .transpose()?,
        descriptor_version: to_u32(m.descriptor_version)?,
        descriptor: SignedRecord {
            payload: m.descriptor_payload,
            signature: m.descriptor_signature,
        },
        status: PeerStatus::parse(&m.status).ok_or_else(invalid)?,
        first_seen_at: m.first_seen_at.into(),
        fetched_at: m.fetched_at.into(),
        last_request_at: m.last_request_at.map(Into::into),
    })
}

/// The values of a pin's columns after `server_id`, in `PEER_COLUMNS` order up to the
/// descriptor signature.
fn pin_values(pin: &PeerPin) -> RepoResult<Vec<Value>> {
    Ok(vec![
        pin.server_id.clone().into(),
        pin.origin.clone().into(),
        to_i32(pin.head_seq)?.into(),
        pin.head_key.clone().into(),
        pin.previous_key.clone().into(),
        pin.rotated_at.into(),
        pin.rotation_signer.map(|s| s.as_str().to_string()).into(),
        to_i32(pin.descriptor_version)?.into(),
        pin.descriptor.payload.clone().into(),
        pin.descriptor.signature.clone().into(),
    ])
}

async fn insert_log_entries(
    txn: &DatabaseTransaction,
    server_id: &str,
    entries: &[(i32, SignedRecord)],
) -> Result<(), DbErr> {
    for (seq, entry) in entries {
        txn.execute_raw(stmt(
            "INSERT INTO peer_key_log_entry (server_id, seq, payload, signature) \
             VALUES ($1, $2, $3, $4)",
            vec![
                server_id.into(),
                (*seq).into(),
                entry.payload.clone().into(),
                entry.signature.clone().into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

fn numbered(log: &[SignedRecord], from: usize) -> RepoResult<Vec<(i32, SignedRecord)>> {
    log.iter()
        .enumerate()
        .skip(from)
        .map(|(seq, entry)| {
            let seq = i32::try_from(seq).map_err(|_| RepoError::new(RepoErrorKind::InvalidData))?;
            Ok((seq, entry.clone()))
        })
        .collect()
}

/// Pinned peers on `peer_server` and `peer_key_log_entry`.
pub struct SeaOrmPeerServerRepository {
    db: DbHandle,
}

impl SeaOrmPeerServerRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl PeerServerRepository for SeaOrmPeerServerRepository {
    async fn find(&self, server_id: &str) -> RepoResult<Option<PeerServer>> {
        let row = self
            .db
            .query_one_raw(stmt(
                &format!("SELECT {PEER_COLUMNS} FROM peer_server WHERE server_id = $1"),
                vec![server_id.into()],
            ))
            .await
            .map_err(map_db_err)?;
        row.as_ref().map(to_peer).transpose()
    }

    async fn key_log(&self, server_id: &str) -> RepoResult<Vec<SignedRecord>> {
        let rows = self
            .db
            .query_all_raw(stmt(
                "SELECT payload, signature FROM peer_key_log_entry \
                 WHERE server_id = $1 ORDER BY seq",
                vec![server_id.into()],
            ))
            .await
            .map_err(map_db_err)?;
        rows.iter()
            .map(|row| {
                Ok(SignedRecord {
                    payload: row.try_get("", "payload").map_err(map_db_err)?,
                    signature: row.try_get("", "signature").map_err(map_db_err)?,
                })
            })
            .collect()
    }

    async fn insert(&self, pin: PeerPin) -> RepoResult<PeerServer> {
        let values = pin_values(&pin)?;
        let entries = numbered(&pin.log, 0)?;
        let server_id = pin.server_id.clone();
        let row = self
            .db
            .transaction::<_, Option<QueryResult>, DbErr>(move |txn| {
                Box::pin(async move {
                    let row = txn
                        .query_one_raw(stmt(
                            &format!(
                                "INSERT INTO peer_server (server_id, origin, head_seq, head_key, \
                                 previous_key, rotated_at, rotation_signer, descriptor_version, \
                                 descriptor_payload, descriptor_signature, fetched_at) \
                                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now()) \
                                 RETURNING {PEER_COLUMNS}"
                            ),
                            values,
                        ))
                        .await?;
                    insert_log_entries(txn, &server_id, &entries).await?;
                    Ok(row)
                })
            })
            .await
            .map_err(map_txn_err)?;
        row.as_ref()
            .map(to_peer)
            .transpose()?
            .ok_or_else(|| RepoError::new(RepoErrorKind::Internal))
    }

    async fn advance(
        &self,
        pin: PeerPin,
        expected_head_seq: u32,
    ) -> RepoResult<Option<PeerServer>> {
        let mut values = pin_values(&pin)?;
        values.push(to_i32(expected_head_seq)?.into());
        let from = usize::try_from(expected_head_seq)
            .map_err(|_| RepoError::new(RepoErrorKind::InvalidData))?
            + 1;
        let entries = numbered(&pin.log, from)?;
        let server_id = pin.server_id.clone();
        let row = self
            .db
            .transaction::<_, Option<QueryResult>, DbErr>(move |txn| {
                Box::pin(async move {
                    let row = txn
                        .query_one_raw(stmt(
                            &format!(
                                "UPDATE peer_server SET origin = $2, head_seq = $3, head_key = $4, \
                                 previous_key = $5, rotated_at = $6, rotation_signer = $7, \
                                 descriptor_version = $8, descriptor_payload = $9, \
                                 descriptor_signature = $10, fetched_at = now() \
                                 WHERE server_id = $1 AND head_seq = $11 AND status = 'active' \
                                 RETURNING {PEER_COLUMNS}"
                            ),
                            values,
                        ))
                        .await?;
                    if row.is_some() {
                        insert_log_entries(txn, &server_id, &entries).await?;
                    }
                    Ok(row)
                })
            })
            .await
            .map_err(map_txn_err)?;
        row.as_ref().map(to_peer).transpose()
    }

    async fn mark_fetched(
        &self,
        server_id: &str,
        descriptor: Option<(u32, SignedRecord)>,
    ) -> RepoResult<()> {
        let statement = match descriptor {
            Some((version, record)) => stmt(
                "UPDATE peer_server SET fetched_at = now(), descriptor_version = $2, \
                 descriptor_payload = $3, descriptor_signature = $4 \
                 WHERE server_id = $1 AND descriptor_version < $2",
                vec![
                    server_id.into(),
                    to_i32(version)?.into(),
                    record.payload.into(),
                    record.signature.into(),
                ],
            ),
            None => stmt(
                "UPDATE peer_server SET fetched_at = now() WHERE server_id = $1",
                vec![server_id.into()],
            ),
        };
        self.db.execute_raw(statement).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn set_status(&self, server_id: &str, status: PeerStatus) -> RepoResult<bool> {
        let result = self
            .db
            .execute_raw(stmt(
                "UPDATE peer_server SET status = $2 WHERE server_id = $1",
                vec![server_id.into(), status.as_str().into()],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected() == 1)
    }

    async fn delete(&self, server_id: &str) -> RepoResult<bool> {
        let result = self
            .db
            .execute_raw(stmt(
                "DELETE FROM peer_server WHERE server_id = $1",
                vec![server_id.into()],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected() == 1)
    }

    async fn touch(&self, server_id: &str) -> RepoResult<()> {
        self.db
            .execute_raw(stmt(
                "UPDATE peer_server SET last_request_at = now() WHERE server_id = $1",
                vec![server_id.into()],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(())
    }
}

/// Request nonces on `s2s_nonce`.
pub struct SeaOrmS2sNonceRepository {
    db: DbHandle,
}

impl SeaOrmS2sNonceRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl S2sNonceRepository for SeaOrmS2sNonceRepository {
    async fn insert(
        &self,
        peer_server_id: &str,
        nonce: &str,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<bool> {
        let result = self
            .db
            .execute_raw(stmt(
                "INSERT INTO s2s_nonce (peer_server_id, nonce, expires_at) VALUES ($1, $2, $3) \
                 ON CONFLICT (peer_server_id, nonce) DO NOTHING",
                vec![peer_server_id.into(), nonce.into(), expires_at.into()],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected() == 1)
    }

    async fn delete_expired(&self) -> RepoResult<u64> {
        let result = self
            .db
            .execute_raw(stmt(
                "DELETE FROM s2s_nonce WHERE expires_at < now()",
                vec![],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected())
    }
}
