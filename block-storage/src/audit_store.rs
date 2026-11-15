//! **The accepted audit claims and their chains** (Law 112).
//!
//! A node stores only claims it reproduced: the acceptance decision is
//! `casper::audit::verify_audit_attestation` plus the node's own replay, and this store is the
//! durable side. It keeps four things:
//!
//! * `audit-attestations` — one row per accepted claim, keyed by the claim's digest;
//! * `audit-heads` — one head per signer (`sequence`, digest), so the next claim must extend it;
//! * `audit-nonces` — `signer:nonce` once, so a request cannot be answered twice;
//! * `audit-conflicts` — claims that were shown false or unchained, with the local reason.
//!
//! The store does no cryptography and no replay; it persists what the verifier accepted and
//! refuses nothing on its own. A caller that skipped the verifier would be writing an unverified
//! claim — the type system cannot stop a same-crate caller, so the store's contract is stated here
//! and its only production caller is `node::runtime::audit_attestation`.

use std::sync::Arc;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::casper::protocol::audit::AuditAttestation;
use rchain_shared::store_manager::KeyValueStoreManager;
use rchain_shared::typed_store::{Codec, KeyValueTypedStore, KeyValueTypedStoreCodec, StringCodec};
use serde::{Deserialize, Serialize};

use crate::dag::codecs::Blake2b256HashCodec;

/// The accepted head of one signer's chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditHead {
    pub sequence: u64,
    pub digest: String,
}

/// A claim a node refused, kept as attributable evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditConflict {
    pub attestation_digest: String,
    pub signer: String,
    pub reason: String,
    pub recorded_at_unix_ms: u64,
}

#[derive(Default)]
struct AuditAttestationCodec;

impl Codec<AuditAttestation> for AuditAttestationCodec {
    fn encode(&self, value: &AuditAttestation) -> Vec<u8> {
        serde_json::to_vec(value).unwrap_or_default()
    }

    fn decode(&self, bytes: &[u8]) -> Result<AuditAttestation, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("audit attestation decode: {e}"))
    }
}

#[derive(Default)]
struct AuditHeadCodec;

impl Codec<AuditHead> for AuditHeadCodec {
    fn encode(&self, value: &AuditHead) -> Vec<u8> {
        serde_json::to_vec(value).unwrap_or_default()
    }

    fn decode(&self, bytes: &[u8]) -> Result<AuditHead, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("audit head decode: {e}"))
    }
}

#[derive(Default)]
struct AuditConflictCodec;

impl Codec<AuditConflict> for AuditConflictCodec {
    fn encode(&self, value: &AuditConflict) -> Vec<u8> {
        serde_json::to_vec(value).unwrap_or_default()
    }

    fn decode(&self, bytes: &[u8]) -> Result<AuditConflict, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("audit conflict decode: {e}"))
    }
}

type AttestationRow = Arc<dyn KeyValueTypedStore<Blake2b256Hash, AuditAttestation>>;
type HeadRow = Arc<dyn KeyValueTypedStore<String, AuditHead>>;
type NonceRow = Arc<dyn KeyValueTypedStore<String, String>>;
type ConflictRow = Arc<dyn KeyValueTypedStore<String, AuditConflict>>;

/// The audit store.
#[derive(Clone)]
pub struct AuditStore {
    attestations: AttestationRow,
    heads: HeadRow,
    nonces: NonceRow,
    conflicts: ConflictRow,
}

impl AuditStore {
    pub async fn open(kvm: &dyn KeyValueStoreManager) -> Result<Self, String> {
        let attestations = kvm.store("audit-attestations").await?;
        let heads = kvm.store("audit-heads").await?;
        let nonces = kvm.store("audit-nonces").await?;
        let conflicts = kvm.store("audit-conflicts").await?;
        Ok(AuditStore {
            attestations: Arc::new(KeyValueTypedStoreCodec::new(
                attestations,
                Arc::new(Blake2b256HashCodec),
                Arc::new(AuditAttestationCodec),
            )),
            heads: Arc::new(KeyValueTypedStoreCodec::new(
                heads,
                Arc::new(StringCodec),
                Arc::new(AuditHeadCodec),
            )),
            nonces: Arc::new(KeyValueTypedStoreCodec::new(
                nonces,
                Arc::new(StringCodec),
                Arc::new(StringCodec),
            )),
            conflicts: Arc::new(KeyValueTypedStoreCodec::new(
                conflicts,
                Arc::new(StringCodec),
                Arc::new(AuditConflictCodec),
            )),
        })
    }

    /// The accepted head for a signer, or `None` before its first accepted claim.
    pub async fn head(&self, signer: &str) -> Result<Option<AuditHead>, String> {
        Ok(self
            .heads
            .get(&[signer.to_string()])
            .await?
            .into_iter()
            .next()
            .flatten())
    }

    /// **Persist an accepted claim**: its digest row, its signer's head, and its nonce. The caller
    /// has already verified it (see the module contract); the writes are read back.
    pub async fn accept(
        &self,
        digest: &Blake2b256Hash,
        attestation: &AuditAttestation,
    ) -> Result<(), String> {
        self.attestations
            .put(&[(*digest, attestation.clone())])
            .await?;
        let head = AuditHead {
            sequence: attestation.sequence,
            digest: digest.to_hex(),
        };
        self.heads
            .put(&[(attestation.signer.clone(), head.clone())])
            .await?;
        self.nonces
            .put(&[(
                nonce_key(&attestation.signer, &attestation.nonce),
                digest.to_hex(),
            )])
            .await?;

        let read_back = self
            .attestations
            .get(&[*digest])
            .await?
            .into_iter()
            .next()
            .flatten();
        if read_back.as_ref() != Some(attestation) {
            return Err("audit attestation read-back mismatch".to_string());
        }
        if self.head(&attestation.signer).await? != Some(head) {
            return Err("audit head read-back mismatch".to_string());
        }
        Ok(())
    }

    /// A stored claim by digest.
    pub async fn get(&self, digest: &Blake2b256Hash) -> Result<Option<AuditAttestation>, String> {
        Ok(self
            .attestations
            .get(&[*digest])
            .await?
            .into_iter()
            .next()
            .flatten())
    }

    /// Every accepted claim this node holds.
    pub async fn accepted(&self) -> Result<Vec<AuditAttestation>, String> {
        Ok(self.attestations.to_map().await?.into_values().collect())
    }

    /// Whether this signer's nonce was already answered.
    pub async fn nonce_seen(&self, signer: &str, nonce: &str) -> Result<bool, String> {
        Ok(self
            .nonces
            .get(&[nonce_key(signer, nonce)])
            .await?
            .into_iter()
            .next()
            .flatten()
            .is_some())
    }

    /// **Record a refused claim as attributable evidence.** A conflict is not an accepted row: it
    /// never enters `accepted`.
    pub async fn record_conflict(&self, conflict: &AuditConflict) -> Result<(), String> {
        self.conflicts
            .put(&[(conflict.attestation_digest.clone(), conflict.clone())])
            .await?;
        if self
            .conflicts
            .get(&[conflict.attestation_digest.clone()])
            .await?
            .into_iter()
            .next()
            .flatten()
            .as_ref()
            != Some(conflict)
        {
            return Err("audit conflict read-back mismatch".to_string());
        }
        Ok(())
    }

    /// Every refused claim this node recorded.
    pub async fn conflicts(&self) -> Result<Vec<AuditConflict>, String> {
        Ok(self.conflicts.to_map().await?.into_values().collect())
    }
}

fn nonce_key(signer: &str, nonce: &str) -> String {
    format!("{signer}:{nonce}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::casper::protocol::audit::SIGNED_HASH_CHAIN_WITNESS;
    use rchain_shared::store_manager::InMemoryStoreManager;

    fn attestation(sequence: u64, nonce: &str) -> AuditAttestation {
        AuditAttestation {
            witness_type: SIGNED_HASH_CHAIN_WITNESS.to_string(),
            shard_id: "root".to_string(),
            checkpoint_digest: "11".repeat(32),
            anchor: "22".repeat(32),
            anchor_height: 7,
            from_height: 0,
            to_height: 7,
            metadata_root: "33".repeat(32),
            history_root: "44".repeat(32),
            derived_root: "55".repeat(32),
            replay_head: "66".repeat(32),
            previous: None,
            sequence,
            nonce: nonce.to_string(),
            signer: "07".repeat(33),
            sig_algorithm: "secp256k1".to_string(),
            sig: "88".repeat(64),
        }
    }

    /// An accepted claim is found by digest, becomes its signer's head, spends its nonce, and all
    /// of that survives a reopen of the same store.
    #[tokio::test]
    async fn the_attestation_store_survives_a_reopen() {
        let kvm = InMemoryStoreManager::default();
        let store = AuditStore::open(&kvm).await.expect("open");
        let claim = attestation(0, "nonce-1");
        let digest = Blake2b256Hash::from_byte_array(&[9u8; 32]);
        store.accept(&digest, &claim).await.expect("accept");

        assert_eq!(store.get(&digest).await.expect("get"), Some(claim.clone()));
        assert_eq!(
            store.head(&claim.signer).await.expect("head"),
            Some(AuditHead {
                sequence: 0,
                digest: digest.to_hex()
            })
        );
        assert!(store
            .nonce_seen(&claim.signer, "nonce-1")
            .await
            .expect("nonce"));
        assert!(!store
            .nonce_seen(&claim.signer, "nonce-2")
            .await
            .expect("nonce"));
        assert_eq!(store.accepted().await.expect("accepted").len(), 1);

        let reopened = AuditStore::open(&kvm).await.expect("reopen");
        assert_eq!(
            reopened.head(&claim.signer).await.expect("head"),
            Some(AuditHead {
                sequence: 0,
                digest: digest.to_hex()
            })
        );
        assert_eq!(reopened.accepted().await.expect("accepted"), vec![claim]);
    }

    /// A conflict is durable evidence and is not an accepted claim.
    #[tokio::test]
    async fn a_conflict_is_recorded_and_not_accepted() {
        let kvm = InMemoryStoreManager::default();
        let store = AuditStore::open(&kvm).await.expect("open");
        let conflict = AuditConflict {
            attestation_digest: "ab".repeat(32),
            signer: "07".repeat(33),
            reason: "the claimed derived root is not the reproduced root".to_string(),
            recorded_at_unix_ms: 1_700_000_000_000,
        };
        store.record_conflict(&conflict).await.expect("record");
        let reopened = AuditStore::open(&kvm).await.expect("reopen");
        assert_eq!(
            reopened.conflicts().await.expect("conflicts"),
            vec![conflict]
        );
        assert!(reopened.accepted().await.expect("accepted").is_empty());
    }
}
