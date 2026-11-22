//! System deploy types + concrete deploys (port of `casper/rholang/types/SystemDeploy*.scala`
//! and `casper/rholang/sysdeploys/`).

use std::collections::{BTreeMap, BTreeSet};

use rchain_block_storage::dag::finalizer::NoAdvance;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::public_key::PublicKey;
use rchain_models::ast::Par;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_metadata::SlashSeverity;
use rchain_models::casper::protocol::casper_message::Event;
use rchain_models::rholang::RhoType::{RhoBoolean, RhoString, RhoTupleN};
use rchain_models::validator::Validator;
use rchain_shared::refined::{BlockHeight, NonNegI64};

/// A user-level system-deploy error (port of `SystemDeployUserError`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemDeployUserError(pub String);

/// A fatal platform failure (port of `SystemDeployPlatformFailure`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemDeployPlatformFailure {
    UnexpectedResult(Vec<Par>),
    UnexpectedSystemErrors(String),
    GasRefundFailure(String),
    ConsumeFailed,
}

/// Accumulated deploy events + mergeable channels (port of `EvalCollector`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvalCollector {
    pub event_log: Vec<Event>,
    pub mergeable_channels: BTreeSet<Par>,
}

impl EvalCollector {
    pub fn add(&self, log: &[Event], merge_chs: &BTreeSet<Par>) -> EvalCollector {
        let mut event_log = self.event_log.clone();
        event_log.extend(log.iter().cloned());
        let mut mergeable_channels = self.mergeable_channels.clone();
        mergeable_channels.extend(merge_chs.iter().cloned());
        EvalCollector {
            event_log,
            mergeable_channels,
        }
    }
}

/// The outcome of playing a system deploy (port of `SystemDeployResult`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemDeployResult<A> {
    PlaySucceeded {
        /// The post-state the deploy produced — typed (deferred item 1b) so the one thing every
        /// caller does with it (hand it to the next step as a state to open) cannot be given 31
        /// bytes.
        state_hash: StateHash,
        event_log: Vec<Event>,
        mergeable_channels: BTreeMap<rchain_crypto::hash::blake2b256_hash::Blake2b256Hash, i64>,
        result: A,
    },
    PlayFailed {
        event_log: Vec<Event>,
        error_msg: String,
    },
}

/// A system deploy: the rholang source plus its normalizer environment (port of `SystemDeploy`).
///
/// Rust-first: the pre-charge/refund/close-block/slash system deploys are now **native** operations
/// (see [`NativeSystemDeployOp`]); `source`/`normalizer_env`/`return_channel` remain only for the
/// legacy rholang path, which is no longer constructed.
pub struct SystemDeploy {
    pub source: &'static str,
    pub normalizer_env: BTreeMap<String, Par>,
    pub rand: Blake2b512Random,
    pub return_channel: Par,
    /// A native system-deploy operation; `Some` makes this deploy bypass the rholang source path.
    pub op: Option<NativeSystemDeployOp>,
}

/// A native system-deploy operation (rust-first replacement for the rholang PoS/registry
/// system-deploy sources; the Scala sources are a checklist only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeSystemDeployOp {
    /// **`amount` is `NonNegI64`, and that is the fix for AUDIT C109, not decoration.** A charge is a
    /// debit subtracted from the deployer's vault, so a *negative* amount inverts it into a credit —
    /// `balance - (-n) == balance + n` — while `credit_pos_vault` no-ops on the negative, leaving the
    /// staking vault untouched. The deployer is simply up by `n`, from nothing. The ingress guard
    /// caught the deploy path; this type is what makes the block-validation and replay paths
    /// (which read a *peer's* `ProcessedSystemDeploy` off the wire) structurally unable to carry one.
    PreCharge {
        deployer: PublicKey,
        amount: NonNegI64,
    },
    /// `Pos.rhox`'s `refundDeploy` is called with only the amount and reads the deployer back out of
    /// the `currentDeployerData` cell that `chargeDeploy` filled. This port carries the deployer in
    /// the deploy instead — same rule, stated in the type rather than in a mutable cell.
    Refund {
        deployer: PublicKey,
        amount: NonNegI64,
    },
    /// `fringe_state_hash` is the state hash of the **last finalised fringe** as of this block, and it
    /// is the entropy the *next* epoch's active-set draw is anchored to (`rholang/src/native_state.rs`'s
    /// `close_block`, step 5). It is neither the deploy's `rand` — `hash(shard_id, block_number, sender,
    /// pre_state_hash)` computed at the moment of use, which a proposer rerolls freely by proposing a
    /// different block — nor the block's pre-state, which a proposer influences through its
    /// justification set. The fringe is the >2/3-agreed object, so a lone proposer does not move it.
    /// Nothing is published and nothing is verified: every node derives it from its own DAG.
    CloseBlock {
        block_number: i64,
        fringe_state_hash: Blake2b256Hash,
        /// **Where each validator's latest message sat in that same fringe** — the participation the
        /// epoch reward's absence rule reads. A validator the map does not mention is one the fringe does
        /// not contain at all, which is as absent as it gets.
        ///
        /// Carried for the reason the state hash above is: not published, recomputed by every node, so
        /// play and replay agree by construction rather than by a check. What it replaces is the same
        /// signal read from the block's **pre-state** (`pos:last_spoke`), which is proposer-steerable —
        /// a pre-state is a function of the justification set, and nothing requires a block to justify
        /// everything it has seen. That is exactly the hole `close_block` step 5 documents for the seed.
        participation: BTreeMap<Validator, BlockHeight>,
        /// **The finality gate's own refusal for this block's parents** — the senders whose messages
        /// did not cover the whole partition (`NoAdvance::Support.not_full`; the coverage arm is
        /// absence by height and stays the jail's business). The boundary's inactivity leak reads it;
        /// a validator named here on `liveness_jail_after_boundaries` consecutive boundaries forfeits
        /// its bond and leaves the quorum, which is what lets the survivors finalise past an
        /// exact-two-thirds layer.
        ///
        /// Derived from the same merge the pre-state came from and, like `participation`, **not
        /// published**: a proposer-chosen set would let a block author retire a rival at a boundary,
        /// so every node recomputes it from its own DAG and the two paths agree by construction.
        layer_missing: BTreeSet<Validator>,
    },
    Slash {
        validator: Validator,
        /// **How much of the victim's holding this slash takes** (AUDIT C199). Read from the block's
        /// own `SystemDeployData::Slash`, so play and replay size the confiscation identically and a
        /// receiving node can check the tier against its own derivation.
        severity: SlashSeverity,
        /// **Equivocation evidence** (AUDIT C200), carried through to the recorded state so a receiver
        /// can check the offence against its own DAG. `None` is the metadata-justified kind.
        evidence: Option<Vec<u8>>,
    },
    /// **Pay the block's producer for the work it did** (B2, #150): a share of what this deploy
    /// burned, taken out of the staking vault by [`NativeSystemState::pay_executor`].
    ///
    /// The op carries the *whole* burned amount and the native state applies the share from its own
    /// `PosParams` — so the percentage lives in consensus state rather than in each caller, and the
    /// caller cannot pay a different fraction than the network's genesis named.
    ///
    /// The executor is a `PublicKey` and not a `Validator` because it is the block's **sender** field,
    /// which is a public key; a validator id would be a different value that happens to be the same
    /// length, and the address the payment goes to is derived from the key.
    PayExecutor {
        executor: PublicKey,
        burned: NonNegI64,
    },
}

/// **The senders a finality refusal names as not covering the whole partition** — the input the
/// boundary's inactivity leak reads. `Support.not_full` names the justification senders whose
/// candidates the partition did not all see: validators that *spoke* but whose message does not
/// justify everything the partition holds, which is the one silence nothing else prices.
///
/// **`Coverage.missing` is deliberately not included.** It names partition members with *no*
/// message in the region — absence by height — and that condition is already handled by the live
/// weight set (a silent validator drops out of the partition within `LIVENESS_WINDOW` heights) and by
/// the bond-preserving jail. Folding it in would make the leak an automatic bond burn for absence,
/// which is the policy this instrument does not adopt.
///
/// One function for play and replay, so the two cannot disagree about which refusal is meant: the
/// caller passes the `NoAdvance` its own merge derived.
pub fn layer_missing_from_stall(stall: &Option<NoAdvance<Validator>>) -> BTreeSet<Validator> {
    match stall {
        Some(NoAdvance::Support { not_full, .. }) => not_full.clone(),
        _ => BTreeSet::new(),
    }
}

impl SystemDeploy {
    pub fn pre_charge(amount: NonNegI64, pk: &PublicKey, rand: Blake2b512Random) -> SystemDeploy {
        SystemDeploy {
            source: "",
            normalizer_env: BTreeMap::new(),
            rand,
            return_channel: Par::default(),
            op: Some(NativeSystemDeployOp::PreCharge {
                deployer: pk.to_owned(),
                amount,
            }),
        }
    }

    pub fn refund(deployer: &PublicKey, amount: NonNegI64, rand: Blake2b512Random) -> SystemDeploy {
        SystemDeploy {
            source: "",
            normalizer_env: BTreeMap::new(),
            rand,
            return_channel: Par::default(),
            op: Some(NativeSystemDeployOp::Refund {
                deployer: deployer.to_owned(),
                amount,
            }),
        }
    }

    /// **Pay the block's producer** (B2, #150). Built by both folds from the block's own `sender` and
    /// the deploy's burned amount, so play and replay pay the same address the same amount; the share
    /// itself is applied by the native state from `PosParams`.
    pub fn pay_executor(
        executor: &PublicKey,
        burned: NonNegI64,
        rand: Blake2b512Random,
    ) -> SystemDeploy {
        SystemDeploy {
            source: "",
            normalizer_env: BTreeMap::new(),
            rand,
            return_channel: Par::default(),
            op: Some(NativeSystemDeployOp::PayExecutor {
                executor: executor.to_owned(),
                burned,
            }),
        }
    }

    /// A close whose gate refusal is empty — the historical shape, kept for callers that do not run a
    /// finality gate of their own (unit rigs and reports). Consensus paths use
    /// [`Self::close_block_attributing`], which states the refusal rather than defaulting it.
    pub fn close_block(
        block_number: i64,
        fringe_state_hash: Blake2b256Hash,
        participation: BTreeMap<Validator, BlockHeight>,
        rand: Blake2b512Random,
    ) -> SystemDeploy {
        Self::close_block_attributing(
            block_number,
            fringe_state_hash,
            participation,
            BTreeSet::new(),
            rand,
        )
    }

    /// **A close carrying the finality gate's own refusal** — the input the boundary's inactivity
    /// leak reads. Both consensus paths build the close with this: the proposer from its pre-state's
    /// `finality_stall`, the replayer from the same refusal its own merge derived.
    pub fn close_block_attributing(
        block_number: i64,
        fringe_state_hash: Blake2b256Hash,
        participation: BTreeMap<Validator, BlockHeight>,
        layer_missing: BTreeSet<Validator>,
        rand: Blake2b512Random,
    ) -> SystemDeploy {
        SystemDeploy {
            source: "",
            normalizer_env: BTreeMap::new(),
            rand,
            return_channel: Par::default(),
            op: Some(NativeSystemDeployOp::CloseBlock {
                block_number,
                fringe_state_hash,
                participation,
                layer_missing,
            }),
        }
    }

    pub fn slash(
        validator: &Validator,
        severity: SlashSeverity,
        evidence: Option<Vec<u8>>,
        rand: Blake2b512Random,
    ) -> SystemDeploy {
        SystemDeploy {
            source: "",
            normalizer_env: BTreeMap::new(),
            rand,
            return_channel: Par::default(),
            op: Some(NativeSystemDeployOp::Slash {
                validator: *validator,
                severity,
                evidence,
            }),
        }
    }
}

/// Interpret the `(Bool, Either[String, Nil])` result of the charge/refund/close/slash deploys
/// (port of their shared `processResult`).
///
/// The `Either[String, Nil]` is a bare `GString` on `Left` (the error message) or `Nil` on `Right`;
/// a `(true, _)` result succeeds, `(false, Left(msg))` fails with `msg`, and anything else fails
/// with `<no cause>`.
pub fn process_bool_result(output: &Par) -> Result<(), SystemDeployUserError> {
    let parts = RhoTupleN::unapply(output)
        .ok_or_else(|| SystemDeployUserError("<no cause>".to_string()))?;
    let success = parts
        .first()
        .and_then(RhoBoolean::unapply)
        .ok_or_else(|| SystemDeployUserError("<no cause>".to_string()))?;
    if success {
        return Ok(());
    }
    let error = parts
        .get(1)
        .and_then(RhoString::unapply)
        .map(|s| SystemDeployUserError(s.to_string()))
        .unwrap_or_else(|| SystemDeployUserError("<no cause>".to_string()));
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `NonNegI64` for test amounts — the charge path takes the refinement since AUDIT C109.
    fn nn(v: i64) -> NonNegI64 {
        NonNegI64::try_from(v).expect("a test amount is non-negative")
    }

    #[test]
    fn pre_charge_is_native() {
        let pk = PublicKey::new(vec![1u8; 65]);
        let rand = Blake2b512Random::new_random(128);
        let d = SystemDeploy::pre_charge(nn(100), &pk, rand);
        assert_eq!(
            d.op,
            Some(NativeSystemDeployOp::PreCharge {
                deployer: pk,
                amount: nn(100)
            })
        );
    }

    /// **The leak's input is the not-full set, and only that.** A support refusal names the senders
    /// whose messages did not cover the partition; a coverage refusal names absent senders, and
    /// folding those in would make the leak an automatic bond burn for absence — the jail's job, and
    /// the shape the policy explicitly did not adopt. An advance names nobody.
    #[test]
    fn the_leaks_input_is_the_gates_not_full_set() {
        let v1 = Validator::new([1u8; 65]);
        let v2 = Validator::new([2u8; 65]);
        let support = Some(NoAdvance::Support {
            supporting: 200,
            total: 300,
            full_partitions: 3,
            candidates: 4,
            partition: 4,
            not_full: BTreeSet::from([v1]),
        });
        assert_eq!(layer_missing_from_stall(&support), BTreeSet::from([v1]));
        let coverage = Some(NoAdvance::Coverage {
            missing: BTreeSet::from([v2]),
            senders: BTreeSet::from([v1]),
            partition: 2,
        });
        assert!(
            layer_missing_from_stall(&coverage).is_empty(),
            "absence by height is the live weight set's and the jail's, not the leak's"
        );
        assert!(layer_missing_from_stall(&Some(NoAdvance::AlreadyPublished)).is_empty());
        assert!(layer_missing_from_stall(&None).is_empty());
    }

    /// The refund carries its payer. The Scala contract reads the deployer back out of the
    /// `currentDeployerData` cell that `chargeDeploy` filled (`Pos.rhox:425`), because its
    /// `refundDeploy` is called with only the amount; here the payer is a field of the deploy, so a
    /// refund cannot be paid to anyone but the account the pre-charge took the phlo from.
    #[test]
    fn refund_carries_its_payer() {
        let pk = PublicKey::new(vec![2u8; 65]);
        let rand = Blake2b512Random::new_random(128);
        let d = SystemDeploy::refund(&pk, nn(70), rand);
        assert_eq!(
            d.op,
            Some(NativeSystemDeployOp::Refund {
                deployer: pk,
                amount: nn(70)
            })
        );
    }

    #[test]
    fn process_bool_result_interprets_tuple() {
        use rchain_models::rholang::RhoType::{RhoBoolean, RhoNil, RhoString, RhoTupleN};

        // (true, _) succeeds regardless of the Either.
        let ok = RhoTupleN::apply(vec![RhoBoolean::apply(true), RhoNil::apply()]);
        assert_eq!(process_bool_result(&ok), Ok(()));

        // (false, Left("boom")) fails with the message.
        let fail = RhoTupleN::apply(vec![
            RhoBoolean::apply(false),
            RhoString::apply("boom".to_string()),
        ]);
        assert_eq!(
            process_bool_result(&fail),
            Err(SystemDeployUserError("boom".to_string()))
        );

        // (false, Right(Nil)) fails with no cause.
        let fail_nil = RhoTupleN::apply(vec![RhoBoolean::apply(false), RhoNil::apply()]);
        assert_eq!(
            process_bool_result(&fail_nil),
            Err(SystemDeployUserError("<no cause>".to_string()))
        );

        // A malformed result fails with no cause.
        assert_eq!(
            process_bool_result(&Par::default()),
            Err(SystemDeployUserError("<no cause>".to_string()))
        );
    }
}
