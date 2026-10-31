//! **Cross-deploy batch certification** (Law 86c): independent deploys may execute concurrently;
//! conflicting ones serialise.
//!
//! A block's deploy list is executed sequentially today. The batch executor forks one runtime per
//! deploy from the same base and merges the resulting `DeployChainIndex`es, so the state root is the
//! merge's rather than the sequential fold's. That is sound only when the deploys do not interact:
//! 86c's criterion is the merge's own **event-log conflict relation** (`are_conflicting`, the
//! relation the block merge already trusts) plus **native-slot disjointness** (the vault, PoS and
//! registry writes that are not RSpace events). This module is the certificate; the executor falls
//! back to the sequential path for any uncertified batch.
//!
//! The certificate is conservative: it refuses a pair if either relation sees an overlap. A refused
//! pair is not an error — it is the scheduler's signal to run the deploy after the batch, which is
//! exactly the "what can't run in parallel stays pending" half of the law.

use std::collections::BTreeSet;
use std::sync::Arc;

use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_rspace::native_store::NativeStoreAction;

use crate::merging::DeployChainIndex;

/// Why a pair of deploys may not be batched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchConflict {
    /// The merge's event-log relation sees a shared channel or a potential COMM.
    EventLog,
    /// Both deploys write the same native slot (vault, PoS, registry, ...).
    NativeSlot(u8, Blake2b256Hash),
}

/// The certificate for one candidate batch, and the first conflict that refused it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchCertificate {
    pub certified: bool,
    pub conflict: Option<BatchConflict>,
}

/// Whether `candidate` may join the deploys already in `batch`. `batch` is expected to be
/// pairwise certified itself; this checks the new pair only.
pub fn compatible(batch: &[Arc<DeployChainIndex>], candidate: &Arc<DeployChainIndex>) -> bool {
    batch.iter().all(|existing| {
        !DeployChainIndex::deploys_are_conflicting(existing, candidate)
            && native_slots(existing).is_disjoint(&native_slots(candidate))
    })
}

/// Certify a whole batch. Empty and singleton batches are trivially certified; the executor still
/// runs a singleton through the merge path only when it is worth forking, and the sequential path
/// for the rest.
pub fn certify(chains: &[Arc<DeployChainIndex>]) -> BatchCertificate {
    for (i, a) in chains.iter().enumerate() {
        for b in &chains[i + 1..] {
            if DeployChainIndex::deploys_are_conflicting(a, b) {
                return BatchCertificate {
                    certified: false,
                    conflict: Some(BatchConflict::EventLog),
                };
            }
            let slots = native_slots(a);
            if let Some(slot) = native_slots(b).iter().find(|s| slots.contains(s)) {
                return BatchCertificate {
                    certified: false,
                    conflict: Some(BatchConflict::NativeSlot(slot.0, slot.1)),
                };
            }
        }
    }
    BatchCertificate {
        certified: true,
        conflict: None,
    }
}

fn native_slots(chain: &DeployChainIndex) -> BTreeSet<(u8, Blake2b256Hash)> {
    chain
        .native_effects
        .iter()
        .map(NativeStoreAction::slot)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merging::DeployIdWithCost;
    use rchain_rspace::merger::event_log_index::EventLogIndex;
    use rchain_rspace::merger::state_change::StateChange;

    fn hash(byte: u8) -> Blake2b256Hash {
        Blake2b256Hash::from_bytes([byte; 32])
    }

    fn chain(id: u8, slots: Vec<(u8, Blake2b256Hash)>) -> Arc<DeployChainIndex> {
        Arc::new(DeployChainIndex {
            host_block: hash(0),
            deploys_with_cost: BTreeSet::from([DeployIdWithCost {
                id: vec![id],
                cost: 0,
            }]),
            pre_state_hash: hash(0),
            post_state_hash: hash(id),
            event_log_index: EventLogIndex::empty(),
            state_changes: StateChange::empty(),
            cost_moves: std::collections::BTreeMap::new(),
            executor: String::new(),
            native_effects: slots
                .into_iter()
                .map(|(prefix, key)| NativeStoreAction::Put {
                    prefix,
                    key,
                    value: vec![id],
                })
                .collect(),
            first_deploy_ordinal: u32::from(id),
        })
    }

    #[test]
    fn disjoint_deploys_are_certified() {
        let a = chain(1, vec![(1, hash(10))]);
        let b = chain(2, vec![(1, hash(20))]);
        assert_eq!(
            certify(&[a.clone(), b.clone()]),
            BatchCertificate {
                certified: true,
                conflict: None
            }
        );
        assert!(compatible(&[a], &b));
    }

    #[test]
    fn a_shared_native_slot_is_refused() {
        let a = chain(1, vec![(3, hash(7))]);
        let b = chain(2, vec![(3, hash(7))]);
        let c = certify(&[a.clone(), b.clone()]);
        assert!(!c.certified);
        assert_eq!(c.conflict, Some(BatchConflict::NativeSlot(3, hash(7))));
        assert!(!compatible(&[a], &b));
    }

    #[test]
    fn a_singleton_and_an_empty_batch_are_trivially_certified() {
        assert!(certify(&[]).certified);
        assert!(certify(&[chain(1, vec![])]).certified);
    }
}
