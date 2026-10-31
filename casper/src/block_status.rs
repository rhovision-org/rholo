//! Block validation status (port of `BlockStatus.scala`).

use rchain_models::block_metadata::FailureCause;
use rchain_models::block_metadata::SlashSeverity;

/// The outcome of validating a block (port of `BlockStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockStatus {
    Valid,
    InvalidBlockNumber,
    InvalidRepeatDeploy,
    InvalidSequenceNumber,
    InvalidDeployShardId,
    JustificationRegression,
    NeglectedInvalidBlock,
    InvalidStateHash,
    /// **The block's declared `pre_state_hash` is not the pre-state this node computed** (#139).
    ///
    /// Its own status because the two hash mismatches were one status and are two different facts. A
    /// **pre**-state mismatch says this node's merge over the block's justifications produced a
    /// different state — the block's justification set, or this node's view of the parents, differs.
    /// A **post**-state mismatch (`InvalidStateHash`) says the merge agreed and the *replay* then
    /// diverged, which points at a deploy or at an input the replay derives locally rather than at the
    /// block's parents. Until this variant existed, an operator reading
    /// `Block <hex> failed validation: InvalidStateHash` could not tell which of the two had happened,
    /// which is exactly what issue #139 has to name.
    InvalidPreStateHash,
    InvalidBondsCache,
    InvalidRejectedDeploy,
    ContainsExpiredDeploy,
    ContainsFutureDeploy,
    ContainsLowCostDeploy,
    /// A deploy's phlo limit is negative (AUDIT C109). Its own status rather than folding into
    /// `ContainsLowCostDeploy`: a negative *limit* is not a cheap deploy, it is a malformed one whose
    /// charge would be a credit, and a validator that reports the two as the same thing cannot say
    /// which it rejected.
    InvalidPhloLimit,
    /// The block slashes a validator that none of its justifications holds responsible (AUDIT C110).
    ///
    /// A `Slash` is a *system* deploy: it is not signed by its victim, it moves that victim's entire
    /// bond to the Coop vault, and every validator re-executes it during replay. Until this status
    /// existed, a block was accepted on the strength of its post-state hash alone — a proposer could
    /// name any bonded validator and, provided it computed the resulting state honestly, every other
    /// validator would help it confiscate the stake. The rule is now re-derived on the receiving side
    /// from the node's own block metadata, not taken from the proposer.
    UnjustifiedSlash,
    /// A deploy in the block is not signed by the key its `deployer` field names (AUDIT C120).
    ///
    /// **Why this is a separate status and not a reuse of `InvalidRepeatDeploy`.** The replay reads the
    /// deployer out of this field to build the pre-charge, the refund and the `rho:rchain:deployerId`
    /// binding, so an unverified field is an *authorization* claim rather than a malformed one: a
    /// proposer could name any account, put arbitrary bytes in `sig`, and have every other validator
    /// debit that account and pay the proposer's term — with a post-state hash they computed honestly,
    /// so the block was valid and nothing was attributable. `verify_signature` existed and was correct;
    /// it was called only at the deploy *ingress*, never on the path a peer's block takes. A validator
    /// that reports this must be able to say that is what it rejected.
    ///
    /// `system_deploys` are exempt by construction and are not inspected: a `Slash` is unsigned by its
    /// victim (see `UnjustifiedSlash` above), and the rule that makes one legitimate is a different
    /// check entirely.
    InvalidDeploySignature,
    /// A block carries more deploys than the protocol's seed index can address (AUDIT F-3).
    ///
    /// The proposer has always bounded its *own* selection, because `close_block` indexes the deploy
    /// randomness seed in a `u8` and the deploy count plus the slash count must fit in 255. Nothing
    /// bounded what a validator would *accept*: a peer's block could carry any number of deploys and
    /// every node would replay all of them. The bound is a length, so it costs nothing to apply, and it
    /// belongs among the pre-replay checks for the same reason `phlo_price` does.
    TooManyDeploys,
    /// A block's justification set is wider than the protocol allows (#153).
    ///
    /// **The half `TooManyDeploys` does not cover, and the one that mattered more.** The deploy count
    /// has a proposer-side bound and, since AUDIT F-3, a receiving-side check. The *justification* count
    /// had neither: `max-number-of-parents` was read from configuration and consulted by no code
    /// anywhere in the tree, while two doc comments and C123's fix rationale all asserted that it
    /// bounded the set. So a proposer could hand every validator an arbitrarily wide justification set,
    /// and the merge — the one input a proposer fully controls — was paid for by everyone.
    ///
    /// The width is legitimate: the parent set is the round snapshot, one message per sender, so a
    /// bonded network produces a set of the order of its validator count. What was missing is the
    /// refusal, and a refusal of a set above a fixed width is a **consensus change**, because it
    /// rejects blocks that are valid today. It is registered as one in `spec/audit/passes.md` §6 and on
    /// #51 §A; the shipped bound is `max-number-of-parents`, defaulted to the protocol's existing
    /// per-block width rather than to `i32::MAX`.
    ///
    /// It is [`FailureCause::Attributable`]: the count is a property of the block alone, together with
    /// the DAG's structure, in the same sense and for the same reason as `TooManyDeploys`.
    TooManyJustifications,
    /// A block's total declared phlo exceeds the block budget (AUDIT F-3).
    ///
    /// Without this there is no bound on what one block costs: the per-deploy budget bounds each deploy
    /// separately, so a proposer could pack the block arbitrarily full and every validator would replay
    /// the lot. Sui and Solana both contain a block's blast radius this way; this is the port's
    /// equivalent, and it is a deliberate divergence — the Scala `blockSummary` composes no per-block
    /// bound at all.
    ExceedsBlockPhloLimit,
    /// The block's `version` field names a version this node does not support (AUDIT F-6).
    ///
    /// `validate::version` has existed since the port with `SUPPORTED = [1]`, a passing unit test, and
    /// **zero production callers**: nothing on the acceptance path read the field. A block stamped
    /// `version: 999`, self-consistently hashed and signed, satisfied every predicate. The field is
    /// inside `hash_block`'s cover, so it is consensus-visible and a future version bump would not
    /// have been enforced at all.
    InvalidVersion,
}

impl BlockStatus {
    pub fn is_valid(&self) -> bool {
        matches!(self, BlockStatus::Valid)
    }

    /// **Why** this refusal happened, which decides whether it may be attributed to the block and
    /// whether a restoring rule may later clear the record (AUDIT C173).
    ///
    /// The principle, stated once so a new status has somewhere to be classified rather than
    /// somewhere to be guessed: a refusal is [`FailureCause::Attributable`] when it follows from the
    /// block itself together with the DAG's **structure** (heights, justification sets, the deploys'
    /// own fields), and [`FailureCause::Divergence`] when it follows from this node's **state or
    /// replay** — the merge, the fringe, the replayed post-state, this node's own metadata. Only the
    /// first is the block's fault, because a node whose state differs would reach the second for a
    /// block that is valid everywhere else; that is the measured `InvalidStateHash` divergence on
    /// #105, and it is why the second class is the one a restore may clear.
    ///
    /// [`FailureCause::Cascade`] is the third case and it is not about this block at all: a child
    /// refused *because a justification failed*.
    ///
    /// `Valid` is never a refusal — the checks return it as `Ok(())`, not as a failure — but it is
    /// classified anyway so that adding a variant forces a decision here rather than inheriting a
    /// default.
    pub fn failure_cause(&self) -> FailureCause {
        match self {
            // Decided by this node's state or replay, so a node with a different view reaches the
            // same verdict for a block that is valid elsewhere.
            BlockStatus::InvalidStateHash
            | BlockStatus::InvalidPreStateHash
            | BlockStatus::InvalidRejectedDeploy
            | BlockStatus::InvalidBondsCache
            | BlockStatus::UnjustifiedSlash => FailureCause::Divergence,

            // Refused because a justification failed. One transient failure must not fabricate slash
            // evidence against every validator above it (#125).
            BlockStatus::NeglectedInvalidBlock => FailureCause::Cascade,

            // The block's own fault, or the structural consequence of a claim it made about the DAG.
            BlockStatus::Valid
            | BlockStatus::InvalidBlockNumber
            | BlockStatus::InvalidRepeatDeploy
            | BlockStatus::InvalidSequenceNumber
            | BlockStatus::InvalidDeployShardId
            | BlockStatus::JustificationRegression
            | BlockStatus::ContainsExpiredDeploy
            | BlockStatus::ContainsFutureDeploy
            | BlockStatus::ContainsLowCostDeploy
            | BlockStatus::InvalidPhloLimit
            | BlockStatus::InvalidDeploySignature
            | BlockStatus::TooManyDeploys
            | BlockStatus::TooManyJustifications
            | BlockStatus::ExceedsBlockPhloLimit
            | BlockStatus::InvalidVersion => FailureCause::Attributable,
        }
    }

    /// **Whether a refusal of this status may take the sender's bond** — the offence predicate, and it
    /// is deliberately **narrower than `Attributable`** (AUDIT C198).
    ///
    /// `Attributable` answers *whose fault the refusal is*, and it is the right test for blame. It is
    /// the wrong test for **slashing**, because three of its fourteen members read an input that
    /// belongs to the **receiving node** rather than to the block:
    ///
    /// * `ContainsLowCostDeploy` — this node's `casper.min-phlo-price`;
    /// * `TooManyJustifications` — this node's `casper.max-number-of-parents`;
    /// * `InvalidVersion` — this software's `SUPPORTED` list.
    ///
    /// **None of the three is genesis identity.** `GenesisBlockData` (`casper/src/conf.rs`) and
    /// `PosParams` (`rholang/src/native_state.rs`) carry neither knob, so two correctly-configured
    /// operators can hold different values and neither is wrong — there is no committed value to be
    /// wrong *about*.
    ///
    /// **And a disagreement about blame is not a tolerated divergence here — it is a chain split.**
    /// [`slash_is_unjustified`](../../../casper/src/interpreter_util.rs) refuses any block whose
    /// slashes are not a subset of what the *receiving* node re-derives from its own metadata. So
    /// node A, whose knob makes a block an offence, proposes `Slash(X)`; node B, whose knob does not,
    /// finds it unjustified and refuses A's block — permanently. A staker then loses its whole bond
    /// for a disagreement about a local setting, which is the case this predicate exists to remove.
    ///
    /// So an offence needs inputs that are **the same on every node running this rule version**: the
    /// block's own signed fields, the DAG's structure, and the **version-coupled constants**
    /// (`DEPLOY_LIFESPAN`, `MAX_BLOCK_DEPLOYS`, `MAX_BLOCK_PHLO`). The constants are shared by one
    /// *binary* rather than by one *chain*, and they are a lockstep-upgrade concern rather than a
    /// per-operator one — the deviation register names them as coupled to the rule version.
    pub fn is_slashing_offence(&self) -> bool {
        self.slash_severity().is_some()
    }

    /// **How much of what the sender holds this refusal may take** — the tier, with `None` for the
    /// refusals that are not offences at all (AUDIT C199).
    ///
    /// [`Self::is_slashing_offence`] is this predicate's `is_some`, so the two can never disagree; the
    /// tier is what the slash is *sized* by, and it is graded by the character of the offence rather
    /// than by the size of the bond, because a rule that always took everything made an operator's
    /// worst case total — and a stake nobody is willing to bond is a consensus problem, not a
    /// disciplinary one.
    ///
    /// **The three tiers, and the argument for each boundary:**
    ///
    /// * [`SlashSeverity::Malicious`] — the block asserts something **forged**: a deploy whose
    ///   signature does not verify against the key it names is an impersonation, and nothing accidental
    ///   produces one. (Equivocation joins this tier when it becomes slashable at all — today the H-1
    ///   gate refuses it and it never reaches a status.)
    /// * [`SlashSeverity::Misdemeanour`] — a rule the author's **own block** breaks with something to
    ///   gain: a DAG position it is not entitled to (`InvalidBlockNumber`, `InvalidSequenceNumber`,
    ///   `JustificationRegression`), a deploy it may not carry (`InvalidRepeatDeploy`,
    ///   `InvalidDeployShardId`, `InvalidPhloLimit`).
    /// * [`SlashSeverity::HonestMistake`] — a **bound** crossed where the block is otherwise
    ///   self-consistent, which a stale deploy pool, a clock skew or a local budget error explains:
    ///   `ContainsExpiredDeploy` and `ContainsFutureDeploy` are the measured K1 class (a *client* bug
    ///   produced exactly those deploys), and `TooManyDeploys` / `ExceedsBlockPhloLimit` are the port's
    ///   own two budgets.
    ///
    /// The boundary is a judgement, and it lives in one table so a later reading can move a status
    /// without hunting the rule.
    pub fn slash_severity(&self) -> Option<SlashSeverity> {
        match self {
            BlockStatus::InvalidDeploySignature => Some(SlashSeverity::Malicious),

            BlockStatus::InvalidBlockNumber
            | BlockStatus::InvalidSequenceNumber
            | BlockStatus::JustificationRegression
            | BlockStatus::InvalidRepeatDeploy
            | BlockStatus::InvalidDeployShardId
            | BlockStatus::InvalidPhloLimit => Some(SlashSeverity::Misdemeanour),

            BlockStatus::ContainsExpiredDeploy
            | BlockStatus::ContainsFutureDeploy
            | BlockStatus::TooManyDeploys
            | BlockStatus::ExceedsBlockPhloLimit => Some(SlashSeverity::HonestMistake),

            // **`Attributable`, and yet not an offence** — each reads an input this node owns, so
            // another node can reach the opposite verdict for the same block.
            BlockStatus::ContainsLowCostDeploy
            | BlockStatus::TooManyJustifications
            | BlockStatus::InvalidVersion => None,

            // Never the block's fault (`Divergence`, `Cascade`), and `Valid` is not a refusal.
            BlockStatus::Valid
            | BlockStatus::InvalidStateHash
            | BlockStatus::InvalidPreStateHash
            | BlockStatus::InvalidRejectedDeploy
            | BlockStatus::InvalidBondsCache
            | BlockStatus::UnjustifiedSlash
            | BlockStatus::NeglectedInvalidBlock => None,
        }
    }
}

impl std::fmt::Display for BlockStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BlockStatus::Valid => "valid",
            BlockStatus::InvalidBlockNumber => "invalid block number",
            BlockStatus::InvalidRepeatDeploy => "a deploy was repeated across blocks",
            BlockStatus::InvalidSequenceNumber => "invalid sender sequence number",
            BlockStatus::InvalidDeployShardId => "deploy shard id does not match the block's shard",
            BlockStatus::JustificationRegression => "a justification regressed from its parent",
            BlockStatus::NeglectedInvalidBlock => "an invalid block was used as a justification",
            BlockStatus::InvalidStateHash => {
                "the block's declared post-state hash does not match the state recomputed by \
                 replaying its deploys — a node state-accounting inconsistency, not an error in your \
                 deploy or API call"
            }
            BlockStatus::InvalidPreStateHash => {
                "the block's declared pre-state hash is not the state this node computed from its \
                 justifications — this node's view of the parents differs from the proposer's, so the \
                 disagreement is about the merge rather than about the block's deploys"
            }
            BlockStatus::InvalidBondsCache => "invalid bonds cache",
            BlockStatus::InvalidRejectedDeploy => "the block's rejected-deploy set does not match its parents",
            BlockStatus::ContainsExpiredDeploy => "a deploy has expired",
            BlockStatus::ContainsFutureDeploy => "a deploy has a future validity window",
            BlockStatus::ContainsLowCostDeploy => "a deploy's phlo price is below the minimum",
            BlockStatus::InvalidPhloLimit => "a deploy has a negative phlo limit",
            BlockStatus::UnjustifiedSlash => "the block slashes a validator none of its justifications holds responsible",
            BlockStatus::InvalidDeploySignature => {
                "a deploy is not signed by the key its `deployer` field names, so the account this \
                 block charges — and pays — was chosen by the block's author rather than proven by a \
                 signature"
            }
            BlockStatus::TooManyDeploys => {
                "the block carries more deploys than the protocol's seed index can address"
            }
            BlockStatus::TooManyJustifications => {
                "the block justifies more parents than the protocol allows — the merge it asks every \
                 validator to pay for is wider than a bonded network can produce"
            }
            BlockStatus::ExceedsBlockPhloLimit => {
                "the block's total declared phlo exceeds the per-block budget"
            }
            BlockStatus::InvalidVersion => {
                "the block's version is not one this node supports"
            }
        };
        write!(f, "{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every status, so a variant added without a message (or with a copy-pasted one) fails here.
    const ALL: [BlockStatus; 21] = [
        BlockStatus::Valid,
        BlockStatus::InvalidBlockNumber,
        BlockStatus::InvalidRepeatDeploy,
        BlockStatus::InvalidSequenceNumber,
        BlockStatus::InvalidDeployShardId,
        BlockStatus::JustificationRegression,
        BlockStatus::NeglectedInvalidBlock,
        BlockStatus::InvalidStateHash,
        BlockStatus::InvalidPreStateHash,
        BlockStatus::InvalidBondsCache,
        BlockStatus::InvalidRejectedDeploy,
        BlockStatus::ContainsExpiredDeploy,
        BlockStatus::ContainsFutureDeploy,
        BlockStatus::ContainsLowCostDeploy,
        BlockStatus::InvalidPhloLimit,
        BlockStatus::UnjustifiedSlash,
        BlockStatus::InvalidDeploySignature,
        BlockStatus::TooManyDeploys,
        BlockStatus::TooManyJustifications,
        BlockStatus::ExceedsBlockPhloLimit,
        BlockStatus::InvalidVersion,
    ];

    /// `Valid` is the **only** status that is valid: `is_valid` is the one predicate the block
    /// processor branches on, and a non-`Valid` status that answered `true` would admit an invalid
    /// block into the DAG.
    #[test]
    fn only_the_valid_status_is_valid() {
        assert!(BlockStatus::Valid.is_valid());
        for status in ALL {
            if status != BlockStatus::Valid {
                assert!(!status.is_valid(), "{status:?} must not be valid");
            }
        }
    }

    /// Every status renders a distinct, non-empty sentence — these reach an operator through the
    /// API when a block is rejected, and two statuses sharing a message would make the rejection
    /// undiagnosable. The `InvalidStateHash` message is the longest and explains that the mismatch
    /// is an interpreter/state-accounting fault rather than a malformed deploy, so it is asserted
    /// in full.
    #[test]
    fn every_status_renders_a_distinct_message() {
        let mut messages: Vec<String> = Vec::new();
        for status in ALL {
            let message = status.to_string();
            assert!(!message.is_empty(), "{status:?} has no message");
            assert!(
                !messages.contains(&message),
                "two statuses share a message: {message}"
            );
            messages.push(message);
        }
        assert_eq!(messages.len(), ALL.len());

        assert_eq!(BlockStatus::Valid.to_string(), "valid");
        assert_eq!(
            BlockStatus::InvalidStateHash.to_string(),
            "the block's declared post-state hash does not match the state recomputed by replaying \
             its deploys — a node state-accounting inconsistency, not an error in your deploy or API \
             call"
        );
        assert_eq!(
            BlockStatus::InvalidDeployShardId.to_string(),
            "deploy shard id does not match the block's shard"
        );
        assert_eq!(
            BlockStatus::ContainsLowCostDeploy.to_string(),
            "a deploy's phlo price is below the minimum"
        );
    }

    /// **The three causes partition the statuses, and the split is the one AUDIT C173 asks for.**
    ///
    /// The cascade case is the load-bearing one: `NeglectedInvalidBlock` refuses a child *because a
    /// justification failed*, so attributing it to the child is what let one transient failure
    /// fabricate slash evidence against every validator above it. The divergence case is the measured
    /// one: `InvalidStateHash` is this node disagreeing with the producer, not the block being wrong.
    ///
    /// This also keeps the classification exhaustive-by-construction: a status added without a cause
    /// does not compile, because [`BlockStatus::failure_cause`] matches on all of them.
    #[test]
    fn the_causes_partition_the_statuses() {
        const DIVERGENCE: [BlockStatus; 5] = [
            BlockStatus::InvalidStateHash,
            BlockStatus::InvalidPreStateHash,
            BlockStatus::InvalidRejectedDeploy,
            BlockStatus::InvalidBondsCache,
            BlockStatus::UnjustifiedSlash,
        ];

        for status in ALL {
            let cause = status.failure_cause();
            let want_divergence = DIVERGENCE.contains(&status);
            assert_eq!(
                cause == FailureCause::Divergence,
                want_divergence,
                "{status:?} was classified {cause:?}"
            );
            assert_eq!(
                cause == FailureCause::Cascade,
                status == BlockStatus::NeglectedInvalidBlock,
                "{status:?} was classified {cause:?}"
            );
        }

        // A refusal of a child *because its parent failed* must not be slashable, and a disagreement
        // with this node's own state must not be either. Everything else is the block's own fault.
        assert!(matches!(
            BlockStatus::NeglectedInvalidBlock.failure_cause(),
            FailureCause::Cascade
        ));
        assert!(matches!(
            BlockStatus::InvalidVersion.failure_cause(),
            FailureCause::Attributable
        ));
    }

    /// The status is a small `Copy` value that hashes: the block processor keeps it in maps and
    /// compares it, so `Eq`/`Hash` must agree (`Valid` equal to itself, distinct from the rest).
    #[test]
    fn the_status_compares_and_hashes_by_value() {
        use std::collections::HashSet;

        let copied = BlockStatus::Valid;
        assert_eq!(copied, BlockStatus::Valid);

        let set: HashSet<BlockStatus> = ALL.into_iter().collect();
        assert_eq!(set.len(), ALL.len(), "all of them are distinct");

        // A rejected block's status is never equal to `Valid`, which is what the caller tests.
        assert_ne!(BlockStatus::InvalidBondsCache, BlockStatus::Valid);
    }

    /// **The offence predicate is narrower than `Attributable`, and the difference is exactly the
    /// three statuses whose input the receiving node owns** (AUDIT C198).
    ///
    /// Asserted from both sides over `ALL`, so a status added later has to be *decided* here rather
    /// than defaulted: every offence must at least be `Attributable` (the predicate never escalates
    /// beyond blame), and the `Attributable` statuses it withholds must be exactly these three.
    #[test]
    fn the_offence_predicate_is_narrower_than_attributable() {
        let offences: Vec<BlockStatus> = ALL
            .iter()
            .copied()
            .filter(BlockStatus::is_slashing_offence)
            .collect();
        assert_eq!(
            offences,
            vec![
                BlockStatus::InvalidBlockNumber,
                BlockStatus::InvalidRepeatDeploy,
                BlockStatus::InvalidSequenceNumber,
                BlockStatus::InvalidDeployShardId,
                BlockStatus::JustificationRegression,
                BlockStatus::ContainsExpiredDeploy,
                BlockStatus::ContainsFutureDeploy,
                BlockStatus::InvalidPhloLimit,
                BlockStatus::InvalidDeploySignature,
                BlockStatus::TooManyDeploys,
                BlockStatus::ExceedsBlockPhloLimit,
            ],
            "an offence is the block's own signed fields, the DAG's structure, or a version-coupled \
             constant — nothing a receiving node's configuration supplies"
        );

        for status in ALL {
            if status.is_slashing_offence() {
                assert_eq!(
                    status.failure_cause(),
                    FailureCause::Attributable,
                    "{status:?} is an offence, so it must at least be the block's own fault"
                );
            }
        }

        // The three withheld, each because the input it reads belongs to the node, not the block:
        // `casper.min-phlo-price`, `casper.max-number-of-parents`, and the compiled `SUPPORTED`.
        // None is in `GenesisBlockData` or `PosParams`, so there is no committed value an operator
        // could be wrong about — which is why the disagreement cannot be resolved by blaming anyone.
        // `Valid` is excluded because it is not a refusal at all — `failure_cause` classifies it
        // `Attributable` only so a new variant has to be decided rather than defaulted, and
        // `is_valid` is the predicate the block processor branches on.
        let withheld: Vec<BlockStatus> = ALL
            .iter()
            .copied()
            .filter(|s| {
                *s != BlockStatus::Valid
                    && s.failure_cause() == FailureCause::Attributable
                    && !s.is_slashing_offence()
            })
            .collect();
        assert_eq!(
            withheld,
            vec![
                BlockStatus::ContainsLowCostDeploy,
                BlockStatus::TooManyJustifications,
                BlockStatus::InvalidVersion,
            ],
            "exactly these three `Attributable` refusals must not take a bond"
        );
    }
}
