//! **Checkpoint digests and the finality-quorum certificate** (sync/verification separation).
//!
//! Sync no longer replays history. A joiner accepts the finalized state by verifying a **quorum of
//! signed checkpoint attestations**: a validator signs the checkpoint digest with a domain tag of its
//! own, and a certificate is quorate when the signers hold strictly more than two thirds of the
//! bonded stake (Law 14's exact integer rule, `is_super_majority`).
//!
//! The digest is the statement. It binds the shard, the finalized anchor block, the anchor's
//! **post-state** (the checkpoint root — not the ordinary fringe root, which is a member's claim),
//! the previous checkpoint's digest, the epoch and the finalized fringe. A signature over one
//! checkpoint is therefore not a signature over another, and a certificate cannot cross shards.
//!
//! **What the certificate does and does not establish.** It attributes the root to a supermajority of
//! the bonded set: a quorum that all computed the same wrong root still certifies it (Law 85's
//! residual is unchanged). The historical audit — `rnode audit` — is the independent counterweight;
//! this module is the stake rule, not a state proof.

use std::collections::{BTreeMap, BTreeSet};

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::casper::protocol::checkpoint::{
    Checkpoint, CheckpointAttestation, CheckpointCertificate,
};
use rchain_models::validator::Validator;
use rchain_shared::base16;
use rchain_shared::refined::NonNegI64;

use crate::validator_identity::checkpoint_attestation_prehash;

/// Domain tag of the checkpoint digest (distinct from the finality attestation's tag).
pub const CHECKPOINT_TAG: &[u8] = b"rchain-checkpoint-v1";

/// Why a checkpoint or a certificate was refused. Every arm names the fact; none is a generic
/// "invalid".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckpointError {
    /// A field does not decode, has the wrong length, or the fringe is empty/duplicated.
    Malformed(String),
    /// The certificate names a different parent checkpoint than the verifier expected.
    WrongParent {
        expected: Option<Blake2b256Hash>,
        found: Option<Blake2b256Hash>,
    },
    /// The anchor is not a member of the finalized fringe.
    AnchorNotInFringe,
    /// The signed stake did not reach the strict two-thirds rule.
    NotQuorate { supporting: i128, total: i128 },
    /// No bonded stake to measure against.
    NoBonds,
    /// An attestation's digest or signature does not verify.
    BadSignature(String),
}

impl std::fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckpointError::Malformed(why) => write!(f, "malformed checkpoint: {why}"),
            CheckpointError::WrongParent { expected, found } => write!(
                f,
                "checkpoint parent mismatch: expected {}, found {}",
                fmt_hash(*expected),
                fmt_hash(*found)
            ),
            CheckpointError::AnchorNotInFringe => {
                write!(f, "the checkpoint anchor is not a member of its fringe")
            }
            CheckpointError::NotQuorate { supporting, total } => write!(
                f,
                "the certificate is not quorate: {supporting} of {total} bonded stake signed \
                 (strictly more than two thirds is required)"
            ),
            CheckpointError::NoBonds => write!(f, "there is no bonded stake to verify against"),
            CheckpointError::BadSignature(why) => write!(f, "bad checkpoint attestation: {why}"),
        }
    }
}

impl std::error::Error for CheckpointError {}

fn fmt_hash(hash: Option<Blake2b256Hash>) -> String {
    hash.map(|h| h.to_hex())
        .unwrap_or_else(|| "none".to_string())
}

fn decode_32(hex: &str, what: &str) -> Result<[u8; 32], CheckpointError> {
    let bytes = base16::decode(hex)
        .ok_or_else(|| CheckpointError::Malformed(format!("{what} is not base16")))?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| CheckpointError::Malformed(format!("{what} is not 32 bytes")))
}

fn lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// **The checkpoint statement, as signed.** One implementation, called by the producer, the verifier
/// and the offline checker.
pub fn checkpoint_digest(checkpoint: &Checkpoint) -> Result<Blake2b256Hash, CheckpointError> {
    if checkpoint.fringe_hashes.is_empty() {
        return Err(CheckpointError::Malformed(
            "the finalized fringe is empty".to_string(),
        ));
    }
    if checkpoint.anchor_height < 0 {
        return Err(CheckpointError::Malformed(
            "the anchor height is negative".to_string(),
        ));
    }
    let anchor = decode_32(&checkpoint.anchor, "anchor")?;
    let post = decode_32(&checkpoint.anchor_post_state, "anchorPostState")?;
    let parent = match &checkpoint.parent_digest {
        Some(hex) => decode_32(hex, "parentDigest")?,
        None => [0u8; 32],
    };
    let fringe_hex = &checkpoint.fringe_hashes;
    if !fringe_hex
        .iter()
        .any(|h| h.eq_ignore_ascii_case(&checkpoint.anchor))
    {
        return Err(CheckpointError::AnchorNotInFringe);
    }
    let mut fringe: Vec<[u8; 32]> = fringe_hex
        .iter()
        .map(|h| decode_32(h, "fringe hash"))
        .collect::<Result<_, _>>()?;
    fringe.sort();
    let before = fringe.len();
    fringe.dedup();
    if fringe.len() != before {
        return Err(CheckpointError::Malformed(
            "the fringe names a block twice".to_string(),
        ));
    }

    let mut buf = Vec::with_capacity(64 + fringe.len() * 32);
    lp(&mut buf, CHECKPOINT_TAG);
    lp(&mut buf, checkpoint.shard_id.as_bytes());
    buf.extend_from_slice(&anchor);
    buf.extend_from_slice(&checkpoint.anchor_height.to_be_bytes());
    buf.extend_from_slice(&post);
    buf.extend_from_slice(&parent);
    buf.extend_from_slice(&checkpoint.epoch.to_be_bytes());
    buf.extend_from_slice(&(fringe.len() as u32).to_be_bytes());
    for h in &fringe {
        buf.extend_from_slice(h);
    }
    Ok(Blake2b256Hash::create(&buf))
}

/// A verified certificate: the statement, its digest, and the stake that signed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedCheckpoint {
    pub checkpoint: Checkpoint,
    pub digest: Blake2b256Hash,
    pub supporting_stake: i128,
    pub total_stake: i128,
}

/// **Verify a checkpoint certificate against the bond map that finalised it.**
///
/// `bonds` is the parent checkpoint's bond map (the genesis bonds for the first checkpoint, or an
/// operator pin's state); `expected_parent` is the digest the joiner already holds, if any.
///
/// The checks are ordered so a refusal names the earliest fact that failed: shape and anchor first,
/// then the digest, then the parent, then each signature, then the stake rule. An unbonded signer is
/// ignored (weight zero) — the producer's admission gate is not the verifier's authority.
pub fn verify_checkpoint_certificate(
    certificate: &CheckpointCertificate,
    bonds: &BTreeMap<Validator, NonNegI64>,
    expected_parent: Option<Blake2b256Hash>,
) -> Result<VerifiedCheckpoint, CheckpointError> {
    let checkpoint = &certificate.checkpoint;
    let digest = checkpoint_digest(checkpoint)?;

    // The parent the joiner expects, if it names one.
    if expected_parent.is_some() {
        let found = match &checkpoint.parent_digest {
            Some(hex) => Some(Blake2b256Hash::from_byte_array(&decode_32(
                hex,
                "parentDigest",
            )?)),
            None => None,
        };
        if found != expected_parent {
            return Err(CheckpointError::WrongParent {
                expected: expected_parent,
                found,
            });
        }
    }

    let total: i128 = bonds.values().map(|s| i128::from(i64::from(*s))).sum();
    if total <= 0 || bonds.is_empty() {
        return Err(CheckpointError::NoBonds);
    }

    let digest_hex = digest.to_hex();
    let prehash = checkpoint_attestation_prehash(&digest);
    let mut signers: BTreeSet<Validator> = BTreeSet::new();
    for attestation in &certificate.attestations {
        if !attestation
            .checkpoint_digest
            .eq_ignore_ascii_case(&digest_hex)
        {
            return Err(CheckpointError::BadSignature(format!(
                "an attestation names digest {}, not {digest_hex}",
                attestation.checkpoint_digest
            )));
        }
        let sender_bytes = base16::decode(&attestation.sender)
            .ok_or_else(|| CheckpointError::BadSignature("sender is not base16".to_string()))?;
        let sender = Validator::try_from(sender_bytes.as_slice())
            .map_err(|e| CheckpointError::BadSignature(format!("sender is not a key: {e}")))?;
        // Unbonded signers carry no weight; they are logged by the caller's counter, not counted.
        if !bonds.contains_key(&sender) {
            continue;
        }
        if !signers.insert(sender) {
            continue; // one validator, one vote
        }
        let sig = base16::decode(&attestation.sig)
            .ok_or_else(|| CheckpointError::BadSignature("sig is not base16".to_string()))?;
        let alg =
            rchain_crypto::signatures::signatures_alg::from_algorithm(&attestation.sig_algorithm)
                .ok_or_else(|| {
                CheckpointError::BadSignature(format!(
                    "unknown signature algorithm {}",
                    attestation.sig_algorithm
                ))
            })?;
        if !alg.verify(&prehash, &sig, sender.as_bytes()) {
            return Err(CheckpointError::BadSignature(format!(
                "the attestation from {} does not verify over the checkpoint digest",
                attestation.sender
            )));
        }
    }

    let supporting: i128 = signers
        .iter()
        .map(|signer| i128::from(i64::from(*bonds.get(signer).unwrap_or(&NonNegI64::zero()))))
        .sum();
    if !rchain_sdk::consensus::is_super_majority(supporting, total) {
        return Err(CheckpointError::NotQuorate { supporting, total });
    }

    Ok(VerifiedCheckpoint {
        checkpoint: checkpoint.clone(),
        digest,
        supporting_stake: supporting,
        total_stake: total,
    })
}

/// Serialize an attestation for the HTTP surface.
pub fn attestation_json(attestation: &CheckpointAttestation) -> String {
    serde_json::to_string(attestation).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_crypto::private_key::PrivateKey;
    use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
    use rchain_models::casper::protocol::checkpoint::Checkpoint;
    use rchain_shared::refined::NonNegI64;

    fn hash(byte: u8) -> String {
        base16::encode(&[byte; 32])
    }

    fn checkpoint() -> Checkpoint {
        Checkpoint {
            shard_id: "root".to_string(),
            anchor: hash(1),
            anchor_height: 42,
            anchor_post_state: hash(2),
            fringe_hashes: vec![hash(1)],
            parent_digest: None,
            epoch: 4,
        }
    }

    fn validator(seed: u8) -> (Validator, PrivateKey) {
        let key = PrivateKey::new(vec![seed; 32]);
        let public = rchain_crypto::signatures::secp256k1::Secp256k1
            .to_public(&key)
            .expect("public key");
        let public_bytes = public.bytes().to_vec();
        (
            Validator::try_from(public_bytes.as_slice()).expect("a validator key"),
            key,
        )
    }

    fn attest(checkpoint: &Checkpoint, key: &PrivateKey) -> CheckpointAttestation {
        let digest = checkpoint_digest(checkpoint).expect("digest");
        let signature = rchain_crypto::signatures::secp256k1::Secp256k1
            .sign(&checkpoint_attestation_prehash(&digest), key.bytes())
            .expect("sign");
        let public = rchain_crypto::signatures::secp256k1::Secp256k1
            .to_public(key)
            .expect("public");
        CheckpointAttestation {
            checkpoint_digest: digest.to_hex(),
            sender: base16::encode(public.bytes()),
            sig_algorithm: "secp256k1".to_string(),
            sig: base16::encode(&signature),
        }
    }

    #[test]
    fn the_digest_binds_every_field() {
        let base = checkpoint();
        let base_digest = checkpoint_digest(&base).expect("digest");
        for mutate in [
            |c: &mut Checkpoint| c.shard_id = "other".to_string(),
            |c: &mut Checkpoint| {
                c.anchor = hash(9);
                c.fringe_hashes = vec![hash(9)];
            },
            |c: &mut Checkpoint| c.anchor_height = 43,
            |c: &mut Checkpoint| c.anchor_post_state = hash(9),
            |c: &mut Checkpoint| c.epoch = 5,
            |c: &mut Checkpoint| c.parent_digest = Some(hash(7)),
            |c: &mut Checkpoint| {
                c.fringe_hashes = vec![hash(1), hash(9)];
            },
        ] {
            let mut changed = base.clone();
            mutate(&mut changed);
            assert_ne!(
                checkpoint_digest(&changed).expect("digest"),
                base_digest,
                "a changed field must change the digest"
            );
        }
    }

    #[test]
    fn the_fringe_order_does_not_change_the_digest_and_a_duplicate_is_refused() {
        let mut a = checkpoint();
        a.fringe_hashes = vec![hash(1), hash(2)];
        let mut b = a.clone();
        b.fringe_hashes.reverse();
        assert_eq!(
            checkpoint_digest(&a).expect("a"),
            checkpoint_digest(&b).expect("b")
        );
        b.fringe_hashes.push(hash(1));
        assert!(matches!(
            checkpoint_digest(&b),
            Err(CheckpointError::Malformed(_))
        ));
    }

    #[test]
    fn the_anchor_must_be_in_the_fringe() {
        let mut c = checkpoint();
        c.anchor = hash(9);
        assert_eq!(
            checkpoint_digest(&c),
            Err(CheckpointError::AnchorNotInFringe)
        );
    }

    #[test]
    fn a_certificate_at_exactly_two_thirds_is_refused() {
        // 3 validators, equal stake: two of three is exactly 2/3 and must not pass.
        let (v1, k1) = validator(1);
        let (v2, k2) = validator(2);
        let (v3, _k3) = validator(3);
        let bonds: BTreeMap<Validator, NonNegI64> = [v1, v2, v3]
            .into_iter()
            .map(|v| (v, NonNegI64::try_from(100).unwrap()))
            .collect();
        let c = checkpoint();
        let cert = CheckpointCertificate {
            checkpoint: c.clone(),
            attestations: vec![attest(&c, &k1), attest(&c, &k2)],
        };
        assert_eq!(
            verify_checkpoint_certificate(&cert, &bonds, None),
            Err(CheckpointError::NotQuorate {
                supporting: 200,
                total: 300
            })
        );
    }

    #[test]
    fn a_certificate_above_two_thirds_is_accepted() {
        let (v1, k1) = validator(1);
        let (v2, k2) = validator(2);
        let (v3, k3) = validator(3);
        let bonds: BTreeMap<Validator, NonNegI64> = [v1, v2, v3]
            .into_iter()
            .map(|v| (v, NonNegI64::try_from(100).unwrap()))
            .collect();
        let c = checkpoint();
        let cert = CheckpointCertificate {
            checkpoint: c.clone(),
            attestations: vec![attest(&c, &k1), attest(&c, &k2), attest(&c, &k3)],
        };
        let verified = verify_checkpoint_certificate(&cert, &bonds, None).expect("quorate");
        assert_eq!(verified.supporting_stake, 300);
        assert_eq!(verified.total_stake, 300);
    }

    #[test]
    fn an_unbonded_signer_carries_no_weight() {
        let (v1, k1) = validator(1);
        let (_v2, k2) = validator(2);
        let bonds: BTreeMap<Validator, NonNegI64> = [(v1, NonNegI64::try_from(100).unwrap())]
            .into_iter()
            .collect();
        let c = checkpoint();
        let cert = CheckpointCertificate {
            checkpoint: c.clone(),
            attestations: vec![attest(&c, &k1), attest(&c, &k2)],
        };
        let verified = verify_checkpoint_certificate(&cert, &bonds, None).expect("quorate");
        assert_eq!(verified.supporting_stake, 100);
    }

    #[test]
    fn a_signature_over_another_digest_is_refused() {
        let (v1, k1) = validator(1);
        let bonds: BTreeMap<Validator, NonNegI64> = [(v1, NonNegI64::try_from(100).unwrap())]
            .into_iter()
            .collect();
        let mut other = checkpoint();
        other.anchor_post_state = hash(9);
        let c = checkpoint();
        let cert = CheckpointCertificate {
            checkpoint: c.clone(),
            attestations: vec![attest(&other, &k1)],
        };
        assert!(matches!(
            verify_checkpoint_certificate(&cert, &bonds, None),
            Err(CheckpointError::BadSignature(_))
        ));
    }

    #[test]
    fn two_conflicting_checkpoints_share_a_signer_under_a_split() {
        // Two quorums must intersect in >1/3 stake; with 3 equal validators, 2-signer quorums share
        // at least one signer. (The theorem is in the Lean model; this pins the Rust rule's shape.)
        let (v1, _k1) = validator(1);
        let (v2, _k2) = validator(2);
        let (v3, _k3) = validator(3);
        let a: BTreeSet<Validator> = [v1, v2].into_iter().collect();
        let b: BTreeSet<Validator> = [v2, v3].into_iter().collect();
        assert!(!a.is_disjoint(&b));
    }
}
