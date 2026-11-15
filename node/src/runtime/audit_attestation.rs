//! **The node's acceptance path for audit attestations** (Law 112).
//!
//! A completed audit run is an [`AuditRun`]: the report plus the facts the replay reproduced. An
//! [`AcceptedAudit`] is a claim this node verified against that run **and** its signer's chain;
//! only [`verify_and_accept`] builds one, and only an accepted claim reaches the store's accepted
//! rows. A claim that fails is recorded as attributable conflict evidence instead.

use rchain_block_storage::audit_store::{AuditConflict, AuditHead, AuditStore};
use rchain_casper::audit::{
    verify_audit_attestation, AuditChainHead, AuditExpectation, AuditVerificationError,
    VerifiedAuditAttestation,
};
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::casper::protocol::audit::AuditAttestation;

use crate::runtime::audit::AuditReport;

/// **A completed audit's reproduced facts.** Built only from an `AGREE` report; the report is the
/// local evidence, and its fields are what a claim must match.
#[derive(Clone, Debug)]
pub struct AuditRun {
    report: AuditReport,
}

impl AuditRun {
    /// A run that reached `AGREE` and carries a derived root and replay head. Anything else is not
    /// a reproduced fact and cannot accept a claim.
    pub fn from_report(report: AuditReport) -> Result<Self, String> {
        if report.verdict != "AGREE" {
            return Err(format!(
                "only an AGREE run can accept a claim; this one is {}",
                report.verdict
            ));
        }
        if report.derived_root.is_none() || report.replay_head.is_none() {
            return Err("an AGREE run must carry its derived root and replay head".to_string());
        }
        Ok(AuditRun { report })
    }

    pub fn report(&self) -> &AuditReport {
        &self.report
    }

    fn expectation(&self) -> Result<AuditExpectation, String> {
        Ok(AuditExpectation {
            checkpoint_digest: self.report.checkpoint_digest.clone(),
            anchor: self.report.anchor.clone(),
            anchor_height: self.report.anchor_height,
            metadata_root: self
                .report
                .metadata_root
                .clone()
                .ok_or_else(|| "a legacy certificate has no metadata commitment".to_string())?,
            history_root: self.report.history_root.clone().ok_or_else(|| {
                "a legacy certificate has no validation-window commitment".to_string()
            })?,
            derived_root: self
                .report
                .derived_root
                .clone()
                .ok_or_else(|| "the run has no derived root".to_string())?,
            replay_head: self
                .report
                .replay_head
                .clone()
                .ok_or_else(|| "the run has no replay head".to_string())?,
        })
    }
}

/// **A claim this node reproduced.** Wraps the verifier's value; there is no other constructor.
#[derive(Clone, Debug)]
pub struct AcceptedAudit {
    verified: VerifiedAuditAttestation,
}

impl AcceptedAudit {
    pub fn attestation(&self) -> &AuditAttestation {
        self.verified.attestation()
    }

    pub fn digest(&self) -> &str {
        self.verified.digest()
    }

    pub fn chain_head(&self) -> AuditChainHead {
        self.verified.chain_head()
    }
}

/// Why acceptance failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcceptanceError {
    Verification(AuditVerificationError),
    /// The run and the claim do not form an acceptable pair for another reason (a legacy
    /// certificate, a replayed nonce, an unreadable store).
    Refused(String),
}

impl std::fmt::Display for AcceptanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcceptanceError::Verification(e) => write!(f, "{e}"),
            AcceptanceError::Refused(e) => write!(f, "{e}"),
        }
    }
}

/// **Verify a claim against this node's run and its signer's accepted head, then store it.**
///
/// A mismatched root/head or an unchained claim is recorded as a conflict and never accepted. A
/// replayed nonce is refused before the signature check, because the answer must be single-use.
pub async fn verify_and_accept(
    store: &AuditStore,
    run: &AuditRun,
    attestation: &AuditAttestation,
    now_unix_ms: u64,
) -> Result<AcceptedAudit, AcceptanceError> {
    if store
        .nonce_seen(&attestation.signer, &attestation.nonce)
        .await
        .map_err(AcceptanceError::Refused)?
    {
        return refuse_as_conflict(
            store,
            attestation,
            "the request nonce has already been answered",
            now_unix_ms,
        )
        .await;
    }
    let expectation = run.expectation().map_err(AcceptanceError::Refused)?;
    let head: Option<AuditChainHead> = store
        .head(&attestation.signer)
        .await
        .map_err(AcceptanceError::Refused)?
        .map(|head: AuditHead| AuditChainHead {
            signer: attestation.signer.clone(),
            sequence: head.sequence,
            digest: head.digest,
        });
    let verified = match verify_audit_attestation(attestation, &expectation, head.as_ref()) {
        Ok(verified) => verified,
        Err(error) => {
            return refuse_as_conflict(store, attestation, &error.to_string(), now_unix_ms).await;
        }
    };
    let digest = Blake2b256Hash::from_hex_either(digest_hex(&verified))
        .map_err(|e| AcceptanceError::Refused(format!("the claim digest is not a hash: {e}")))?;
    store
        .accept(&digest, verified.attestation())
        .await
        .map_err(AcceptanceError::Refused)?;
    Ok(AcceptedAudit { verified })
}

fn digest_hex(verified: &VerifiedAuditAttestation) -> &str {
    verified.digest()
}

async fn refuse_as_conflict(
    store: &AuditStore,
    attestation: &AuditAttestation,
    reason: &str,
    now_unix_ms: u64,
) -> Result<AcceptedAudit, AcceptanceError> {
    let digest = attestation
        .digest()
        .map(|digest| rchain_shared::base16::encode(&digest))
        .unwrap_or_else(|_| attestation.nonce.clone());
    let conflict = AuditConflict {
        attestation_digest: digest,
        signer: attestation.signer.clone(),
        reason: reason.to_string(),
        recorded_at_unix_ms: now_unix_ms,
    };
    // Evidence is best-effort: a store that cannot take it must not turn the refusal into an
    // acceptance, and the refusal is what the caller sees.
    let _ = store.record_conflict(&conflict).await;
    Err(AcceptanceError::Refused(reason.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::casper::protocol::audit::SIGNED_HASH_CHAIN_WITNESS;
    use rchain_shared::store_manager::InMemoryStoreManager;

    fn run() -> AuditRun {
        AuditRun::from_report(AuditReport {
            verdict: "AGREE".to_string(),
            checkpoint_digest: "11".repeat(32),
            anchor: "22".repeat(32),
            anchor_height: 7,
            shard_id: "root".to_string(),
            claimed_root: "55".repeat(32),
            metadata_root: Some("33".repeat(32)),
            history_root: Some("44".repeat(32)),
            derived_root: Some("55".repeat(32)),
            replay_head: Some("66".repeat(32)),
            blocks: 8,
            metadata_committed: true,
            window_committed: true,
            reason: None,
        })
        .expect("a run")
    }

    fn claim(derived_root: &str, nonce: &str) -> AuditAttestation {
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
            derived_root: derived_root.to_string(),
            replay_head: "66".repeat(32),
            previous: None,
            sequence: 0,
            nonce: nonce.to_string(),
            signer: "07".repeat(33),
            sig_algorithm: "secp256k1".to_string(),
            sig: "00".repeat(64),
        }
    }

    /// A non-AGREE report is not a run that can accept anything.
    #[test]
    fn a_disagreement_cannot_accept_a_claim() {
        let mut report = run().report().clone();
        report.verdict = "DISAGREE".to_string();
        assert!(AuditRun::from_report(report).is_err());
    }

    /// A mismatched claim is refused and recorded as a conflict, never accepted.
    #[tokio::test]
    async fn a_mismatched_claim_is_refused_and_recorded_as_falsification() {
        let kvm = InMemoryStoreManager::default();
        let store = AuditStore::open(&kvm).await.expect("open");
        let claim = claim(&"99".repeat(32), "nonce-1");
        let refused = verify_and_accept(&store, &run(), &claim, 1_700_000_000_000).await;
        assert!(refused.is_err());
        assert!(store.accepted().await.expect("accepted").is_empty());
        assert_eq!(store.conflicts().await.expect("conflicts").len(), 1);
    }

    /// A replayed nonce is refused before acceptance, even with a matching root.
    #[tokio::test]
    async fn a_replayed_nonce_is_refused() {
        let kvm = InMemoryStoreManager::default();
        let store = AuditStore::open(&kvm).await.expect("open");
        let claim = claim(&"55".repeat(32), "nonce-1");
        store
            .accept(&Blake2b256Hash::from_byte_array(&[1u8; 32]), &claim)
            .await
            .expect("seed");
        let err = verify_and_accept(&store, &run(), &claim, 1_700_000_000_000)
            .await
            .expect_err("replayed");
        assert!(err.to_string().contains("nonce"));
    }
}
