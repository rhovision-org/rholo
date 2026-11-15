//! **A signed re-execution claim** (Law 112).
//!
//! `rnode audit` reproduces a checkpoint from genesis, but the report it writes is a local file:
//! anyone can copy it, alter it, or repeat the claim for free. An `AuditAttestation` signs the
//! result and chains it to the signer's previous claim:
//!
//! * the **claim** names the checkpoint, the anchor and its height, the certificate's metadata and
//!   window commitments, the `derivedRoot` the replay produced, and a `replayHead` — a hash chain
//!   over the anchor's canonical ancestor closure;
//! * `previous`/`sequence` make the claims an append-only chain per signer;
//! * a `nonce` binds a claim to the request that asked for it, so a captured claim is not a fresh
//!   answer.
//!
//! **Acceptance is reproduction, not signature.** A node stores a claim only after its own replay
//! agrees on the root and the replay head; that rule is the caller's (`casper::audit`), and the
//! accepted type there has no public constructor. This module only fixes the statement and its
//! canonical bytes — never JSON — so a field cannot be reinterpreted by another parser.

use std::collections::BTreeSet;

use rchain_crypto::hash::blake2b256;
use rchain_shared::base16;
use serde::{Deserialize, Serialize};

use crate::block::state_hash::StateHash;
use crate::block_hash::BlockHash;

/// The statement's domain tag, first in the signed preimage.
pub const AUDIT_ATTESTATION_TAG: &str = "rchain-audit-attestation-v1";
/// The witness this statement carries. A future zero-knowledge witness would name another value.
pub const SIGNED_HASH_CHAIN_WITNESS: &str = "signed-hash-chain-v1";
/// The replay head's genesis tag.
pub const REPLAY_GENESIS_TAG: &str = "rchain-audit-replay-genesis-v1";
/// One replay step's tag.
pub const REPLAY_STEP_TAG: &str = "rchain-audit-replay-step-v1";

/// **One signed re-execution claim.**
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditAttestation {
    /// `signed-hash-chain-v1`.
    pub witness_type: String,
    pub shard_id: String,
    /// The digest of the checkpoint statement this claim reproduces.
    pub checkpoint_digest: String,
    /// The certified anchor.
    pub anchor: String,
    pub anchor_height: i64,
    /// Always `0`: an audit reproduces from genesis, not from an unverified intermediate state.
    pub from_height: i64,
    pub to_height: i64,
    /// The certificate's commitments. Both are required: a legacy uncommitted certificate cannot be
    /// attested, because there is nothing for the claim to be relative to.
    pub metadata_root: String,
    pub history_root: String,
    /// The state root the replay **actually derived**.
    pub derived_root: String,
    /// The cumulative hash over the anchor's canonical ancestor closure.
    pub replay_head: String,
    /// The signer's previous claim; `None` for sequence 0. An explicit all-zero hash is refused.
    pub previous: Option<String>,
    pub sequence: u64,
    /// The challenge nonce this claim answers.
    pub nonce: String,
    /// The signer's public key (hex).
    pub signer: String,
    pub sig_algorithm: String,
    pub sig: String,
}

/// Why a claim is not a well-shaped statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditShapeError {
    /// The witness type is not one this build verifies.
    UnknownWitness(String),
    /// `from_height` is not zero, or `to_height` is not the anchor's height.
    NotAGenesisReplay {
        from: i64,
        to: i64,
        anchor_height: i64,
    },
    /// A hash field is not 32 bytes of base16.
    BadHash { field: &'static str },
    /// `previous` is the explicit zero digest; the absent previous is `null`, not zero.
    ExplicitZeroPrevious,
    /// A negative height.
    NegativeHeight(i64),
}

impl std::fmt::Display for AuditShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditShapeError::UnknownWitness(w) => write!(f, "unknown audit witness type {w}"),
            AuditShapeError::NotAGenesisReplay {
                from,
                to,
                anchor_height,
            } => write!(
                f,
                "an audit attestation reproduces from genesis: expected from=0 to={anchor_height}, \
                 got from={from} to={to}"
            ),
            AuditShapeError::BadHash { field } => write!(f, "{field} is not a 32-byte hex digest"),
            AuditShapeError::ExplicitZeroPrevious => {
                write!(
                    f,
                    "previous is the explicit zero digest; the absent previous is null"
                )
            }
            AuditShapeError::NegativeHeight(h) => write!(f, "negative height {h}"),
        }
    }
}

impl AuditAttestation {
    /// **Check the statement's shape and return its canonical bytes.** The canonical preimage is
    /// length-prefixed so no field can be shifted into another, and the tag is part of it so a
    /// signature for another statement type cannot be replayed here.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AuditShapeError> {
        if self.witness_type != SIGNED_HASH_CHAIN_WITNESS {
            return Err(AuditShapeError::UnknownWitness(self.witness_type.clone()));
        }
        if self.anchor_height < 0 {
            return Err(AuditShapeError::NegativeHeight(self.anchor_height));
        }
        if self.from_height != 0 || self.to_height != self.anchor_height {
            return Err(AuditShapeError::NotAGenesisReplay {
                from: self.from_height,
                to: self.to_height,
                anchor_height: self.anchor_height,
            });
        }
        let checkpoint_digest = hash_bytes(&self.checkpoint_digest, "checkpointDigest")?;
        let anchor = hash_bytes(&self.anchor, "anchor")?;
        let metadata_root = hash_bytes(&self.metadata_root, "metadataRoot")?;
        let history_root = hash_bytes(&self.history_root, "historyRoot")?;
        let derived_root = hash_bytes(&self.derived_root, "derivedRoot")?;
        let replay_head = hash_bytes(&self.replay_head, "replayHead")?;
        let previous = match &self.previous {
            Some(hex) => {
                let bytes = hash_bytes(hex, "previous")?;
                if bytes == [0u8; 32] {
                    return Err(AuditShapeError::ExplicitZeroPrevious);
                }
                Some(bytes)
            }
            None => None,
        };
        let mut buf = Vec::new();
        lp(&mut buf, AUDIT_ATTESTATION_TAG.as_bytes());
        lp(&mut buf, self.witness_type.as_bytes());
        lp(&mut buf, self.shard_id.as_bytes());
        buf.extend_from_slice(&checkpoint_digest);
        buf.extend_from_slice(&anchor);
        buf.extend_from_slice(&self.anchor_height.to_be_bytes());
        buf.extend_from_slice(&self.from_height.to_be_bytes());
        buf.extend_from_slice(&self.to_height.to_be_bytes());
        buf.extend_from_slice(&metadata_root);
        buf.extend_from_slice(&history_root);
        buf.extend_from_slice(&derived_root);
        buf.extend_from_slice(&replay_head);
        match previous {
            Some(previous) => {
                buf.push(1);
                buf.extend_from_slice(&previous);
            }
            None => buf.push(0),
        }
        buf.extend_from_slice(&self.sequence.to_be_bytes());
        lp(&mut buf, self.nonce.as_bytes());
        lp(&mut buf, self.signer.as_bytes());
        lp(&mut buf, self.sig_algorithm.as_bytes());
        Ok(buf)
    }

    /// The statement's digest: the hash of its canonical bytes.
    pub fn digest(&self) -> Result<Vec<u8>, AuditShapeError> {
        Ok(blake2b256::hash(&self.canonical_bytes()?))
    }

    /// The value the signer signs: the domain tag over the statement digest.
    pub fn prehash(&self) -> Result<Vec<u8>, AuditShapeError> {
        let digest = self.digest()?;
        let mut buf = Vec::new();
        lp(&mut buf, AUDIT_ATTESTATION_TAG.as_bytes());
        buf.extend_from_slice(&digest);
        Ok(blake2b256::hash(&buf))
    }
}

/// One replayed block: its hash, height and post-state root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayStep {
    pub block_hash: BlockHash,
    pub height: i64,
    pub post_state_hash: StateHash,
}

/// **The cumulative hash over the replay.** A domain-tagged genesis value seeded by the checkpoint
/// digest, then one domain-tagged step per block, sorted by `(height, hash)` so two honest replays
/// of the same closure agree regardless of the order they learned it. Genesis is part of the
/// closure (it is a block the replay consumed), so a claim that omits it produces another head.
pub fn replay_head(
    checkpoint_digest: &[u8],
    steps: impl IntoIterator<Item = ReplayStep>,
) -> Vec<u8> {
    let mut steps: Vec<ReplayStep> = steps.into_iter().collect();
    steps.sort_by_key(|step| (step.height, step.block_hash));
    // Two steps for one block are a caller defect; the closure is a set. Deduplicate by hash so a
    // duplicated body cannot silently lengthen the chain.
    let mut seen: BTreeSet<BlockHash> = BTreeSet::new();
    let mut buf = Vec::new();
    lp(&mut buf, REPLAY_GENESIS_TAG.as_bytes());
    buf.extend_from_slice(checkpoint_digest);
    let mut head = blake2b256::hash(&buf);
    for step in steps {
        if !seen.insert(step.block_hash) {
            continue;
        }
        let mut buf = Vec::new();
        lp(&mut buf, REPLAY_STEP_TAG.as_bytes());
        buf.extend_from_slice(&head);
        buf.extend_from_slice(step.block_hash.as_bytes());
        buf.extend_from_slice(&step.height.to_be_bytes());
        buf.extend_from_slice(step.post_state_hash.as_bytes());
        head = blake2b256::hash(&buf);
    }
    head
}

fn hash_bytes(hex: &str, field: &'static str) -> Result<[u8; 32], AuditShapeError> {
    let bytes = base16::decode(hex).ok_or(AuditShapeError::BadHash { field })?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| AuditShapeError::BadHash { field })?;
    Ok(bytes)
}

fn lp(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    buf.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attestation() -> AuditAttestation {
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
            sequence: 0,
            nonce: "nonce-1".to_string(),
            signer: "07".repeat(33),
            sig_algorithm: "secp256k1".to_string(),
            sig: "88".repeat(64),
        }
    }

    /// **The statement's encoding is pinned.** A golden digest catches a field silently leaving the
    /// preimage, which is what would let a claim be altered without breaking its signature.
    #[test]
    fn the_audit_claim_digest_is_golden() {
        let digest = attestation().digest().expect("shape");
        assert_eq!(
            base16::encode(&digest),
            // Independently recomputed with Python's `hashlib.blake2b(digest_size=32)` over the
            // canonical preimage, not only pinned from the Rust encoder.
            "d3d8e684c8e3a5172411dd8e5c900dc50ef72f91487c786b3a1997d1bbf86fe9",
            "the golden digest of the canonical claim"
        );
    }

    /// The absent previous is a presence byte, and the explicit zero is refused: `null` and
    /// `Some(zero)` must not encode the same preimage.
    #[test]
    fn an_explicit_zero_previous_is_not_the_absent_previous() {
        let mut absent = attestation();
        absent.previous = None;
        let mut zero = attestation();
        zero.previous = Some("00".repeat(32));
        assert_eq!(
            zero.canonical_bytes(),
            Err(AuditShapeError::ExplicitZeroPrevious)
        );
        assert_ne!(absent.canonical_bytes().expect("absent"), Vec::<u8>::new());
    }

    /// An audit is a genesis replay: a prefix range and a non-genesis `from` are refused.
    #[test]
    fn a_prefix_height_range_is_refused() {
        let mut claim = attestation();
        claim.from_height = 3;
        claim.to_height = 7;
        assert!(matches!(
            claim.canonical_bytes(),
            Err(AuditShapeError::NotAGenesisReplay { .. })
        ));
        let mut claim = attestation();
        claim.to_height = 6;
        assert!(matches!(
            claim.canonical_bytes(),
            Err(AuditShapeError::NotAGenesisReplay { .. })
        ));
    }

    /// Only the witness this build verifies is accepted.
    #[test]
    fn an_unknown_witness_type_is_refused() {
        let mut claim = attestation();
        claim.witness_type = "zk-snark-v1".to_string();
        assert!(matches!(
            claim.canonical_bytes(),
            Err(AuditShapeError::UnknownWitness(_))
        ));
    }

    /// Every field is in the preimage: changing one changes the digest.
    #[test]
    fn every_field_changes_the_digest() {
        let base = attestation().digest().expect("shape");
        let mut other = attestation();
        other.nonce = "nonce-2".to_string();
        assert_ne!(base, other.digest().expect("shape"));
        let mut other = attestation();
        other.derived_root = "99".repeat(32);
        assert_ne!(base, other.digest().expect("shape"));
        let mut other = attestation();
        other.previous = Some("aa".repeat(32));
        assert_ne!(base, other.digest().expect("shape"));
    }

    /// The replay head is order-independent and covers each block once; a changed post-state or a
    /// dropped step changes it.
    #[test]
    fn the_replay_head_is_order_independent_and_covers_every_block() {
        let one = ReplayStep {
            block_hash: BlockHash::new([1u8; 32]),
            height: 0,
            post_state_hash: StateHash::new([0xA0; 32]),
        };
        let two = ReplayStep {
            block_hash: BlockHash::new([2u8; 32]),
            height: 1,
            post_state_hash: StateHash::new([0xA1; 32]),
        };
        let checkpoint = [7u8; 32];
        let forward = replay_head(&checkpoint, [one, two]);
        let reverse = replay_head(&checkpoint, [two, one]);
        assert_eq!(forward, reverse, "sorted by (height, hash)");
        assert_ne!(forward, replay_head(&checkpoint, [one]), "a dropped step");
        let mut changed = two;
        changed.post_state_hash = StateHash::new([0xA2; 32]);
        assert_ne!(
            forward,
            replay_head(&checkpoint, [one, changed]),
            "a changed post-state"
        );
        assert_ne!(
            forward,
            replay_head(&[8u8; 32], [one, two]),
            "another checkpoint seeds another head"
        );
    }
}
