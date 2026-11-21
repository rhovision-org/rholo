//! **The claims this node's own key has observed on the network** (Law 124; RC-4 item B).
//!
//! Law 122 made the *signing* decision sound: a validator proceeds only above its durable
//! own-sequence record. The drill found the remaining edge — between the signature and the hash
//! announcement the network's copy of this key's block at the same sequence can arrive, and the
//! announcement then makes the duplicate network-visible (`e3f5195c9`, 18:38:10).
//!
//! The cell is fed **only** by blocks that name this node's validator and pass the signature check
//! (`format_of_fields`, `block_hash`, `block_signature`), so a peer cannot forge an entry and hold a
//! proposal closed. It is keyed by sequence and keeps the first hash observed — two hashes at one
//! sequence are the equivocation the gate exists to prevent, and the first one is what the network
//! saw.
//!
//! Three readers, all fail-safe:
//!
//! * the block creator refuses to **sign** through a claim (before the expensive replay);
//! * the proposer's insert path refuses with `SelfEquivocation` (the existing not-a-failure arm),
//!   so an own-key verdict is not latched for a block this node never announced;
//! * `do_propose` re-checks immediately before the hash announcement — the issue's required
//!   broadcast-time guard. The residual race (a block still in transport buffering at the last
//!   check) is named in the HAZOP; H-1 and the durable own-key verdict remain the backstop.

use std::collections::BTreeMap;
use std::sync::Mutex;

use rchain_models::block_hash::BlockHash;
use rchain_models::validator::Validator;
use rchain_shared::refined::SeqNum;

/// How many of this validator's own sequences the cell remembers. A claim that matters is near the
/// sequence the proposer is deriving; keeping the most recent window bounds memory without ever
/// forgetting a sequence the node could still reuse.
const CLAIM_WINDOW: usize = 256;

/// The claims this node's key has observed, keyed by sequence.
#[derive(Debug)]
pub struct ReceivedClaims {
    own: Validator,
    inner: Mutex<BTreeMap<SeqNum, BlockHash>>,
}

impl ReceivedClaims {
    pub fn new(own: Validator) -> Self {
        ReceivedClaims {
            own,
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn own(&self) -> &Validator {
        &self.own
    }

    /// Record a signature-verified block by this node's key. The first hash observed at a sequence
    /// wins: a second, different hash is the equivocation itself, and it is the processor's job (the
    /// H-1 gate) to name it — this cell only keeps what the network showed first.
    pub fn record(&self, seq: SeqNum, hash: BlockHash) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.len() >= CLAIM_WINDOW && !inner.contains_key(&seq) {
            while inner.len() >= CLAIM_WINDOW {
                let Some(oldest) = inner.keys().next().copied() else {
                    break;
                };
                inner.remove(&oldest);
            }
        }
        inner.entry(seq).or_insert(hash);
    }

    /// The hash claimed at `seq`, if any.
    pub fn conflict_any(&self, seq: SeqNum) -> Option<BlockHash> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&seq)
            .copied()
    }

    /// A claim at `seq` that is **not** `our_hash`. `None` when the sequence is unclaimed or the
    /// claim is this very block.
    pub fn conflict_with(&self, seq: SeqNum, our_hash: BlockHash) -> Option<BlockHash> {
        self.conflict_any(seq)
            .filter(|claimed| *claimed != our_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> BlockHash {
        BlockHash::new([byte; 32])
    }

    fn seq(value: i64) -> SeqNum {
        SeqNum::try_from(value).expect("non-negative")
    }

    fn own() -> Validator {
        Validator::new([7; 65])
    }

    /// **A signature-valid self block records a claim; a same-hash claim never conflicts.**
    #[test]
    fn a_signature_valid_self_block_records_a_claim() {
        let claims = ReceivedClaims::new(own());
        claims.record(seq(196), hash(1));
        assert_eq!(claims.conflict_any(seq(196)), Some(hash(1)));
        assert_eq!(
            claims.conflict_with(seq(196), hash(1)),
            None,
            "the block itself is not a conflict"
        );
        assert_eq!(
            claims.conflict_with(seq(196), hash(2)),
            Some(hash(1)),
            "a different hash at the same sequence is the conflict"
        );
        assert_eq!(claims.conflict_any(seq(197)), None);
    }

    /// **The first observed hash wins.** A second, different hash at the same sequence is the
    /// equivocation the H-1 gate names; the cell does not let the later copy erase what the network
    /// showed first.
    #[test]
    fn the_first_observed_claim_is_kept() {
        let claims = ReceivedClaims::new(own());
        claims.record(seq(196), hash(1));
        claims.record(seq(196), hash(2));
        assert_eq!(claims.conflict_any(seq(196)), Some(hash(1)));
    }

    /// **The window is bounded.** The oldest sequences are pruned first, so a long-lived validator's
    /// cell cannot grow without bound.
    #[test]
    fn the_claim_window_is_bounded_and_prunes_the_oldest() {
        let claims = ReceivedClaims::new(own());
        for value in 0..(CLAIM_WINDOW as i64 + 10) {
            claims.record(seq(value), hash(1));
        }
        let inner = claims.inner.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(inner.len(), CLAIM_WINDOW);
        assert_eq!(
            inner.keys().next().copied(),
            Some(seq(10)),
            "the oldest ten sequences were pruned"
        );
    }
}
