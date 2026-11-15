//! **The checkpoint-certificate store** (sync/verification separation).
//!
//! A joiner no longer replays history to establish the finalized state: it verifies a certificate —
//! a checkpoint statement plus a supermajority of validator signatures — and starts from the state
//! the statement names. This store keeps those certificates so the node can serve them to joiners
//! (`GET /api/v1/checkpoint` and `/{digest}`) and re-verify its own acceptance after a restart.
//!
//! Three named stores, deliberately separate from [`crate::approved_store`]:
//!
//! * `checkpoint-certificates` is keyed by the **checkpoint digest**, so a lookup is a lookup of
//!   the exact statement signed; two statements cannot collide under one row.
//! * `checkpoint-anchors` maps an **anchor block hash** to the digest of the statement recorded for
//!   it. It is the conflict index: two different statements at one anchor are two records, and
//!   keeping the first mapping is what makes the second visible instead of silently replacing it.
//! * `checkpoint-head` holds the latest certificate this node **adopted**, under a single key. It is
//!   the serving pointer, not a statement: the digest-keyed row remains the record.
//!
//! **Recording and adopting are separate on purpose.** A record is evidence — the digest row and
//! the anchor index — and is written before a joiner walks the state, so a crash after hydration
//! still leaves the statement it accepted. Adoption moves the served head, never backwards, never
//! across shards, and never from one statement to another at the same anchor. The digest is
//! computed by `rchain-casper::checkpoint` (this crate has no hashing/crypto policy), so callers
//! pass it in and this store only persists what it is given.

use std::sync::Arc;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::casper::protocol::checkpoint::CheckpointCertificate;
use rchain_shared::base16;
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

/// What a [`CheckpointStore::record`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recorded {
    /// The digest row and the anchor index row are new.
    Fresh,
    /// The digest row was already present and the anchor index agrees.
    Again,
    /// **Another statement is already recorded at this anchor.** Both digest rows are kept; the
    /// anchor row is not overwritten, so the first statement remains the anchor's referent. Two
    /// quorate statements at one anchor share a signer (Law 104), so this is the store's evidence of
    /// an equivocator, not a routine replacement.
    Conflict { existing_digest: Blake2b256Hash },
}

/// What a [`CheckpointStore::adopt`] did to the served head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Adoption {
    /// The head now names this certificate.
    Adopted,
    /// The head already named it.
    AlreadyHead,
    /// A different statement at the same anchor. The head does not move; both records stay.
    Conflict {
        head_digest: Option<Blake2b256Hash>,
        incoming_digest: Blake2b256Hash,
    },
    /// A different anchor at the same height: normal sibling finality movement. The head moves
    /// only when the caller has tied the certificate to its accepted fringe (`allow_sibling`).
    Sibling {
        head_digest: Option<Blake2b256Hash>,
        head_anchor_height: i64,
    },
    /// An older anchor. The head never moves backwards.
    Stale { head_anchor_height: i64 },
    /// The certificate names another shard. This store serves one shard's head.
    ForeignShard {
        head_shard: String,
        incoming_shard: String,
    },
}

type DigestRow = Arc<dyn KeyValueTypedStore<Blake2b256Hash, CheckpointCertificate>>;
type HeadRow = Arc<dyn KeyValueTypedStore<u8, CheckpointCertificate>>;
type HistoryRow = Arc<dyn KeyValueTypedStore<u8, i64>>;
type AnchorRow = Arc<dyn KeyValueTypedStore<Blake2b256Hash, Blake2b256Hash>>;

/// The certificate store: digest-keyed records, the anchor conflict index, and the adopted head.
#[derive(Clone)]
pub struct CheckpointStore {
    certificates: DigestRow,
    head: HeadRow,
    history: HistoryRow,
    anchors: AnchorRow,
}

impl CheckpointStore {
    /// Open (or create) the named stores over the same manager.
    pub async fn open(kvm: &dyn KeyValueStoreManager) -> Result<Self, String> {
        let certificates = kvm.store("checkpoint-certificates").await?;
        let head = kvm.store("checkpoint-head").await?;
        let history = kvm.store("checkpoint-history").await?;
        let anchors = kvm.store("checkpoint-anchors").await?;
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
            anchors: Arc::new(KeyValueTypedStoreCodec::new(
                anchors,
                Arc::new(Blake2b256HashCodec),
                Arc::new(Blake2b256HashCodec),
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

    /// **Persist the digest-keyed record and the anchor index.** This is evidence, not adoption:
    /// the head does not move here. A conflicting statement at the same anchor is recorded as its
    /// own digest row and reported, but the anchor index keeps the first statement — the second
    /// cannot silently take its place.
    pub async fn record(
        &self,
        digest: &Blake2b256Hash,
        certificate: &CheckpointCertificate,
    ) -> Result<Recorded, String> {
        let anchor = anchor_hash(certificate)?;
        self.certificates
            .put(&[(*digest, certificate.clone())])
            .await?;
        if self.certificates.get(&[*digest]).await? != vec![Some(certificate.clone())] {
            return Err("checkpoint certificate read-back mismatch".to_string());
        }

        match self
            .anchors
            .get(&[anchor])
            .await?
            .into_iter()
            .next()
            .flatten()
        {
            None => {
                self.anchors.put(&[(anchor, *digest)]).await?;
                if self.anchors.get(&[anchor]).await? != vec![Some(*digest)] {
                    return Err("checkpoint anchor read-back mismatch".to_string());
                }
                Ok(Recorded::Fresh)
            }
            Some(existing) if existing == *digest => Ok(Recorded::Again),
            Some(existing) => Ok(Recorded::Conflict {
                existing_digest: existing,
            }),
        }
    }

    /// **Move the served head, monotonically and without crossing a statement boundary.**
    ///
    /// * a newer anchor adopts;
    /// * a different anchor at the same height is a sibling — adopted only when the caller has
    ///   tied it to the accepted fringe (`allow_sibling`);
    /// * an older anchor is stale;
    /// * two statements at the **same anchor** are a conflict, and the head does not move.
    pub async fn adopt(
        &self,
        digest: &Blake2b256Hash,
        certificate: &CheckpointCertificate,
        allow_sibling: bool,
    ) -> Result<Adoption, String> {
        let incoming_anchor = anchor_hash(certificate)?;
        let incoming_height = certificate.checkpoint.anchor_height;
        let Some(head) = self.latest().await? else {
            self.write_head(certificate).await?;
            return Ok(Adoption::Adopted);
        };
        if head.checkpoint.shard_id != certificate.checkpoint.shard_id {
            return Ok(Adoption::ForeignShard {
                head_shard: head.checkpoint.shard_id,
                incoming_shard: certificate.checkpoint.shard_id.clone(),
            });
        }
        let head_anchor = anchor_hash(&head)?;
        let head_digest = self
            .anchors
            .get(&[head_anchor])
            .await?
            .into_iter()
            .next()
            .flatten();
        let head_height = head.checkpoint.anchor_height;

        if head_anchor == incoming_anchor {
            if head.checkpoint == certificate.checkpoint {
                return Ok(Adoption::AlreadyHead);
            }
            return Ok(Adoption::Conflict {
                head_digest,
                incoming_digest: *digest,
            });
        }
        if incoming_height > head_height || (allow_sibling && incoming_height == head_height) {
            self.write_head(certificate).await?;
            return Ok(Adoption::Adopted);
        }
        if incoming_height == head_height {
            return Ok(Adoption::Sibling {
                head_digest,
                head_anchor_height: head_height,
            });
        }
        Ok(Adoption::Stale {
            head_anchor_height: head_height,
        })
    }

    /// The digest recorded for an anchor block, if this node holds one.
    pub async fn find_by_anchor(
        &self,
        anchor: &Blake2b256Hash,
    ) -> Result<Option<Blake2b256Hash>, String> {
        Ok(self
            .anchors
            .get(&[*anchor])
            .await?
            .into_iter()
            .next()
            .flatten())
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

    /// The latest certificate this node adopted.
    pub async fn latest(&self) -> Result<Option<CheckpointCertificate>, String> {
        Ok(self
            .head
            .get(&[CHECKPOINT_HEAD_KEY])
            .await?
            .into_iter()
            .next()
            .flatten())
    }

    async fn write_head(&self, certificate: &CheckpointCertificate) -> Result<(), String> {
        self.head
            .put(&[(CHECKPOINT_HEAD_KEY, certificate.clone())])
            .await?;
        if self.head.get(&[CHECKPOINT_HEAD_KEY]).await? != vec![Some(certificate.clone())] {
            return Err("checkpoint head read-back mismatch".to_string());
        }
        Ok(())
    }
}

fn anchor_hash(certificate: &CheckpointCertificate) -> Result<Blake2b256Hash, String> {
    let bytes = base16::decode(&certificate.checkpoint.anchor)
        .ok_or_else(|| "checkpoint anchor is not base16".to_string())?;
    if bytes.len() != 32 {
        return Err(format!(
            "checkpoint anchor is {} bytes, not 32",
            bytes.len()
        ));
    }
    Ok(Blake2b256Hash::from_byte_array(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::casper::protocol::checkpoint::{
        Checkpoint, CheckpointAttestation, CheckpointCertificate,
    };
    use rchain_shared::store_manager::InMemoryStoreManager;

    fn certificate(anchor_byte: u8, post_byte: u8) -> CheckpointCertificate {
        let anchor = base16::encode(&[anchor_byte; 32]);
        CheckpointCertificate {
            checkpoint: Checkpoint {
                shard_id: "root".to_string(),
                anchor: anchor.clone(),
                anchor_height: i64::from(anchor_byte),
                anchor_post_state: base16::encode(&[post_byte; 32]),
                fringe_hashes: vec![anchor],
                parent_digest: None,
                epoch: 1,
                metadata_root: None,
                history_root: None,
            },
            attestations: vec![CheckpointAttestation {
                checkpoint_digest: "ef".repeat(32),
                sender: "01".repeat(65),
                sig_algorithm: "secp256k1".to_string(),
                sig: "02".repeat(64),
            }],
        }
    }

    fn digest(byte: u8) -> Blake2b256Hash {
        Blake2b256Hash::from_byte_array(&[byte; 32])
    }

    fn anchor_of(certificate: &CheckpointCertificate) -> Blake2b256Hash {
        anchor_hash(certificate).expect("anchor")
    }

    #[tokio::test]
    async fn a_record_is_found_by_its_digest_anchor_and_as_the_head_after_a_reopen() {
        let kvm = InMemoryStoreManager::default();
        let store = CheckpointStore::open(&kvm).await.expect("open");
        assert!(store.latest().await.expect("latest").is_none());

        let cert = certificate(7, 0xcd);
        let digest = digest(0x11);
        assert_eq!(
            store.record(&digest, &cert).await.expect("record"),
            Recorded::Fresh
        );
        assert_eq!(
            store.adopt(&digest, &cert, false).await.expect("adopt"),
            Adoption::Adopted
        );
        assert_eq!(store.get(&digest).await.expect("get"), Some(cert.clone()));
        assert_eq!(store.latest().await.expect("latest"), Some(cert.clone()));
        assert_eq!(
            store
                .find_by_anchor(&anchor_of(&cert))
                .await
                .expect("anchor"),
            Some(digest)
        );

        let reopened = CheckpointStore::open(&kvm).await.expect("reopen");
        assert_eq!(
            reopened.get(&digest).await.expect("get"),
            Some(cert.clone()),
            "the record is keyed by the digest and survives a reopen"
        );
        assert_eq!(
            reopened.latest().await.expect("latest"),
            Some(cert.clone()),
            "the head survives a reopen"
        );
        assert_eq!(
            reopened
                .find_by_anchor(&anchor_of(&cert))
                .await
                .expect("anchor"),
            Some(digest),
            "the anchor index survives a reopen"
        );
    }

    #[tokio::test]
    async fn a_newer_anchor_is_adopted_and_an_older_one_never_moves_the_head_back() {
        let kvm = InMemoryStoreManager::default();
        let store = CheckpointStore::open(&kvm).await.expect("open");
        let older = certificate(7, 0x0a);
        let newer = certificate(8, 0x0b);
        store
            .record(&digest(1), &older)
            .await
            .expect("record older");
        store
            .adopt(&digest(1), &older, false)
            .await
            .expect("adopt older");
        store
            .record(&digest(2), &newer)
            .await
            .expect("record newer");
        assert_eq!(
            store.adopt(&digest(2), &newer, false).await.expect("adopt"),
            Adoption::Adopted
        );
        assert_eq!(store.latest().await.expect("latest"), Some(newer));
        assert_eq!(
            store.adopt(&digest(1), &older, false).await.expect("stale"),
            Adoption::Stale {
                head_anchor_height: 8
            }
        );
    }

    #[tokio::test]
    async fn a_sibling_at_the_same_height_moves_the_head_only_when_tied() {
        let kvm = InMemoryStoreManager::default();
        let store = CheckpointStore::open(&kvm).await.expect("open");
        let a = certificate(7, 0x0a);
        // A different anchor at the same height: the helper ties height to the anchor byte, so
        // move the height back after building the second statement.
        let mut b = certificate(8, 0x0b);
        b.checkpoint.anchor_height = 7;
        store.record(&digest(1), &a).await.expect("record a");
        store.adopt(&digest(1), &a, false).await.expect("adopt a");
        store.record(&digest(2), &b).await.expect("record b");
        assert_eq!(
            store.adopt(&digest(2), &b, false).await.expect("sibling"),
            Adoption::Sibling {
                head_digest: Some(digest(1)),
                head_anchor_height: 7
            }
        );
        assert_eq!(store.latest().await.expect("latest"), Some(a.clone()));
        assert_eq!(
            store.adopt(&digest(2), &b, true).await.expect("tied"),
            Adoption::Adopted
        );
        assert_eq!(store.latest().await.expect("latest"), Some(b));
    }

    #[tokio::test]
    async fn two_statements_at_one_anchor_are_a_conflict_and_the_first_keeps_the_head() {
        let kvm = InMemoryStoreManager::default();
        let store = CheckpointStore::open(&kvm).await.expect("open");
        let a = certificate(7, 0x0a);
        let b = certificate(7, 0x0b);
        store.record(&digest(1), &a).await.expect("record a");
        store.adopt(&digest(1), &a, false).await.expect("adopt a");
        assert_eq!(
            store.record(&digest(2), &b).await.expect("record b"),
            Recorded::Conflict {
                existing_digest: digest(1)
            }
        );
        assert_eq!(
            store.adopt(&digest(2), &b, false).await.expect("conflict"),
            Adoption::Conflict {
                head_digest: Some(digest(1)),
                incoming_digest: digest(2)
            }
        );
        assert_eq!(store.latest().await.expect("latest"), Some(a));
        assert!(
            store.get(&digest(2)).await.expect("get b").is_some(),
            "the conflicting statement stays as evidence"
        );
        assert_eq!(
            store.find_by_anchor(&anchor_of(&b)).await.expect("anchor"),
            Some(digest(1)),
            "the anchor index keeps the first statement"
        );
    }

    #[tokio::test]
    async fn a_foreign_shard_is_refused_and_the_head_stays() {
        let kvm = InMemoryStoreManager::default();
        let store = CheckpointStore::open(&kvm).await.expect("open");
        let a = certificate(7, 0x0a);
        store.record(&digest(1), &a).await.expect("record a");
        store.adopt(&digest(1), &a, false).await.expect("adopt a");
        let mut other = certificate(8, 0x0b);
        other.checkpoint.shard_id = "other".to_string();
        store.record(&digest(2), &other).await.expect("record");
        assert_eq!(
            store.adopt(&digest(2), &other, false).await.expect("adopt"),
            Adoption::ForeignShard {
                head_shard: "root".to_string(),
                incoming_shard: "other".to_string()
            }
        );
        assert_eq!(store.latest().await.expect("latest"), Some(a));
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
        let cert = certificate(7, 0x0a);
        let recorded_digest = digest(0x11);
        store.record(&recorded_digest, &cert).await.expect("record");
        store
            .adopt(&recorded_digest, &cert, false)
            .await
            .expect("adopt");
        assert_eq!(
            store.get(&digest(0x22)).await.expect("get"),
            None,
            "a lookup names the exact statement, not whatever is latest"
        );
    }
}
