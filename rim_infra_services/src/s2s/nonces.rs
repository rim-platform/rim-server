use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rim_core::s2s::repository::{PeerServerRepository, S2sNonceRepository};
use rim_services::ServiceResult;
use rim_services::s2s::service::S2sNonceService;

/// Request nonces in the database, so a replay is refused whichever node it reaches.
pub struct DefaultS2sNonceService {
    nonces: Arc<dyn S2sNonceRepository>,
    peers: Arc<dyn PeerServerRepository>,
}

impl DefaultS2sNonceService {
    pub fn new(nonces: Arc<dyn S2sNonceRepository>, peers: Arc<dyn PeerServerRepository>) -> Self {
        Self { nonces, peers }
    }
}

#[async_trait]
impl S2sNonceService for DefaultS2sNonceService {
    async fn accept(
        &self,
        peer_server_id: &str,
        nonce: &str,
        expires_at: DateTime<Utc>,
    ) -> ServiceResult<bool> {
        let fresh = self
            .nonces
            .insert(peer_server_id, nonce, expires_at)
            .await?;
        if fresh {
            self.peers.touch(peer_server_id).await?;
        }
        Ok(fresh)
    }

    async fn purge_expired(&self) -> ServiceResult<u64> {
        Ok(self.nonces.delete_expired().await?)
    }
}

#[cfg(test)]
mod tests {
    use mockall::predicate::eq;
    use rim_core::s2s::repository::{MockPeerServerRepository, MockS2sNonceRepository};

    use super::*;

    #[tokio::test]
    async fn a_fresh_nonce_is_accepted_and_touches_the_peer() {
        let mut nonces = MockS2sNonceRepository::new();
        nonces
            .expect_insert()
            .with(eq("rims1_a"), eq("n1"), mockall::predicate::always())
            .returning(|_, _, _| Box::pin(async { Ok(true) }));
        let mut peers = MockPeerServerRepository::new();
        peers
            .expect_touch()
            .with(eq("rims1_a"))
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
        let service = DefaultS2sNonceService::new(Arc::new(nonces), Arc::new(peers));
        assert!(
            service
                .accept("rims1_a", "n1", Utc::now())
                .await
                .expect("accept")
        );
    }

    #[tokio::test]
    async fn a_seen_nonce_is_refused_without_touching_the_peer() {
        let mut nonces = MockS2sNonceRepository::new();
        nonces
            .expect_insert()
            .returning(|_, _, _| Box::pin(async { Ok(false) }));
        let mut peers = MockPeerServerRepository::new();
        peers.expect_touch().never();
        let service = DefaultS2sNonceService::new(Arc::new(nonces), Arc::new(peers));
        assert!(
            !service
                .accept("rims1_a", "n1", Utc::now())
                .await
                .expect("accept")
        );
    }
}
