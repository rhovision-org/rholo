//! **The checkpoint-certificate store** (sync/verification separation).
//!
//! A joiner no longer replays history to establish the finalized state: it verifies a certificate —
//! a checkpoint statement plus a supermajority of validator signatures — and starts from the state
//! the statement names. This store keeps those certificates so the node can serve them to joiners
//! (`GET /api/v1/checkpoint` and `/{digest}`) and re-verify its own acceptance after a restart.
//!
//! Two named stores, deliberately separate from [`crate::approved_store`]:
//!
//! * `checkpoint-certificates` is keyed by the **checkpoint digest**, so a lookup is a lookup of
//!   the exact statement signed; two statements cannot collide under one row.
//! * `checkpoint-head` holds the latest certificate this node accepted, under a single key. It is
//!   the serving pointer, not a statement: the digest-keyed row remains the record.
//!
//! The digest is computed by `rchain-casper::checkpoint` (this crate has no hashing/crypto
//! policy), so callers pass it in and this store only persists what it is given.

use std::sync::Arc;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::casper::protocol::checkpoint::CheckpointCertificate;
use rchain_shared::store_manager::KeyValueStoreManager;
use rchain_shared::typed_store::{Codec, KeyValueTypedStore, KeyValueTypedStoreCodec};

use crate::dag::codecs::{Blake2b256HashCodec, ByteCodec};

/// The latest accepted certificate's key in the `checkpoint-head` store.
pub const CHECKPOINT_HEAD_KEY: u8 = 44;

/// **The lowest height from which this node holds bodies and deploy-index rows.** Written when a
/// certified sync accepts a checkpoint (the history below its anchor is metadata only), lowered by
/// a backfill. Absent means *from genesis* — a node that never joined at a checkpoint.
pub const HISTORY_FROM_KEY: u8 = 45;

/// Big-endian `i64` codec for the history marker.
#[derive(Default)]
pub struct I64Codec;

impl Codec<i64> for I64Codec {
    fn encode(&self, value: &i64) -> Vec<u8> {
        value.to_be_bytes().to_vec()
    }

    fn decode(&self, bytes: &[u8]) -> Result<i64, String> {
        let arr: [u8; 8] = bytes
            .try_into()
            .map_err(|_| format!("expected 8 bytes, got {}", bytes.len()))?;
        Ok(i64::from_be_bytes(arr))
    }
}

/// JSON codec for a certificate. The wire type is already the signed statement's serialization;
/// persisting it as JSON keeps the store readable by an operator and avoids a second protobuf
/// schema whose only reader would be this store.
#[derive(Default)]
pub struct CheckpointCertificateCodec;

impl Codec<CheckpointCertificate> for CheckpointCertificateCodec {
    fn encode(&self, value: &CheckpointCertificate) -> Vec<u8> {
        // The `Codec` contract has no error channel, and a serialization failure here is not a
        // runtime condition: the type is a fixed set of strings, lists and options. An empty row is
        // refused by `decode`, so a hypothetical failure surfaces as an unreadable certificate
        // rather than a panic in a store write.
        serde_json::to_vec(value).unwrap_or_default()
    }

    fn decode(&self, bytes: &[u8]) -> Result<CheckpointCertificate, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("checkpoint certificate decode: {e}"))
    }
}

type DigestRow = Arc<dyn KeyValueTypedStore<Blake2b256Hash, CheckpointCertificate>>;
type HeadRow = Arc<dyn KeyValueTypedStore<u8, CheckpointCertificate>>;
type HistoryRow = Arc<dyn KeyValueTypedStore<u8, i64>>;

/// The certificate store: a digest-keyed record plus the latest-accepted pointer.
#[derive(Clone)]
pub struct CheckpointStore {
    certificates: DigestRow,
    head: HeadRow,
    history: HistoryRow,
}

impl CheckpointStore {
    /// Open (or create) the named stores over the same manager.
    pub async fn open(kvm: &dyn KeyValueStoreManager) -> Result<Self, String> {
        let certificates = kvm.store("checkpoint-certificates").await?;
        let head = kvm.store("checkpoint-head").await?;
        let history = kvm.store("checkpoint-history").await?;
        Ok(CheckpointStore {
            certificates: Arc::new(KeyValueTypedStoreCodec::new(
                certificates,
                Arc::new(Blake2b256HashCodec),
                Arc::new(CheckpointCertificateCodec),
            )),
            head: Arc::new(KeyValueTypedStoreCodec::new(
                head,
                Arc::new(ByteCodec),
                Arc::new(CheckpointCertificateCodec),
            )),
            history: Arc::new(KeyValueTypedStoreCodec::new(
                history,
                Arc::new(ByteCodec),
                Arc::new(I64Codec),
            )),
        })
    }

    /// The lowest height whose body and deploy-index rows this node holds, or `None` for a node
    /// that never joined at a checkpoint (coverage from genesis).
    pub async fn history_from(&self) -> Result<Option<i64>, String> {
        Ok(self
            .history
            .get(&[HISTORY_FROM_KEY])
            .await?
            .into_iter()
            .next()
            .flatten())
    }

    /// Record the history floor. Written read-back like every other accepted fact in this store.
    pub async fn set_history_from(&self, height: i64) -> Result<(), String> {
        self.history.put(&[(HISTORY_FROM_KEY, height)]).await?;
        if self.history.get(&[HISTORY_FROM_KEY]).await? != vec![Some(height)] {
            return Err("checkpoint history marker read-back mismatch".to_string());
        }
        Ok(())
    }

    /// Persist a certificate under its digest **and** make it the served head.
    ///
    /// Both writes are read back. A certificate a node cannot read back is a certificate it must not
    /// claim to serve — the same rule the approved store applies to the finalized fringe.
    pub async fn put(
        &self,
        digest: &Blake2b256Hash,
        certificate: &CheckpointCertificate,
    ) -> Result<(), String> {
        self.certificates
            .put(&[(*digest, certificate.clone())])
            .await?;
        if self.certificates.get(&[*digest]).await? != vec![Some(certificate.clone())] {
            return Err("checkpoint certificate read-back mismatch".to_string());
        }
        self.head
            .put(&[(CHECKPOINT_HEAD_KEY, certificate.clone())])
            .await?;
        if self.head.get(&[CHECKPOINT_HEAD_KEY]).await? != vec![Some(certificate.clone())] {
            return Err("checkpoint head read-back mismatch".to_string());
        }
        Ok(())
    }

    /// The certificate signed for exactly this digest, if this node holds it.
    pub async fn get(
        &self,
        digest: &Blake2b256Hash,
    ) -> Result<Option<CheckpointCertificate>, String> {
        Ok(self
            .certificates
            .get(&[*digest])
            .await?
            .into_iter()
            .next()
            .flatten())
    }

    /// The latest certificate this node accepted.
    pub async fn latest(&self) -> Result<Option<CheckpointCertificate>, String> {
        Ok(self
            .head
            .get(&[CHECKPOINT_HEAD_KEY])
            .await?
            .into_iter()
            .next()
            .flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::casper::protocol::checkpoint::{
        Checkpoint, CheckpointAttestation, CheckpointCertificate,
    };
    use rchain_shared::store_manager::InMemoryStoreManager;

    fn certificate(anchor_byte: u8) -> CheckpointCertificate {
        let anchor = format!("{}", "ab".repeat(32));
        CheckpointCertificate {
            checkpoint: Checkpoint {
                shard_id: "root".to_string(),
                anchor,
                anchor_height: i64::from(anchor_byte),
                anchor_post_state: "cd".repeat(32),
                fringe_hashes: vec!["ab".repeat(32)],
                parent_digest: None,
                epoch: 1,
            },
            attestations: vec![CheckpointAttestation {
                checkpoint_digest: "ef".repeat(32),
                sender: "01".repeat(65),
                sig_algorithm: "secp256k1".to_string(),
                sig: "02".repeat(64),
            }],
        }
    }

    #[tokio::test]
    async fn a_certificate_is_found_by_its_digest_and_as_the_head_after_a_reopen() {
        let kvm = InMemoryStoreManager::default();
        let store = CheckpointStore::open(&kvm).await.expect("open");
        assert!(store.latest().await.expect("latest").is_none());

        let cert = certificate(7);
        let digest = Blake2b256Hash::from_byte_array(&[0x11; 32]);
        store.put(&digest, &cert).await.expect("put");
        assert_eq!(store.get(&digest).await.expect("get"), Some(cert.clone()));
        assert_eq!(store.latest().await.expect("latest"), Some(cert.clone()));

        let reopened = CheckpointStore::open(&kvm).await.expect("reopen");
        assert_eq!(
            reopened.get(&digest).await.expect("get"),
            Some(cert.clone()),
            "the record is keyed by the digest and survives a reopen"
        );
        assert_eq!(
            reopened.latest().await.expect("latest"),
            Some(cert),
            "the head survives a reopen"
        );
    }

    /// **The history floor is a durable, read-back fact.** It is what turns "this node has no deploy
    /// row for that id" into "this node does not index that height", so a lost or unread marker
    /// would silently restore the old, misleading answer.
    #[tokio::test]
    async fn the_history_floor_survives_a_reopen_and_is_absent_until_written() {
        let kvm = InMemoryStoreManager::default();
        let store = CheckpointStore::open(&kvm).await.expect("open");
        assert_eq!(store.history_from().await.expect("read"), None);

        store.set_history_from(2_433).await.expect("write");
        assert_eq!(store.history_from().await.expect("read"), Some(2_433));
        let reopened = CheckpointStore::open(&kvm).await.expect("reopen");
        assert_eq!(
            reopened.history_from().await.expect("read"),
            Some(2_433),
            "the floor is in the store, not in the process"
        );
    }

    #[tokio::test]
    async fn a_different_digest_is_not_served_the_head() {
        let kvm = InMemoryStoreManager::default();
        let store = CheckpointStore::open(&kvm).await.expect("open");
        let digest = Blake2b256Hash::from_byte_array(&[0x11; 32]);
        store.put(&digest, &certificate(7)).await.expect("put");
        assert_eq!(
            store
                .get(&Blake2b256Hash::from_byte_array(&[0x22; 32]))
                .await
                .expect("get"),
            None,
            "a lookup names the exact statement, not whatever is latest"
        );
    }
}
