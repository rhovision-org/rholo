//! **Verifying a signed re-execution claim** (Law 112).
//!
//! The statement and its canonical bytes live in `rchain_models::casper::protocol::audit`. This
//! module is the **acceptance** rule:
//!
//! * the statement is well shaped (a genesis replay, known witness);
//! * its signature verifies over the domain-tagged prehash;
//! * it names the checkpoint this node is checking, and the roots and replay head agree with the
//!   node's **own** replay (`AuditExpectation`);
//! * it extends its signer's chain (`previous`/`sequence`), or is sequence zero with no previous.
//!
//! A `VerifiedAuditAttestation` can only be built here, and its fields are private: there is no
//! public path from a wire claim to an accepted one. The node's store takes this type, never a
//! bare `AuditAttestation`.

use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::signatures_alg::from_algorithm;
use rchain_models::casper::protocol::audit::{
    AuditAttestation, AuditShapeError, SIGNED_HASH_CHAIN_WITNESS,
};
use rchain_shared::base16;
use serde::Serialize;

/// What this node's own replay says the claim must contain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditExpectation {
    /// The checkpoint statement's digest.
    pub checkpoint_digest: String,
    pub anchor: String,
    pub anchor_height: i64,
    /// The certificate's commitments, which the claim must name.
    pub metadata_root: String,
    pub history_root: String,
    /// The state root this node's replay derived.
    pub derived_root: String,
    /// The replay head this node's replay computed.
    pub replay_head: String,
}

/// Why a claim was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditVerificationError {
    /// The statement is not well shaped.
    Shape(AuditShapeError),
    /// The signature does not verify over the prehash.
    Signature,
    /// The claim names another checkpoint or another anchor.
    WrongCheckpoint,
    /// The claim's commitments are not the certificate's.
    WrongCommitment,
    /// The claimed derived root is not the root this node's replay produced.
    RootMismatch { claimed: String, reproduced: String },
    /// The claimed replay head is not the one this node computed.
    ReplayHeadMismatch { claimed: String, reproduced: String },
    /// The claim does not extend its signer's accepted head.
    Unchained {
        expected_sequence: u64,
        expected_previous: Option<String>,
        found_sequence: u64,
        found_previous: Option<String>,
    },
    /// Another signer's chain cannot be extended by this claim.
    ForeignSigner { expected: String, found: String },
}

impl std::fmt::Display for AuditVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditVerificationError::Shape(e) => write!(f, "{e}"),
            AuditVerificationError::Signature => {
                write!(f, "the audit attestation's signature does not verify")
            }
            AuditVerificationError::WrongCheckpoint => {
                write!(
                    f,
                    "the audit attestation names another checkpoint or anchor"
                )
            }
            AuditVerificationError::WrongCommitment => write!(
                f,
                "the audit attestation's metadata/window commitment is not the certificate's"
            ),
            AuditVerificationError::RootMismatch {
                claimed,
                reproduced,
            } => write!(
                f,
                "the claimed derived root {claimed} is not the reproduced root {reproduced}"
            ),
            AuditVerificationError::ReplayHeadMismatch {
                claimed,
                reproduced,
            } => write!(
                f,
                "the claimed replay head {claimed} is not the reproduced head {reproduced}"
            ),
            AuditVerificationError::Unchained {
                expected_sequence,
                expected_previous,
                found_sequence,
                found_previous,
            } => write!(
                f,
                "the claim does not extend its signer's chain: expected sequence \
                 {expected_sequence} after {expected_previous:?}, got sequence {found_sequence} \
                 after {found_previous:?}"
            ),
            AuditVerificationError::ForeignSigner { expected, found } => write!(
                f,
                "the claim extends {found}'s chain, not the accepted signer {expected}"
            ),
        }
    }
}

/// The accepted head of one signer's chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditChainHead {
    pub signer: String,
    pub sequence: u64,
    /// The last accepted claim's digest.
    pub digest: String,
}

/// **A claim this node reproduced.** Private fields, and only [`verify_audit_attestation`] builds
/// one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VerifiedAuditAttestation {
    attestation: AuditAttestation,
    digest: String,
}

impl VerifiedAuditAttestation {
    pub fn attestation(&self) -> &AuditAttestation {
        &self.attestation
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn signer(&self) -> &str {
        &self.attestation.signer
    }

    pub fn sequence(&self) -> u64 {
        self.attestation.sequence
    }

    /// The head this claim becomes for its signer.
    pub fn chain_head(&self) -> AuditChainHead {
        AuditChainHead {
            signer: self.attestation.signer.clone(),
            sequence: self.attestation.sequence,
            digest: self.digest.clone(),
        }
    }
}

/// **Verify a claim against this node's replay and its signer's chain.**
///
/// `head` is the last accepted claim for the signer, or `None` when this is its first.
pub fn verify_audit_attestation(
    attestation: &AuditAttestation,
    expectation: &AuditExpectation,
    head: Option<&AuditChainHead>,
) -> Result<VerifiedAuditAttestation, AuditVerificationError> {
    if attestation.witness_type != SIGNED_HASH_CHAIN_WITNESS {
        return Err(AuditVerificationError::Shape(
            AuditShapeError::UnknownWitness(attestation.witness_type.clone()),
        ));
    }
    let prehash = attestation
        .prehash()
        .map_err(AuditVerificationError::Shape)?;
    let algorithm =
        from_algorithm(&attestation.sig_algorithm).ok_or(AuditVerificationError::Signature)?;
    let public = base16::decode(&attestation.signer).ok_or(AuditVerificationError::Signature)?;
    let signature = base16::decode(&attestation.sig).ok_or(AuditVerificationError::Signature)?;
    if !algorithm.verify(&prehash, &signature, PublicKey::new(public).bytes()) {
        return Err(AuditVerificationError::Signature);
    }

    if !attestation
        .checkpoint_digest
        .eq_ignore_ascii_case(&expectation.checkpoint_digest)
        || !attestation.anchor.eq_ignore_ascii_case(&expectation.anchor)
        || attestation.anchor_height != expectation.anchor_height
        || attestation.to_height != expectation.anchor_height
    {
        return Err(AuditVerificationError::WrongCheckpoint);
    }
    if !attestation
        .metadata_root
        .eq_ignore_ascii_case(&expectation.metadata_root)
        || !attestation
            .history_root
            .eq_ignore_ascii_case(&expectation.history_root)
    {
        return Err(AuditVerificationError::WrongCommitment);
    }
    if !attestation
        .derived_root
        .eq_ignore_ascii_case(&expectation.derived_root)
    {
        return Err(AuditVerificationError::RootMismatch {
            claimed: attestation.derived_root.clone(),
            reproduced: expectation.derived_root.clone(),
        });
    }
    if !attestation
        .replay_head
        .eq_ignore_ascii_case(&expectation.replay_head)
    {
        return Err(AuditVerificationError::ReplayHeadMismatch {
            claimed: attestation.replay_head.clone(),
            reproduced: expectation.replay_head.clone(),
        });
    }

    match head {
        None => {
            if attestation.sequence != 0 {
                return Err(AuditVerificationError::Unchained {
                    expected_sequence: 0,
                    expected_previous: None,
                    found_sequence: attestation.sequence,
                    found_previous: attestation.previous.clone(),
                });
            }
            // A first claim has no previous. The shape check already refuses an explicit zero;
            // this refuses a non-zero previous on sequence zero, which would name a head that is
            // not this signer's.
            if attestation.previous.is_some() {
                return Err(AuditVerificationError::Unchained {
                    expected_sequence: 0,
                    expected_previous: None,
                    found_sequence: 0,
                    found_previous: attestation.previous.clone(),
                });
            }
        }
        Some(head) => {
            if !attestation.signer.eq_ignore_ascii_case(&head.signer) {
                return Err(AuditVerificationError::ForeignSigner {
                    expected: head.signer.clone(),
                    found: attestation.signer.clone(),
                });
            }
            let expected_sequence = head.sequence + 1;
            let previous = attestation.previous.as_deref();
            if attestation.sequence != expected_sequence
                || !previous.is_some_and(|previous| previous.eq_ignore_ascii_case(&head.digest))
            {
                return Err(AuditVerificationError::Unchained {
                    expected_sequence,
                    expected_previous: Some(head.digest.clone()),
                    found_sequence: attestation.sequence,
                    found_previous: attestation.previous.clone(),
                });
            }
        }
    }

    let digest = base16::encode(
        &attestation
            .digest()
            .map_err(AuditVerificationError::Shape)?,
    );
    Ok(VerifiedAuditAttestation {
        attestation: attestation.clone(),
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
    use rchain_models::casper::protocol::audit::SIGNED_HASH_CHAIN_WITNESS;

    fn keys() -> (
        &'static dyn SignaturesAlg,
        rchain_crypto::private_key::PrivateKey,
        String,
    ) {
        let algorithm = from_algorithm("secp256k1").expect("registered");
        let (secret, public) = algorithm.new_key_pair();
        let public = base16::encode(public.bytes());
        (algorithm, secret, public)
    }

    fn expectation() -> AuditExpectation {
        AuditExpectation {
            checkpoint_digest: "11".repeat(32),
            anchor: "22".repeat(32),
            anchor_height: 7,
            metadata_root: "33".repeat(32),
            history_root: "44".repeat(32),
            derived_root: "55".repeat(32),
            replay_head: "66".repeat(32),
        }
    }

    fn signed(
        signer: &str,
        secret: &rchain_crypto::private_key::PrivateKey,
        algorithm: &'static dyn SignaturesAlg,
        sequence: u64,
        previous: Option<String>,
    ) -> AuditAttestation {
        let mut attestation = AuditAttestation {
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
            previous,
            sequence,
            nonce: "nonce-1".to_string(),
            signer: signer.to_string(),
            sig_algorithm: algorithm.name().to_string(),
            sig: String::new(),
        };
        let prehash = attestation.prehash().expect("shape");
        let signature = algorithm.sign(&prehash, secret.bytes()).expect("sign");
        attestation.sig = base16::encode(&signature);
        attestation
    }

    #[test]
    fn a_reproduced_claim_is_accepted() {
        let (algorithm, secret, public) = keys();
        let verified = verify_audit_attestation(
            &signed(&public, &secret, algorithm, 0, None),
            &expectation(),
            None,
        )
        .expect("accepted");
        assert_eq!(verified.signer(), public);
        assert_eq!(verified.sequence(), 0);
        assert_eq!(verified.chain_head().sequence, 0);
    }

    #[test]
    fn a_tampered_derived_root_is_refused() {
        let (algorithm, secret, public) = keys();
        let mut claim = signed(&public, &secret, algorithm, 0, None);
        claim.derived_root = "99".repeat(32);
        // Re-sign so the signature is valid: the refusal must be the reproduction, not the key.
        let prehash = claim.prehash().expect("shape");
        claim.sig = base16::encode(&algorithm.sign(&prehash, secret.bytes()).expect("sign"));
        assert!(matches!(
            verify_audit_attestation(&claim, &expectation(), None),
            Err(AuditVerificationError::RootMismatch { .. })
        ));
    }

    #[test]
    fn an_attestation_from_another_key_is_refused() {
        let (algorithm, secret, public) = keys();
        let (_other_algorithm, _other_secret, other_public) = keys();
        let mut claim = signed(&public, &secret, algorithm, 0, None);
        claim.signer = other_public;
        assert_eq!(
            verify_audit_attestation(&claim, &expectation(), None),
            Err(AuditVerificationError::Signature)
        );
    }

    #[test]
    fn a_claim_for_another_checkpoint_is_refused() {
        let (algorithm, secret, public) = keys();
        let claim = signed(&public, &secret, algorithm, 0, None);
        let mut other = expectation();
        other.checkpoint_digest = "77".repeat(32);
        assert_eq!(
            verify_audit_attestation(&claim, &other, None),
            Err(AuditVerificationError::WrongCheckpoint)
        );
    }

    #[test]
    fn a_replay_head_mismatch_is_refused() {
        let (algorithm, secret, public) = keys();
        let claim = signed(&public, &secret, algorithm, 0, None);
        let mut other = expectation();
        other.replay_head = "88".repeat(32);
        assert!(matches!(
            verify_audit_attestation(&claim, &other, None),
            Err(AuditVerificationError::ReplayHeadMismatch { .. })
        ));
    }

    #[test]
    fn a_chained_claim_extends_the_head_and_an_unchained_one_is_refused() {
        let (algorithm, secret, public) = keys();
        let first = verify_audit_attestation(
            &signed(&public, &secret, algorithm, 0, None),
            &expectation(),
            None,
        )
        .expect("first");
        let head = first.chain_head();

        let chained = signed(&public, &secret, algorithm, 1, Some(head.digest.clone()));
        assert!(verify_audit_attestation(&chained, &expectation(), Some(&head)).is_ok());

        // A claim that skips a sequence, or names another previous, cannot extend the chain.
        assert!(matches!(
            verify_audit_attestation(
                &signed(&public, &secret, algorithm, 2, Some(head.digest.clone())),
                &expectation(),
                Some(&head)
            ),
            Err(AuditVerificationError::Unchained { .. })
        ));
        assert!(matches!(
            verify_audit_attestation(
                &signed(&public, &secret, algorithm, 1, Some("aa".repeat(32))),
                &expectation(),
                Some(&head)
            ),
            Err(AuditVerificationError::Unchained { .. })
        ));

        // Sequence zero with no head is the first claim; sequence zero with a previous is not.
        assert!(matches!(
            verify_audit_attestation(
                &signed(&public, &secret, algorithm, 0, Some("aa".repeat(32))),
                &expectation(),
                None
            ),
            Err(AuditVerificationError::Unchained { .. })
        ));
    }
}
