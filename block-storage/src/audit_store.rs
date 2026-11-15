//! **The accepted audit claims, their chains, and their answers** (Law 112).
//!
//! A node stores only claims it reproduced: the acceptance decision is
//! `casper::audit::verify_audit_attestation` plus the node's own replay, and this store is the
//! durable side. It keeps, under **one** store so a claim and its indexes commit together:
//!
//! * `attestation:<digest>` — one row per accepted claim;
//! * `head:<signer>` — one head per signer (`sequence`, digest), so the next claim must extend it;
//! * `nonce:<signer>:<nonce>` — the spent nonce, mapped to the claim digest;
//! * `response:<signer>:<nonce>` — the answer for any completed audit verdict, including an
//!   unsigned `DISAGREE` or `INCOMPLETE`, so a retry cannot silently re-execute another result.
//!
//! **One `put` batch is one LMDB write transaction**, so the four rows cannot be observed
//! half-written: a crash leaves either the whole claim or none of it. A separate
//! `audit-conflicts` store keeps refused claims, one row per refusal; a conflict is not an accepted
//! row and never enters `accepted`.
//!
//! The store does no cryptography and no replay; it persists what the verifier accepted and
//! refuses nothing on its own except the two index rules that must not race: a nonce is spent once,
//! and a head only moves to a strictly greater sequence. Both read-then-write decisions are held
//! under one in-process gate, so two concurrent acceptances cannot both pass. One node per store is
//! the documented assumption; cross-process writers are outside the gate.

use std::sync::Arc;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::casper::protocol::audit::AuditAttestation;
use rchain_shared::store_manager::KeyValueStoreManager;
use rchain_shared::typed_store::{Codec, KeyValueTypedStore, KeyValueTypedStoreCodec, StringCodec};
use serde::{Deserialize, Serialize};

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

/// **The answer this node gave for a request nonce.** Signed AGREE claims and unsigned
/// DISAGREE/INCOMPLETE reports share this wire shape; old signed rows remain decodable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditResponse {
    /// The reproduced report, as the node served it.
    pub report: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<AuditAttestation>,
}

/// One row of the claims store. The key prefix says which arm it is, and the arm is checked on
/// decode, so a row cannot be read as a different kind of fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "row", rename_all = "camelCase")]
enum ClaimRow {
    Attestation { attestation: AuditAttestation },
    Head { head: AuditHead },
    Nonce { digest: String },
    /// A completed replay with no signed claim still consumes its nonce.
    UnattestedNonce,
    Response { response: AuditResponse },
}

#[derive(Default)]
struct ClaimRowCodec;

impl Codec<ClaimRow> for ClaimRowCodec {
    fn encode(&self, value: &ClaimRow) -> Vec<u8> {
        serde_json::to_vec(value).unwrap_or_default()
    }

    fn decode(&self, bytes: &[u8]) -> Result<ClaimRow, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("audit claim row decode: {e}"))
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

type ClaimStore = Arc<dyn KeyValueTypedStore<String, ClaimRow>>;
type ConflictRow = Arc<dyn KeyValueTypedStore<String, AuditConflict>>;

/// The audit store.
#[derive(Clone)]
pub struct AuditStore {
    claims: ClaimStore,
    conflicts: ConflictRow,
    /// The read-then-write decisions (`head`, `nonce`) are one decision.
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl AuditStore {
    pub async fn open(kvm: &dyn KeyValueStoreManager) -> Result<Self, String> {
        let claims = kvm.store("audit-claims").await?;
        let conflicts = kvm.store("audit-conflicts").await?;
        Ok(AuditStore {
            claims: Arc::new(KeyValueTypedStoreCodec::new(
                claims,
                Arc::new(StringCodec),
                Arc::new(ClaimRowCodec),
            )),
            conflicts: Arc::new(KeyValueTypedStoreCodec::new(
                conflicts,
                Arc::new(StringCodec),
                Arc::new(AuditConflictCodec),
            )),
            gate: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// The accepted head for a signer, or `None` before its first accepted claim.
    pub async fn head(&self, signer: &str) -> Result<Option<AuditHead>, String> {
        match self.get_row(&head_key(signer)).await? {
            Some(ClaimRow::Head { head }) => Ok(Some(head)),
            Some(_) => Err(format!(
                "the audit store's head row for signer {signer} is another kind of row"
            )),
            None => Ok(None),
        }
    }

    /// **Persist an accepted claim and the answer it produced, atomically.**
    ///
    /// The caller has already verified the claim (see the module contract). One `put` batch writes
    /// the attestation, the signer's new head, the spent nonce and the answer; a failure at the
    /// store leaves none of them, and the rows are read back before success is reported. A nonce
    /// already spent or a sequence that does not advance the head is refused rather than
    /// overwritten.
    pub async fn accept(
        &self,
        digest: &Blake2b256Hash,
        attestation: &AuditAttestation,
        response: Option<&AuditResponse>,
    ) -> Result<(), String> {
        let _gate = self.gate.lock().await;
        let digest_hex = digest.to_hex();
        let nonce_key = nonce_key(&attestation.signer, &attestation.nonce);
        if self.get_row(&nonce_key).await?.is_some() {
            return Err(format!(
                "the request nonce {:?} for signer {} has already been answered",
                attestation.nonce, attestation.signer
            ));
        }
        let head = AuditHead {
            sequence: attestation.sequence,
            digest: digest_hex.clone(),
        };
        if let Some(previous) = self.head(&attestation.signer).await? {
            if attestation.sequence <= previous.sequence {
                return Err(format!(
                    "the claim's sequence {} does not advance signer {}'s accepted head {}",
                    attestation.sequence, attestation.signer, previous.sequence
                ));
            }
        }
        let mut rows = vec![
            (
                attestation_key(&digest_hex),
                ClaimRow::Attestation {
                    attestation: attestation.clone(),
                },
            ),
            (
                head_key(&attestation.signer),
                ClaimRow::Head { head: head.clone() },
            ),
            (
                nonce_key.clone(),
                ClaimRow::Nonce {
                    digest: digest_hex.clone(),
                },
            ),
        ];
        if let Some(response) = response {
            rows.push((
                response_key(&attestation.signer, &attestation.nonce),
                ClaimRow::Response {
                    response: response.clone(),
                },
            ));
        }
        // **The commit point.** Every row above is in this one batch, so a crash cannot leave a
        // visible claim with an outdated head or an unconsumed nonce.
        self.claims.put(&rows).await?;

        if self.get(&digest_hex).await?.as_ref() != Some(attestation) {
            return Err("audit attestation read-back mismatch".to_string());
        }
        if self.head(&attestation.signer).await? != Some(head) {
            return Err("audit head read-back mismatch".to_string());
        }
        if !self
            .nonce_seen(&attestation.signer, &attestation.nonce)
            .await?
        {
            return Err("audit nonce read-back mismatch".to_string());
        }
        if let Some(response) = response {
            if self
                .response(&attestation.signer, &attestation.nonce)
                .await?
                .as_ref()
                != Some(response)
            {
                return Err("audit response read-back mismatch".to_string());
            }
        }
        Ok(())
    }

    /// Persist a completed unsigned audit report without altering the accepted signature chain.
    ///
    /// An INCOMPLETE or DISAGREE is a result, not an accepted attestation. Its spent nonce and
    /// original answer commit in the same batch; a retry after restart must not re-run it.
    pub async fn record_unattested_response(
        &self,
        signer: &str,
        nonce: &str,
        response: &AuditResponse,
    ) -> Result<(), String> {
        if response.attestation.is_some()
            || !matches!(
                response.report.get("verdict").and_then(serde_json::Value::as_str),
                Some("INCOMPLETE" | "DISAGREE")
            )
        {
            return Err(
                "only an unsigned INCOMPLETE or DISAGREE is stored without a claim".to_string(),
            );
        }
        let _gate = self.gate.lock().await;
        let key = nonce_key(signer, nonce);
        if self.get_row(&key).await?.is_some() {
            return Err(format!(
                "the request nonce {nonce:?} for signer {signer} has already been answered"
            ));
        }
        self.claims
            .put(&[
                (key, ClaimRow::UnattestedNonce),
                (
                    response_key(signer, nonce),
                    ClaimRow::Response {
                        response: response.clone(),
                    },
                ),
            ])
            .await?;
        if !self.nonce_seen(signer, nonce).await?
            || self.response(signer, nonce).await?.as_ref() != Some(response)
        {
            return Err("unsigned audit result read-back mismatch".to_string());
        }
        Ok(())
    }

    /// A stored claim by digest.
    pub async fn get(&self, digest: &str) -> Result<Option<AuditAttestation>, String> {
        match self.get_row(&attestation_key(digest)).await? {
            Some(ClaimRow::Attestation { attestation }) => Ok(Some(attestation)),
            Some(_) => Err(format!(
                "the audit store's attestation row for {digest} is another kind of row"
            )),
            None => Ok(None),
        }
    }

    /// The persisted answer for a request nonce, if this node answered it.
    pub async fn response(
        &self,
        signer: &str,
        nonce: &str,
    ) -> Result<Option<AuditResponse>, String> {
        match self.get_row(&response_key(signer, nonce)).await? {
            Some(ClaimRow::Response { response }) => Ok(Some(response)),
            Some(_) => Err(
                "the audit store's response row is another kind of row for this nonce".to_string(),
            ),
            None => Ok(None),
        }
    }

    /// Every accepted claim this node holds.
    pub async fn accepted(&self) -> Result<Vec<AuditAttestation>, String> {
        Ok(self
            .claims
            .to_map()
            .await?
            .into_iter()
            .filter(|(key, _)| key.starts_with("attestation:"))
            .filter_map(|(_, row)| match row {
                ClaimRow::Attestation { attestation } => Some(attestation),
                _ => None,
            })
            .collect())
    }

    /// Whether this signer's nonce was already answered.
    pub async fn nonce_seen(&self, signer: &str, nonce: &str) -> Result<bool, String> {
        Ok(self.get_row(&nonce_key(signer, nonce)).await?.is_some())
    }

    /// **Record a refused claim as attributable evidence.** A conflict is not an accepted row: it
    /// never enters `accepted`, and it is one row in its own store.
    pub async fn record_conflict(&self, conflict: &AuditConflict) -> Result<(), String> {
        self.conflicts
            .put(&[(conflict.attestation_digest.clone(), conflict.clone())])
            .await?;
        if self
            .conflicts
            .get(std::slice::from_ref(&conflict.attestation_digest))
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

    async fn get_row(&self, key: &str) -> Result<Option<ClaimRow>, String> {
        Ok(self
            .claims
            .get(&[key.to_string()])
            .await?
            .into_iter()
            .next()
            .flatten())
    }
}

fn attestation_key(digest: &str) -> String {
    format!("attestation:{digest}")
}

fn head_key(signer: &str) -> String {
    format!("head:{signer}")
}

fn nonce_key(signer: &str, nonce: &str) -> String {
    format!("nonce:{signer}:{nonce}")
}

fn response_key(signer: &str, nonce: &str) -> String {
    format!("response:{signer}:{nonce}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::casper::protocol::audit::SIGNED_HASH_CHAIN_WITNESS;
    use rchain_shared::store::{InMemoryKeyValueStore, KeyValueStore};
    use rchain_shared::store_manager::InMemoryStoreManager;
    use rchain_shared::typed_store::SharedStore;
    use std::sync::atomic::{AtomicBool, Ordering};

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

    fn response(claim: &AuditAttestation) -> AuditResponse {
        AuditResponse {
            report: serde_json::json!({"verdict": "AGREE", "nonce": claim.nonce}),
            attestation: Some(claim.clone()),
        }
    }

    /// An accepted claim is found by digest, becomes its signer's head, spends its nonce, keeps
    /// its answer, and all of that survives a reopen.
    #[tokio::test]
    async fn the_attestation_store_survives_a_reopen() {
        let kvm = InMemoryStoreManager::default();
        let store = AuditStore::open(&kvm).await.expect("open");
        let claim = attestation(0, "nonce-1");
        let answer = response(&claim);
        let digest = Blake2b256Hash::from_byte_array(&[9u8; 32]);
        store
            .accept(&digest, &claim, Some(&answer))
            .await
            .expect("accept");

        assert_eq!(
            store.get(&digest.to_hex()).await.expect("get"),
            Some(claim.clone())
        );
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
        assert_eq!(
            reopened
                .response(&"07".repeat(33), "nonce-1")
                .await
                .expect("response"),
            Some(answer),
            "a retry after a restart reads the answer from the store"
        );
    }

    /// The store refuses a spent nonce and a head that does not advance, so two acceptances cannot
    /// both pass through the read-then-write window.
    #[tokio::test]
    async fn a_spent_nonce_and_a_stale_sequence_are_refused() {
        let kvm = InMemoryStoreManager::default();
        let store = AuditStore::open(&kvm).await.expect("open");
        let first = attestation(0, "nonce-1");
        let digest = Blake2b256Hash::from_byte_array(&[1u8; 32]);
        store.accept(&digest, &first, None).await.expect("first");

        let replayed_nonce = attestation(1, "nonce-1");
        let err = store
            .accept(
                &Blake2b256Hash::from_byte_array(&[2u8; 32]),
                &replayed_nonce,
                None,
            )
            .await
            .expect_err("a spent nonce is refused");
        assert!(err.contains("already been answered"), "{err}");

        let stale = attestation(0, "nonce-2");
        let err = store
            .accept(&Blake2b256Hash::from_byte_array(&[3u8; 32]), &stale, None)
            .await
            .expect_err("a sequence that does not advance the head is refused");
        assert!(err.contains("does not advance"), "{err}");
        assert_eq!(store.accepted().await.expect("accepted").len(), 1);
    }

    /// **A failed batch leaves no row at all** — the store's one commit point. The failing store
    /// rejects the `put` before applying it, which is the strongest fault this interface exposes;
    /// after the fault is cleared the same acceptance succeeds.
    #[tokio::test]
    async fn a_failed_batch_writes_none_of_the_claim() {
        struct FailingStore {
            inner: InMemoryKeyValueStore,
            fail: Arc<AtomicBool>,
        }
        impl KeyValueStore for FailingStore {
            fn get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>, String> {
                self.inner.get(keys)
            }
            fn put(&mut self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<(), String> {
                if self.fail.load(Ordering::SeqCst) {
                    return Err("injected write failure".to_string());
                }
                self.inner.put(pairs)
            }
            fn delete(&mut self, keys: &[Vec<u8>]) -> Result<usize, String> {
                self.inner.delete(keys)
            }
            fn entries(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
                self.inner.entries()
            }
        }
        struct FailingManager {
            store: SharedStore,
        }
        #[async_trait::async_trait]
        impl KeyValueStoreManager for FailingManager {
            async fn store(&self, _name: &str) -> Result<SharedStore, String> {
                Ok(self.store.clone())
            }
            async fn shutdown(&self) {}
        }
        let fail = Arc::new(AtomicBool::new(true));
        let kvm = FailingManager {
            store: Arc::new(tokio::sync::Mutex::new(Box::new(FailingStore {
                inner: InMemoryKeyValueStore::default(),
                fail: fail.clone(),
            }))),
        };
        let store = AuditStore::open(&kvm).await.expect("open");
        let claim = attestation(0, "nonce-1");
        let answer = response(&claim);
        let digest = Blake2b256Hash::from_byte_array(&[4u8; 32]);
        let err = store
            .accept(&digest, &claim, Some(&answer))
            .await
            .expect_err("the injected fault is surfaced");
        assert!(err.contains("injected write failure"), "{err}");
        assert!(store.accepted().await.expect("accepted").is_empty());
        assert!(store.head(&claim.signer).await.expect("head").is_none());
        assert!(!store
            .nonce_seen(&claim.signer, "nonce-1")
            .await
            .expect("nonce"));
        assert!(store
            .response(&claim.signer, "nonce-1")
            .await
            .expect("response")
            .is_none());

        fail.store(false, Ordering::SeqCst);
        store
            .accept(&digest, &claim, Some(&answer))
            .await
            .expect("the same acceptance succeeds once the fault is cleared");
        assert_eq!(store.accepted().await.expect("accepted"), vec![claim]);
    }

    /// Unsigned outcomes survive reopen, are idempotent and cannot create a signed-chain head.
    #[tokio::test]
    async fn unsigned_verdicts_are_atomic_durable_and_never_accepted() {
        let kvm = InMemoryStoreManager::default();
        let store = AuditStore::open(&kvm).await.expect("open");
        let signer = "07".repeat(33);
        for (idx, verdict) in ["DISAGREE", "INCOMPLETE"].iter().enumerate() {
            let nonce = format!("unsigned-{idx}");
            let answer = AuditResponse {
                report: serde_json::json!({"verdict": verdict, "reason": "fixture"}),
                attestation: None,
            };
            store
                .record_unattested_response(&signer, &nonce, &answer)
                .await
                .expect("persist");
            let reopened = AuditStore::open(&kvm).await.expect("reopen");
            assert_eq!(
                reopened.response(&signer, &nonce).await.expect("response"),
                Some(answer.clone())
            );
            assert!(reopened.nonce_seen(&signer, &nonce).await.expect("spent nonce"));
            assert!(reopened.head(&signer).await.expect("head").is_none());
            assert!(reopened.accepted().await.expect("claims").is_empty());
            assert!(reopened
                .record_unattested_response(&signer, &nonce, &answer)
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn an_agree_report_cannot_be_stored_as_unsigned() {
        let kvm = InMemoryStoreManager::default();
        let store = AuditStore::open(&kvm).await.expect("open");
        let signer = "07".repeat(33);
        let reply = AuditResponse {
            report: serde_json::json!({"verdict": "AGREE"}),
            attestation: None,
        };
        assert!(store
            .record_unattested_response(&signer, "n", &reply)
            .await
            .is_err());
        assert!(!store.nonce_seen(&signer, "n").await.expect("nonce remains unused"));
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
