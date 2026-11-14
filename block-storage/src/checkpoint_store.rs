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

/// JSON codec for a certificate. The wire type is already the signed statement's serialization;
/// persisting it as JSON keeps the store readable by an operator and avoids a second protobuf
/// schema whose only reader would be this store.
#[derive(Default)]
pub struct CheckpointCertificateCodec;

impl Codec<CheckpointCertificate> for CheckpointCertificateCodec {
    fn encode(&self, value: &CheckpointCertificate) -> Vec<u8> {
        serde_json::to_vec(value).expect("a checkpoint certificate serializes")
    }

    fn decode(&self, bytes: &[u8]) -> Result<CheckpointCertificate, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("checkpoint certificate decode: {e}"))
    }
}

type DigestRow = Arc<dyn KeyValueTypedStore<Blake2b256Hash, CheckpointCertificate>>;
type HeadRow = Arc<dyn KeyValueTypedStore<u8, CheckpointCertificate>>;

/// The certificate store: a digest-keyed record plus the latest-accepted pointer.
#[derive(Clone)]
pub struct CheckpointStore {
    certificates: DigestRow,
    head: HeadRow,
}

impl CheckpointStore {
    /// Open (or create) both named stores over the same manager.
    pub async fn open(kvm: &dyn KeyValueStoreManager) -> Result<Self, String> {
        let certificates = kvm.store("checkpoint-certificates").await?;
        let head = kvm.store("checkpoint-head").await?;
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
        })
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
