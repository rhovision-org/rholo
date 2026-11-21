//! Block creation (port of `blocks/proposer/BlockCreator.scala`).

use std::collections::{BTreeMap, BTreeSet};

use rchain_block_storage::dag::dag_storage::{BlockDagStorage, DeployId};
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_version::version_at;
use rchain_models::casper::protocol::casper_message::{
    ProcessedDeploy, ProcessedSystemDeploy, RholangState, SignedDeployData,
};
use rchain_models::validator::Validator;
use rchain_rholang::system_processes::BlockData;
use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};
use rchain_shared::time::current_millis;

use crate::block_random_seed::BlockRandomSeed;
use crate::blocks::proposer::proposer::ProposedSlash;
use crate::interpreter_util::compute_deploys_checkpoint;
use crate::merging::ParentsMergedState;
use crate::proto_util::unsigned_block_proto;
use crate::rholang::{SystemDeployRuntimeResult, UserDeployRuntimeResult};
use crate::runtime_manager::RuntimeManager;
use crate::system_deploy::SystemDeploy;
use crate::validator_identity::ValidatorIdentity;

use super::propose_result::BlockCreatorResult;

/// A block creator: validator identity + shard (port of `BlockCreator`).
#[derive(Clone, Debug)]
pub struct BlockCreator {
    pub id: ValidatorIdentity,
    pub shard_id: String,
}

type StateTransitionResult = (
    Blake2b256Hash,
    Vec<UserDeployRuntimeResult>,
    Vec<SystemDeployRuntimeResult>,
);

/// **The own-sequence decision** (Law 122), pure so the boundary is testable without a runtime.
///
/// `durable` is the highest `(height, seq)` this validator has signed, from the DAG's durable
/// record; `snapshot` is the sequence the round snapshot holds for this validator. The proposer may
/// proceed only when the snapshot has caught up to the record — otherwise its next number would
/// duplicate a signature the network already has, and the network slashes a duplicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnSeqDecision {
    Proceed,
    Wait {
        signed: i64,
        signed_height: i64,
        snapshot: Option<i64>,
    },
}

pub(crate) fn own_seq_decision(
    durable: Option<(BlockHeight, SeqNum)>,
    snapshot: Option<SeqNum>,
) -> OwnSeqDecision {
    match durable {
        None => OwnSeqDecision::Proceed,
        Some((height, seq)) => {
            if snapshot.is_some_and(|seen| seq <= seen) {
                OwnSeqDecision::Proceed
            } else {
                OwnSeqDecision::Wait {
                    signed: i64::from(seq),
                    signed_height: i64::from(height),
                    snapshot: snapshot.map(i64::from),
                }
            }
        }
    }
}

impl BlockCreator {
    /// Create (and sign) a block from the merged pre-state + a set of pooled deploys (port of
    /// `BlockCreator.create`).
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        runtime: &RuntimeManager,
        dag: &dyn BlockDagStorage,
        pre_state: &ParentsMergedState,
        deploys: &[DeployId],
        to_slash: &BTreeMap<Validator, ProposedSlash>,
        change_epoch: bool,
        suppress_attestation: bool,
        batch_limit: Option<usize>,
    ) -> Result<BlockCreatorResult, String> {
        let pre_state_hash = pre_state.pre_state_hash;
        let parents: Vec<BlockHash> = pre_state
            .justifications
            .iter()
            .map(|m| m.block_hash)
            .collect();
        let block_num = pre_state
            .justifications
            .iter()
            .map(|m| m.block_num)
            .max()
            .map(|m| m + NonNegI64::one())
            .unwrap_or_else(BlockHeight::zero);
        let creators_pk = self.id.public_key.clone();
        let creators_validator = Validator::from_slice(creators_pk.bytes());
        let snapshot_seq = pre_state
            .justifications
            .iter()
            .find(|m| m.sender == creators_validator)
            .map(|m| m.seq_num);
        // **The own-sequence floor** (Law 122; RC-4 HAZOP J8). The snapshot names the last sequence
        // this key signed *that the DAG still holds*. A rewind or a light join can drop the block
        // that carried a later one, and deriving from the snapshot alone then signs a duplicate —
        // which the network, correctly, reads as an equivocation and slashes (measured 2026-11-21:
        // a rejoined validator re-signed sequence 152 and lost its bond). The durable record is the
        // one memory that survives the rewind; while it is ahead, the node waits.
        let durable = dag.read_own_latest().await?;
        if let OwnSeqDecision::Wait {
            signed,
            signed_height,
            snapshot,
        } = own_seq_decision(durable.map(|(height, seq, _)| (height, seq)), snapshot_seq)
        {
            return Ok(BlockCreatorResult::OwnSeqNotReconciled {
                signed,
                signed_height,
                snapshot,
            });
        }
        let seq_num = snapshot_seq
            .map(|seq| seq + NonNegI64::one())
            .unwrap_or_else(SeqNum::zero);
        // Informational block timestamp: the proposer's wall clock, chosen once and used both for
        // the block header and for `rho:block:data` during evaluation, so a contract that reads it
        // sees exactly the value a replayer will (determinism).
        let block_timestamp = current_millis();
        let block_data = BlockData {
            block_number: block_num,
            sender: creators_pk.clone(),
            seq_num,
            timestamp: block_timestamp,
        };
        let should_propose = !deploys.is_empty() || !to_slash.is_empty() || change_epoch;
        let finalization = pre_state.fringe_rejected_deploys.clone();

        let post_state: Option<StateTransitionResult> = if should_propose {
            let rand = BlockRandomSeed::random_generator_from(
                &self.shard_id,
                i64::from(block_num),
                creators_pk.clone(),
                pre_state_hash,
            )
            .map_err(|e| e.to_string())?;

            // Pooled deploys filtered to the selected ids — computed FIRST so the slash/close seed
            // index below reflects the ACTUAL deploy count (`selected.len()`, matching replay's
            // `terms.len()`), not the requested id list (`deploys.len()`) which can over-count if a
            // requested deploy is absent from the pool (S2/S4).
            let pooled = dag.pooled_deploys().await?;
            let deploy_set: BTreeSet<&DeployId> = deploys.iter().collect();
            let selected: Vec<SignedDeployData> = pooled
                .into_iter()
                .filter(|(id, _)| deploy_set.contains(id))
                .map(|(_, d)| d)
                .collect();

            // Slash + close-block system deploys, in the one order the replay will read them in.
            let system_deploys = block_system_deploys(
                &to_slash,
                selected.len(),
                i64::from(block_num),
                pre_state.fringe_state,
                pre_state.participation.clone(),
                &rand,
            )?;

            Some(
                compute_deploys_checkpoint(
                    runtime,
                    &selected,
                    &system_deploys,
                    &rand,
                    block_data,
                    &pre_state_hash,
                    // The fringe the new block extends, which is what its close deploy anchors the
                    // *next* epoch's active-set seed to. Derived here from the DAG, not carried on the
                    // block: every node computes it for itself, including the replayer.
                    &pre_state.fringe_state,
                    batch_limit,
                )
                .await?,
            )
        } else if !suppress_attestation {
            // Attestation: empty state transition over the pre-state.
            Some((pre_state_hash, Vec::new(), Vec::new()))
        } else {
            None
        };

        match post_state {
            None => Ok(BlockCreatorResult::NoNewDeploys),
            Some((post_state_hash, user_results, sys_results)) => {
                let processed_deploys: Vec<ProcessedDeploy> =
                    user_results.into_iter().map(|r| r.deploy).collect();
                let processed_system_deploys: Vec<ProcessedSystemDeploy> =
                    sys_results.into_iter().map(|r| r.deploy).collect();
                let state = RholangState {
                    deploys: processed_deploys,
                    system_deploys: processed_system_deploys,
                };
                // The block's bond cache is the *active* PoS set at the block's post-state. This is
                // what `Validate::bonds_cache` recomputes, and it is what lets a block change the
                // validator pool (bond/withdraw/slash) without the block being rejected.
                let bonds_map = runtime
                    .compute_bonds(&StateHash::from_slice(post_state_hash.as_bytes()))
                    .await?;
                let unsigned_block = unsigned_block_proto(
                    // The version scheduled for this height, which is what `Validate::block_version`
                    // demands of it — so a release carrying a new row switches proposers over at
                    // the row's height, with no flag to set.
                    version_at(i64::from(block_num)),
                    self.shard_id.clone(),
                    block_num,
                    creators_validator,
                    seq_num,
                    pre_state_hash.into(),
                    post_state_hash.into(),
                    parents,
                    bonds_map,
                    finalization,
                    state,
                    block_timestamp,
                );
                let signed_block = self
                    .id
                    .sign_block(&unsigned_block)
                    .map_err(|e| e.to_string())?;
                // **The record is written before the block leaves this function** (Law 122). The
                // proposer validates and broadcasts what is returned, so a write failure refuses
                // the proposal rather than allowing a signature the node cannot remember. The
                // block's hash travels with it: the boot reconciles by asking peers for exactly
                // this block, and can retire the record when no peer serves it.
                dag.write_own_latest(block_num, seq_num, signed_block.block_hash)
                    .await?;
                Ok(BlockCreatorResult::Created(signed_block))
            }
        }
    }
}

/// **The block-level system deploys a proposer attaches, in the order they are recorded.**
///
/// A function rather than an inline block, for the reason `slashable_offenders` is one: the *list* is
/// what consensus turns on, and it was reachable only through `create_block` — which needs a runtime, a
/// DAG and a signing identity, so nothing in the tree pinned it. That was a real gap, and the entry it
/// hid was the proposer's own activity record (see `spec/audit/passes.md` §55).
///
/// **The order is load-bearing, not cosmetic.** The replay rebuilds each entry's random seed from its
/// *position* in the recorded list (`runtime_replay.rs`'s `replay_deploys`:
/// `rand.split_byte(terms.len() + i)`), and this side derives the same seed from the same position
/// (`deploy_count + i`). The two lists agree because both are this one, in this order — so an entry
/// inserted anywhere but the end moves every seed after it, and `deploy_count` is the *selected* count
/// rather than the requested one for the same reason (a requested deploy absent from the pool would
/// otherwise shift them all; S2/S4).
///
/// **Two entries now, and the first one used to be the activity record** (B4, #150). Retiring
/// `RecordSpoke` shifted every seed in this list down by one, which is a consensus change rather than a
/// tidy-up — it is the same class as an insertion, just in the other direction — and it is why the
/// positional rule above is spelled out rather than assumed.
///
/// Each entry is a rule: one `Slash` per offender in `to_slash`'s canonical `BTreeMap` order (C110,
/// C199, C200), then the `CloseBlock` that carries the **fringe's** state hash and the participation
/// derived from it — the two inputs here that are not the proposer's to choose (O1).
fn block_system_deploys(
    to_slash: &BTreeMap<Validator, ProposedSlash>,
    deploy_count: usize,
    block_number: i64,
    fringe_state: Blake2b256Hash,
    participation: BTreeMap<Validator, BlockHeight>,
    rand: &Blake2b512Random,
) -> Result<Vec<SystemDeploy>, String> {
    let mut system_deploys: Vec<SystemDeploy> = Vec::new();
    for (i, (v, slash)) in to_slash.iter().enumerate() {
        system_deploys.push(SystemDeploy::slash(
            v,
            slash.severity,
            slash.evidence.clone(),
            seed_at(rand, deploy_count + i)?,
        ));
    }
    system_deploys.push(SystemDeploy::close_block(
        block_number,
        fringe_state,
        participation,
        seed_at(rand, deploy_count + to_slash.len())?,
    ));
    Ok(system_deploys)
}

/// The seed for the entry at `position` of the block-level list — the one derivation both this side and
/// the replay use, in one place so the two cannot drift apart.
fn seed_at(rand: &Blake2b512Random, position: usize) -> Result<Blake2b512Random, String> {
    Ok(rand.split_byte(u8::try_from(position).map_err(|e| e.to_string())?))
}

#[cfg(test)]
mod block_system_deploy_tests {
    use super::{block_system_deploys, own_seq_decision, OwnSeqDecision};
    use crate::blocks::proposer::proposer::ProposedSlash;
    use crate::system_deploy::NativeSystemDeployOp;
    use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
    use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
    use rchain_models::block_metadata::SlashSeverity;
    use rchain_models::validator::Validator;
    use rchain_shared::refined::{BlockHeight, SeqNum};
    use std::collections::BTreeMap;

    fn offender(
        byte: u8,
        tier: SlashSeverity,
        evidence: Option<Vec<u8>>,
    ) -> (Validator, ProposedSlash) {
        (
            Validator::new([byte; 65]),
            ProposedSlash {
                severity: tier,
                evidence,
            },
        )
    }

    /// **The list is the block's own record first, its slashes next, and the close last** — and every
    /// entry's seed is the one the *replay* will derive from its position.
    ///
    /// The seed arm is the one that cannot be replaced by reading the code twice: `replay_deploys`
    /// assigns `rand.split_byte(terms.len() + i)` by position, so this test derives exactly that from
    /// the same base rand and requires the proposer to have done the same. Inserting an entry, or
    /// starting `deploy_count` from the *requested* rather than the selected count, turns it red.
    #[test]
    fn the_proposer_attaches_the_list_the_replay_will_read() {
        let rand = Blake2b512Random::from_init(&[7u8; 32]);
        let fringe = Blake2b256Hash::from_bytes([9u8; 32]);
        let to_slash = BTreeMap::from([
            offender(2, SlashSeverity::Malicious, Some(vec![1, 2, 3])),
            offender(1, SlashSeverity::HonestMistake, None),
        ]);
        let deploy_count = 4usize;
        let deploys =
            block_system_deploys(&to_slash, deploy_count, 12, fringe, BTreeMap::new(), &rand)
                .expect("a list of system deploys");

        // Three entries: two slashes and the close, in that order. It was four until `RecordSpoke` was
        // retired, and the shift is the point of the seed arm below rather than a detail of this one.
        assert_eq!(
            deploys.len(),
            3,
            "two offenders and the close that pays them out"
        );
        // `to_slash` is a `BTreeMap`, so validator 1 precedes validator 2 whatever order it was built
        // in — which is the property the seed indices depend on.
        assert_eq!(
            deploys[0].op,
            Some(NativeSystemDeployOp::Slash {
                validator: Validator::new([1u8; 65]),
                severity: SlashSeverity::HonestMistake,
                evidence: None,
            }),
            "the first entry is now the first slash, where the retired record used to be"
        );
        assert_eq!(
            deploys[1].op,
            Some(NativeSystemDeployOp::Slash {
                validator: Validator::new([2u8; 65]),
                severity: SlashSeverity::Malicious,
                evidence: Some(vec![1, 2, 3]),
            }),
            "the tier and the evidence travel with the slash"
        );
        assert_eq!(
            deploys[2].op,
            Some(NativeSystemDeployOp::CloseBlock {
                block_number: 12,
                fringe_state_hash: fringe,
                participation: BTreeMap::new(),
            }),
            "and the close carries the fringe's own state hash, which is not the proposer's to choose"
        );

        // **The positional seeds.** This is `replay_deploys`'s derivation, spelled the same way — and
        // it is what catches the retirement: with the entry gone every seed in the list moved down by
        // one, and a proposer that kept the old offsets and a replay that used the new ones would agree
        // about nothing.
        for (i, deploy) in deploys.iter().enumerate() {
            let expected =
                rand.split_byte(u8::try_from(deploy_count + i).expect("a test position"));
            assert_eq!(
                deploy.rand.to_bytes(),
                expected.to_bytes(),
                "entry {i} must carry the seed the replay assigns to position {i}"
            );
        }
    }

    /// A block with nothing to slash still closes — and closing is now the *only* entry that is always
    /// there, since the block's own activity record is retired.
    #[test]
    fn a_block_with_no_offenders_still_closes() {
        let rand = Blake2b512Random::from_init(&[1u8; 32]);
        let deploys = block_system_deploys(
            &BTreeMap::new(),
            0,
            1,
            Blake2b256Hash::from_bytes([0u8; 32]),
            BTreeMap::new(),
            &rand,
        )
        .expect("a list of system deploys");
        assert_eq!(deploys.len(), 1);
        assert!(matches!(
            deploys[0].op,
            Some(NativeSystemDeployOp::CloseBlock { .. })
        ));
    }

    /// **The own-sequence floor is the drill's own record** (Law 122, RC-4 HAZOP J8). The pre-fix
    /// derivation — the snapshot alone — signs sequence 152 again from a snapshot stuck at 151; the
    /// durable record refuses and names what was signed. At the boundary, where the catch-up has
    /// re-adopted the block, the node proceeds.
    #[test]
    fn the_own_sequence_record_refuses_a_sequence_already_signed() {
        let height = |v: i64| BlockHeight::try_from(v).expect("non-negative");
        let seq = |v: i64| SeqNum::try_from(v).expect("non-negative");
        // No record: a node that has never signed here derives from the snapshot as before.
        assert_eq!(
            own_seq_decision(None, Some(seq(151))),
            OwnSeqDecision::Proceed
        );
        // The record is ahead of the snapshot: the block that carried 152 was dropped by a rewind.
        assert_eq!(
            own_seq_decision(Some((height(155), seq(152))), Some(seq(151))),
            OwnSeqDecision::Wait {
                signed: 152,
                signed_height: 155,
                snapshot: Some(151),
            }
        );
        // The record with no own entry in the snapshot is the same hazard, named.
        assert_eq!(
            own_seq_decision(Some((height(155), seq(152))), None),
            OwnSeqDecision::Wait {
                signed: 152,
                signed_height: 155,
                snapshot: None,
            }
        );
        // The catch-up re-adopted the block: the snapshot reaches the record and the node signs 153.
        assert_eq!(
            own_seq_decision(Some((height(155), seq(152))), Some(seq(152))),
            OwnSeqDecision::Proceed
        );
    }
}
