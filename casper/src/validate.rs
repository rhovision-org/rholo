//! Block validation predicates (port of `Validate.scala`) — the pure, effect-free checks.

use rchain_models::block_hash::BlockHash;
use rchain_models::block_version::{VersionSchedule, COMPILED, SUPPORTED};
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_shared::refined::ShardId;

use crate::block_status::BlockStatus;
use crate::proto_util::hash_block;

/// Validate that the block's identifying fields are non-empty (port of `formatOfFields`).
pub fn format_of_fields(b: &BlockMessage) -> bool {
    if b.block_hash == BlockHash::new([0u8; 32]) {
        false
    } else if b.sig.is_empty() {
        false
    } else if b.sig_algorithm.is_empty() {
        false
    } else if ShardId::try_from(b.shard_id.clone()).is_err() {
        // A non-empty, ASCII shard id (Law 26) — the validated constructor is the single ingress
        // check, rather than the empty + non-ASCII pair it replaces (and the `debug_assert!` inside
        // `BlockRandomSeed::new`).
        false
    } else {
        true
    }
}

/// Validate that the block version is supported (port of `version`).
pub fn version(b: &BlockMessage) -> bool {
    SUPPORTED.contains(&b.version)
}

/// Validate that the block hash matches its content-addressed value (Law 16; port of `blockHash`).
pub fn block_hash(b: &BlockMessage) -> bool {
    b.block_hash == hash_block(b)
}

/// Validate the block signature against the sender's public key (port of `blockSignature`).
pub fn block_signature(b: &BlockMessage) -> bool {
    match rchain_crypto::signatures::signatures_alg::from_algorithm(&b.sig_algorithm) {
        Some(alg) => alg.verify(b.block_hash.as_bytes(), &b.sig, b.sender.as_bytes()),
        None => false,
    }
}

/// The one future-deploy boundary used by both block validation and proposer selection.
/// A deploy is not yet usable in block `B` when its `valid_after_block_number` is after `B`.
pub(crate) fn deploy_is_future_at_block(valid_after_block_number: i64, block_number: i64) -> bool {
    valid_after_block_number > block_number
}

/// Validate that no deploy is scheduled for a future block (port of `futureTransaction`).
pub fn future_transaction(b: &BlockMessage) -> BlockStatus {
    if b.state.deploys.iter().any(|d| {
        deploy_is_future_at_block(
            d.deploy.data.valid_after_block_number,
            i64::from(b.block_number),
        )
    }) {
        BlockStatus::ContainsFutureDeploy
    } else {
        BlockStatus::Valid
    }
}

/// The one deploy-expiration boundary used by both block validation and the deploy pool.
///
/// A deploy is invalid in block `B` when its `valid_after_block_number` is at or before
/// `B - expiration_threshold`. Keeping this as one predicate prevents the proposer pool from
/// retaining a deploy for the exact first block in which validation will reject it.
pub(crate) fn deploy_expired_at_block(
    valid_after_block_number: i64,
    block_number: i64,
    expiration_threshold: i64,
) -> bool {
    valid_after_block_number <= block_number - expiration_threshold
}

/// Validate that no deploy has expired (port of `transactionExpiration`).
pub fn transaction_expiration(b: &BlockMessage, expiration_threshold: i64) -> BlockStatus {
    if b.state.deploys.iter().any(|d| {
        deploy_expired_at_block(
            d.deploy.data.valid_after_block_number,
            i64::from(b.block_number),
            expiration_threshold,
        )
    }) {
        BlockStatus::ContainsExpiredDeploy
    } else {
        BlockStatus::Valid
    }
}

/// Validate that all deploys belong to the validator's shard (port of `deploysShardIdentifier`).
pub fn deploys_shard_identifier(b: &BlockMessage, shard_id: &str) -> BlockStatus {
    if b.state
        .deploys
        .iter()
        .all(|d| d.deploy.data.shard_id == shard_id)
    {
        BlockStatus::Valid
    } else {
        BlockStatus::InvalidDeployShardId
    }
}

/// Validate that all deploys meet the minimum phlo price (port of `phloPrice`).
pub fn phlo_price(b: &BlockMessage, min_phlo_price: i64) -> BlockStatus {
    if b.state
        .deploys
        .iter()
        .all(|d| d.deploy.data.phlo_price >= min_phlo_price)
    {
        BlockStatus::Valid
    } else {
        BlockStatus::ContainsLowCostDeploy
    }
}

/// Validate that no deploy carries a negative phlo limit (AUDIT C109).
///
/// **Why this check has to exist on this path.** The charge is `phlo_limit × phlo_price`, and it is a
/// debit: it reaches `native.pre_charge`, which subtracts it from the deployer's vault. A negative
/// limit made that subtraction an addition — `balance - (-n) == balance + n` — crediting the deployer
/// out of nothing while the staking-vault side of the transfer silently no-opped. The deploy-ingress
/// path checked for it (`BlockApiImpl::deploy`), so a *local* deploy could not carry one; but this
/// function's callers are the checks a validator runs on **another node's block**, where the deploy
/// data comes off the wire and nothing had looked at the sign. That asymmetry is exactly the shape
/// that let the bug be reachable only by a peer.
///
/// `total_phlo_charge` now refuses a negative product at the type level, so the replay path cannot
/// mint even without this check. This one is here because a validator should *reject the block*, not
/// fail somewhere downstream: a `BlockStatus` says which rule was broken, and "the charge could not
/// be computed" during replay does not.
pub fn phlo_limit(b: &BlockMessage) -> BlockStatus {
    if b.state
        .deploys
        .iter()
        .all(|d| d.deploy.data.phlo_limit >= 0)
    {
        BlockStatus::Valid
    } else {
        BlockStatus::InvalidPhloLimit
    }
}

/// The validators a block's own justifications hold responsible for an attributable failure
/// (AUDIT C110). This is the *rule*; the proposer narrows it further (see
/// [`crate::blocks::proposer::proposer`], which additionally requires the offender to be bonded — a
/// proposer's choice about who is worth slashing, not part of what makes a slash justified).
///
/// **Why this is one function and not two.** The proposer decided who to slash from this rule, and
/// the receiving validators used to take that decision on trust: they replayed the `Slash`, recomputed
/// the state, saw the hash agree, and accepted. A proposer could therefore name any bonded validator
/// and have every other validator confiscate that stake, honestly and deterministically, without a
/// single node asking whether the victim had done anything. The rule lived only on the producing side,
/// so the consuming side had nothing to check against it. Now both call this.
///
/// `BlockMetadata::slashable` — not `validation_failed` — is the signal, for the reason recorded on
/// that field: it is set only for a failure attributable to the *block*, so a node that merely could
/// not replay something locally does not thereby condemn its sender.
pub fn slashable_senders(
    justifications: &[rchain_models::block_metadata::BlockMetadata],
) -> BTreeMap<rchain_models::validator::Validator, SlashSeverity> {
    let mut out: BTreeMap<rchain_models::validator::Validator, SlashSeverity> = BTreeMap::new();
    for m in justifications.iter().filter(|m| m.slashable) {
        // A validator answers for the **worst** thing it did, not for a count: the slash is
        // per-validator, and a second offence cannot be cheaper than the first.
        out.entry(m.sender)
            .and_modify(|tier| *tier = tier.harsher(m.slash_severity))
            .or_insert(m.slash_severity);
    }
    out
}

/// The validators a block slashes, read off its system deploys. A slash that *failed* during the
/// proposer's run carries no `SystemDeployData::Slash` (`ProcessedSystemDeploy::Failed` holds only an
/// error message), so this reads exactly the slashes that took effect.
pub fn slashed_validators(
    b: &BlockMessage,
) -> BTreeMap<rchain_models::validator::Validator, SlashSeverity> {
    b.state
        .system_deploys
        .iter()
        .filter_map(|sd| match sd {
            rchain_models::casper::protocol::casper_message::ProcessedSystemDeploy::Succeeded {
                system_deploy,
                ..
            } => match system_deploy {
                rchain_models::casper::protocol::casper_message::SystemDeployData::Slash {
                    validator,
                    severity,
                    evidence: _,
                } => Some((*validator, *severity)),
                _ => None,
            },
            rchain_models::casper::protocol::casper_message::ProcessedSystemDeploy::Failed {
                ..
            } => None,
        })
        .collect()
}

/// The **equivocation evidence** a block carries, per victim (AUDIT C200). A slash with none is the
/// metadata-justified kind, which `slashed_validators` above is the whole story for.
pub fn slashed_evidence(
    b: &BlockMessage,
) -> BTreeMap<rchain_models::validator::Validator, Vec<u8>> {
    b.state
        .system_deploys
        .iter()
        .filter_map(|sd| match sd {
            rchain_models::casper::protocol::casper_message::ProcessedSystemDeploy::Succeeded {
                system_deploy:
                    rchain_models::casper::protocol::casper_message::SystemDeployData::Slash {
                        validator,
                        evidence: Some(bytes),
                        ..
                    },
                ..
            } => Some((*validator, bytes.clone())),
            _ => None,
        })
        .collect()
}

/// **Does this evidence prove that `offender` equivocated** (AUDIT C200) — the second way a slash can
/// be justified, and the one that needs no failed block.
///
/// Two conditions, and every node can check both against the chain it already has:
///
/// 1. the evidence is a header the **offender itself signed** — the sender check and the signature
///    verification are [`EquivocationEvidence::is_signed_by`], so a proposer can neither name a victim
///    it did not sign for nor invent a hash the victim never signed;
/// 2. the DAG **already holds a different block** by that sender at that `seq_num`, which is what makes
///    two blocks an equivocation rather than one block.
///
/// **The first block does not travel.** It was inserted normally and is in the receiver's own DAG —
/// only the *second* was refused, at the H-1 gate, before any write. So the evidence is a header and no
/// receiver has to take the proposer's word for anything: it re-derives the offence from a block it
/// already has and a signature it verifies.
///
/// A malformed or unparsable payload is `false`, not an error: it proves nothing, which is exactly what
/// a refusal should say.
pub async fn equivocation_is_proved(
    dag: &dyn BlockDagStorage,
    offender: &rchain_models::validator::Validator,
    evidence: &[u8],
) -> Result<bool, String> {
    let Ok(proved) =
        rchain_models::casper::protocol::casper_message::EquivocationEvidence::decode(evidence)
    else {
        return Ok(false);
    };
    if !proved.is_signed_by(offender) {
        return Ok(false);
    }
    let repr = dag.get_representation().await;
    Ok(repr.dag_message_state.msg_map.iter().any(|(h, m)| {
        m.sender == *offender && m.sender_seq == proved.seq_num && *h != proved.block_hash
    }))
}

// --- Effectful checks (depend on the block DAG) ------------------------------------------------

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_block_storage::dag::finalizer::Message;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::block_metadata::SlashSeverity;
use rchain_models::validator::Validator;

use crate::proto_util::{
    get_parent_metadatas_above_block_number, get_parents_metadata, max_block_number_metadata,
};
use crate::runtime_manager::RuntimeManager;

/// A block-validation outcome: `Ok(())` is valid, `Err(status)` is the invalid status (port of
/// `ValidBlockProcessing`).
pub type ValidBlockProcessing = Result<(), BlockStatus>;

/// Validate the block number against its justifications (port of `blockNumber`).
///
/// Two roles, deliberately separated (H1b, now enforced):
///
/// - **Every resolved parent must be *lower* than this block** — `Descends`
///   (`spec/Rchain/Casper/Dag.lean:Descends`) — failed or not. A failed block's recorded height is its
///   **claimed** `block_num` (`message_from_block_metadata`'s `height: block.block_num`), so without
///   this a block could name a failed parent far above itself, and the model's premise was false of a
///   state the port admitted. The witness is `dag.rs`'s
///   `h1b_a_failed_parent_above_the_childs_height_is_refused`, which was the *reproduction* of that
///   admitted violation before this check existed.
/// - **The maximum counts every resolved parent too, failed or not.** The Scala skips a failed
///   justification here (`if (!m.validationFailed)`), and the port copied that — which made a failure
///   *hide* its own block: a child justifying the failed parent was refused `InvalidBlockNumber` for
///   the sole reason that this node had marked the parent failed, and it was refused *here*, in
///   `block_summary`, before `neglected_invalid_block` could even run. That is the second of the
///   readers that turn one attributable failure into a permanently estranged node (AUDIT C173, #125).
///
///   Counting it lets the child reach the other checks — a **prerequisite for the restoring rule, not
///   a substitute for it**. The child's own pre-state still merges the failed parent out (the
///   non-failed parent set in `interpreter_util.rs`), so this admits the block to validation rather
///   than promising it passes: with the parent still recorded, the child of a *bonded* sender now
///   fails `neglected_invalid_block` instead, and the child of an *unbonded* one reaches the replay
///   and fails a `Divergence` — the cause the restore may clear.
///
/// Both halves are therefore **deliberate divergences from the Scala**, which skips failed parents for
/// the height check entirely and so admits a block this refuses. The laws are the port's oracle, so the
/// premise the model needs is *guaranteed* here rather than assumed; the divergences are registered in
/// §6 (a peer sending such a block is refused — see the audit entry for the operator consequence).
///
/// Related, and unchanged: `neglected_invalid_block` refuses a block that justifies a failed **bonded**
/// validator's block (`h1b_a_justified_bonded_failed_block_is_refused_rather_than_forced`), so the
/// route this change opens is the unbonded one.
pub async fn block_number(
    dag: &dyn BlockDagStorage,
    b: &BlockMessage,
) -> Result<ValidBlockProcessing, String> {
    let mut max_block_number = -1i64;
    for j in &b.justifications {
        let meta = dag
            .lookup(j)
            .await?
            .ok_or_else(|| format!("missing justification {}", j.to_hex()))?;
        // The descent bound, for *every* resolved parent (H1b).
        if i64::from(meta.block_num) >= i64::from(b.block_number) {
            return Ok(Err(BlockStatus::InvalidBlockNumber));
        }
        // The maximum, over **every** resolved parent. A failed block still counts: skipping it made
        // the failure hide the block, refusing its child for the parent's record alone (AUDIT C173).
        max_block_number = max_block_number.max(i64::from(meta.block_num));
    }
    if max_block_number + 1 == i64::from(b.block_number) {
        Ok(Ok(()))
    } else {
        Ok(Err(BlockStatus::InvalidBlockNumber))
    }
}

/// Validate the sender's sequence number is one more than its latest justification's (port of
/// `sequenceNumber`).
pub async fn sequence_number(
    dag: &dyn BlockDagStorage,
    b: &BlockMessage,
) -> Result<ValidBlockProcessing, String> {
    let mut creator_latest_seq = -1i64;
    for j in &b.justifications {
        let meta = dag
            .lookup(j)
            .await?
            .ok_or_else(|| format!("missing justification {}", j.to_hex()))?;
        if meta.sender == b.sender {
            creator_latest_seq = creator_latest_seq.max(i64::from(meta.seq_num));
        }
    }
    if creator_latest_seq + 1 == i64::from(b.seq_num) {
        Ok(Ok(()))
    } else {
        Ok(Err(BlockStatus::InvalidSequenceNumber))
    }
}

/// **The regression check's answer** (Law 88d). `Unresolved` is not a verdict on the block: the
/// check cannot be evaluated because a justification is absent from the DAG message map, and the
/// current rule **passes** (the Scala's own `None`). It is carried so the reason is named rather
/// than folded into "the check said yes" — the later height/sequence check is where such a block
/// fails, and it already names the missing hash. `NoOwnJustification` is the second, different
/// unresolved shape: the sender is absent from its own justification set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JustificationOutcome {
    Pass,
    Regression,
    Unresolved(Vec<BlockHash>),
    NoOwnJustification,
}

impl JustificationOutcome {
    /// **The classification as a pure function** (Law 88d), for the conformance corpus. Unresolved
    /// when the justification is absent from the message map; otherwise the regression relation's
    /// answer. It carries no hash, because the corpus is about the classification, not the id.
    pub fn classify(in_msg_map: bool, regression: bool) -> Self {
        if !in_msg_map {
            JustificationOutcome::Unresolved(Vec::new())
        } else if regression {
            JustificationOutcome::Regression
        } else {
            JustificationOutcome::Pass
        }
    }

    /// Whether the outcome refuses the block. **Only `Regression` does**; `Unresolved` is not a
    /// verdict, which is what keeps this clause free of a consensus change.
    pub fn refuses(&self) -> bool {
        matches!(self, JustificationOutcome::Regression)
    }

    pub fn name(&self) -> &'static str {
        match self {
            JustificationOutcome::Pass => "pass",
            JustificationOutcome::Regression => "regression",
            JustificationOutcome::Unresolved(_) => "unresolved",
            JustificationOutcome::NoOwnJustification => "no_own",
        }
    }
}

/// Validate there is no justification regression (port of `justificationRegressions`).
///
/// **Verdicts are unchanged.** `Pass`, `Unresolved` and `NoOwnJustification` all admit to the next
/// check; only `Regression` refuses. Making `Unresolved` a refusal would be a consensus change.
pub async fn justification_regressions(
    dag: &dyn BlockDagStorage,
    b: &BlockMessage,
) -> Result<ValidBlockProcessing, String> {
    match check_justification_regression(dag, b).await? {
        JustificationOutcome::Regression => Ok(Err(BlockStatus::JustificationRegression)),
        JustificationOutcome::Pass
        | JustificationOutcome::Unresolved(_)
        | JustificationOutcome::NoOwnJustification => Ok(Ok(())),
    }
}

async fn check_justification_regression(
    dag: &dyn BlockDagStorage,
    b: &BlockMessage,
) -> Result<JustificationOutcome, String> {
    let repr = dag.get_representation().await;
    let msg_map: &BTreeMap<BlockHash, Message<BlockHash, Validator>> =
        &repr.dag_message_state.msg_map;

    // `justifications.map(msgMap.get).sequence` — None if any is missing (see the Scala TODO).
    let justifications: Option<Vec<Message<BlockHash, Validator>>> = b
        .justifications
        .iter()
        .map(|j| msg_map.get(j).cloned())
        .collect();
    let justifications = match justifications {
        Some(js) => js,
        None => {
            let missing = b
                .justifications
                .iter()
                .filter(|j| !msg_map.contains_key(*j))
                .cloned()
                .collect();
            return Ok(JustificationOutcome::Unresolved(missing));
        }
    };

    let prev_msg = match justifications.iter().find(|m| m.sender == b.sender) {
        Some(m) => m,
        None => return Ok(JustificationOutcome::NoOwnJustification),
    };

    let res = justifications.iter().all(|just| {
        let just_prev_msg = prev_msg
            .parents
            .iter()
            .filter_map(|p| msg_map.get(p))
            .find(|m| m.sender == just.sender);
        match just_prev_msg {
            Some(just_prev_msg) => just_prev_msg.seen.difference(&just.seen).next().is_none(),
            None => true,
        }
    });
    Ok(if res {
        JustificationOutcome::Pass
    } else {
        JustificationOutcome::Regression
    })
}

/// Validate that a block does not neglect an invalid-but-still-bonded justification (port of
/// `neglectedInvalidBlock`).
pub async fn neglected_invalid_block(
    dag: &dyn BlockDagStorage,
    b: &BlockMessage,
) -> Result<ValidBlockProcessing, String> {
    let mut justifications = Vec::new();
    for j in &b.justifications {
        if let Some(meta) = dag.lookup(j).await? {
            justifications.push(meta);
        }
    }
    let neglected = justifications
        .iter()
        .filter(|m| m.validation_failed)
        .map(|m| m.sender)
        .any(|v| {
            b.bonds
                .get(&v)
                .map(|&stake| i64::from(stake) > 0)
                .unwrap_or(false)
        });
    if neglected {
        Ok(Err(BlockStatus::NeglectedInvalidBlock))
    } else {
        Ok(Ok(()))
    }
}

/// Look up a block from the block store, failing if absent (port of `BlockStore.getUnsafe`).
async fn get_block_unsafe(
    block_store: &BlockStore,
    hash: &BlockHash,
) -> Result<BlockMessage, String> {
    let mut vals = block_store.get(&[*hash]).await?;
    vals.pop()
        .flatten()
        .ok_or_else(|| format!("missing block {}", hash.to_hex()))
}

/// Normalize a deploy signature to its low-S form so a high-S / low-S pair of the same ECDSA
/// signature are treated as the same deploy (signature malleability). `algorithm` is the deploy's
/// `sig_algorithm` (e.g. `"secp256k1"`). Delegates to the crypto crate's canonicalizer.
fn normalize_signature_low_s(algorithm: &str, signature: &[u8]) -> Vec<u8> {
    rchain_crypto::signatures::signatures_alg::normalize_signature_low_s(algorithm, signature)
}

/// Validate that no deploy with the same sig has been produced in the chain within the expiration
/// window (port of `repeatDeploy`).
pub async fn repeat_deploy(
    dag: &dyn BlockDagStorage,
    block_store: &BlockStore,
    block: &BlockMessage,
    expiration_threshold: i64,
) -> Result<ValidBlockProcessing, String> {
    let deploy_key_set: BTreeSet<Vec<u8>> = block
        .state
        .deploys
        .iter()
        .map(|d| normalize_signature_low_s(&d.deploy.sig_algorithm, &d.deploy.sig))
        .collect();

    let block_metadata = BlockMetadata::from_block(block);
    let init_parents = get_parents_metadata(dag, &block_metadata).await?;
    let max_block_number = max_block_number_metadata(&init_parents);
    let earliest_block_number = max_block_number + 1 - expiration_threshold;

    // Breadth-first traversal of the parent chain above the expiration horizon (port of
    // `DagOps.bfTraverseF(...).findF(...)`).
    let mut queue: VecDeque<BlockMetadata> = init_parents.into_iter().collect();
    let mut visited: HashSet<BlockHash> = HashSet::new();
    while let Some(curr) = queue.pop_front() {
        if visited.contains(&curr.block_hash) {
            continue;
        }
        visited.insert(curr.block_hash);

        let b = get_block_unsafe(block_store, &curr.block_hash).await?;
        if b.state.deploys.iter().any(|d| {
            deploy_key_set.contains(&normalize_signature_low_s(
                &d.deploy.sig_algorithm,
                &d.deploy.sig,
            ))
        }) {
            return Ok(Err(BlockStatus::InvalidRepeatDeploy));
        }

        let parents =
            get_parent_metadatas_above_block_number(dag, &curr, earliest_block_number).await?;
        for p in parents {
            if !visited.contains(&p.block_hash) {
                queue.push_back(p);
            }
        }
    }
    Ok(Ok(()))
}

/// Validate that the block's bond cache matches the proof-of-stake contract's bonds at the post
/// state (port of `bondsCache`).
pub async fn bonds_cache(
    runtime: &RuntimeManager,
    block: &BlockMessage,
) -> Result<ValidBlockProcessing, String> {
    let tuplespace_hash = block.post_state_hash;
    let computed_bonds = runtime.compute_bonds(&tuplespace_hash).await?;
    if block.bonds == computed_bonds {
        Ok(Ok(()))
    } else {
        Ok(Err(BlockStatus::InvalidBondsCache))
    }
}

/// Compose the effectful + pure checks (port of `blockSummary`).
///
/// Explicit-bound entry point for callers with already resolved parameters and legacy fixtures.
/// The production validator uses [`block_summary_from_state`] to resolve the committed bounds.
pub async fn block_summary(
    dag: &dyn BlockDagStorage,
    block_store: &BlockStore,
    block: &BlockMessage,
    shard_id: &str,
    expiration_threshold: i64,
    min_phlo_price: i64,
    max_number_of_parents: i32,
) -> Result<ValidBlockProcessing, String> {
    block_summary_from_state(
        dag,
        block_store,
        block,
        shard_id,
        expiration_threshold,
        std::future::ready(Ok((min_phlo_price, i64::from(max_number_of_parents)))),
    )
    .await
}

/// Resolve bounds only after structural checks, but before timestamp reads, merge and replay.
/// The future reads a locally validated parent's state, never the incoming block's claimed root.
pub async fn block_summary_from_state(
    dag: &dyn BlockDagStorage,
    block_store: &BlockStore,
    block: &BlockMessage,
    shard_id: &str,
    expiration_threshold: i64,
    bounds: impl std::future::Future<Output = Result<(i64, i64), String>>,
) -> Result<ValidBlockProcessing, String> {
    if let Err(status) = justification_regressions(dag, block).await? {
        return Ok(Err(status));
    }
    if let Err(status) = sequence_number(dag, block).await? {
        return Ok(Err(status));
    }
    if let Err(status) = block_number(dag, block).await? {
        return Ok(Err(status));
    }
    let (min_phlo_price, max_number_of_parents) = bounds.await?;
    // Pure deploy checks — including `phlo_price`, which must be rejected *before* the expensive
    // replay so an economically-free deploy cannot force every validator to replay it (R27).
    let pure = [
        deploys_shard_identifier(block, shard_id),
        future_transaction(block),
        transaction_expiration(block, expiration_threshold),
        phlo_price(block, min_phlo_price),
        // `phlo_limit` belongs in this list for the reason `phlo_price` is here at all: both are
        // cheap tests on wire-supplied deploy data that must reject a block *before* the expensive
        // replay. For `phlo_limit` the stakes are higher — without it a peer's block reaches the
        // charge path with a negative limit (AUDIT C109).
        phlo_limit(block),
        // `deploy_signatures` is here for that same reason, and it is the check that decides *whose*
        // account the replay is about to debit: every check above reads the deploy's fields as data,
        // and this is the only one that asks whether the key those fields name actually authored them
        // (AUDIT C120).
        deploy_signatures(block),
        // The two bounds on the block *as a whole* (F-3). Both are lengths or sums over
        // wire-supplied data, so they are here for the reason the checks above are: a peer's block must
        // be rejected before every validator spends the replay on it. Without them the per-deploy
        // budget bounds each deploy and nothing bounds the block.
        deploy_count(block),
        block_phlo(block),
        // And the block's declared version (F-6). `version` had a predicate and a test from the port
        // onward and no caller: a block stamped `version: 999` was accepted by every check here.
        block_version(block),
        // The *other* per-block width, and the one that had no check on either side (#153). It sits in
        // this list for the reason the two F-3 bounds do: the count is a length over wire-supplied data,
        // so a peer's block has to be refused before every validator merges over the set it names.
        justification_count(block, max_number_of_parents),
    ];
    for status in pure {
        if !status.is_valid() {
            return Ok(Err(status));
        }
    }
    repeat_deploy(dag, block_store, block, expiration_threshold).await
}

/// Validate that every deploy in the block is signed by the key its `deployer` field names
/// (AUDIT C120).
///
/// **Why this check has to exist on this path, and not only at the ingress.** The replay reads
/// `deploy.deployer` to build the pre-charge, the refund and the `rho:rchain:deployerId` binding the
/// term can spend from, so that field is an authorization claim. `SignedDeployData::verify_signature`
/// existed and was correct, but its only production caller was `BlockApiImpl::deploy` — the path a
/// *local* deploy takes. A block arriving from a peer carries its deploys as `ProcessedDeploy`s whose
/// `deployer` and `sig` are copied verbatim off the wire, and the checks a validator ran on them
/// (shard id, validity window, phlo price, phlo limit, signature *deduplication*) all read those
/// fields without ever asking who signed them. So a bonded proposer could name any account, put
/// arbitrary bytes in `sig`, and have every other validator debit that account and pay the proposer's
/// term — with a post-state hash the proposer computed honestly, so `block_signature`,
/// `validate_block_checkpoint` and `bonds_cache` all agreed, the block was valid, and nothing was
/// attributable. That asymmetry between the ingress and the block path is the same one C109 records
/// one field over, which is why the check is one of `block_summary`'s pure checks.
///
/// `verify_signature` is over the whole deploy data (the term included), so this is a check on the
/// message and not merely on the key: a term altered after signing fails it. A *valid* signature made
/// by a key other than the named deployer also fails it, because the key it verifies against is the
/// one the field names.
///
/// `system_deploys` are deliberately not inspected: a `Slash` is a system deploy and is unsigned by
/// its victim, and the rule that makes one legitimate is `slashable_senders`/`slashed_validators`,
/// which `validate_block_checkpoint` applies (C110).
///
/// **Why it is declared here and not among the pure checks above.** It belongs beside `phlo_limit` for
/// the reason that check's own comment gives, and the first version of it was written there — which
/// moved every line below it, and with them **six citations in `spec/laws.tsv` and seven in
/// `spec/AUDIT.md`** (measured, not feared: the register checks those anchors by line). Declaring it
/// after the one function that consumes it costs nothing and shifts no anchor, so a later pass reading
/// the law register's citations finds them where they were written.
pub fn deploy_signatures(b: &BlockMessage) -> BlockStatus {
    if b.state.deploys.iter().all(|d| d.deploy.verify_signature()) {
        BlockStatus::Valid
    } else {
        BlockStatus::InvalidDeploySignature
    }
}

/// The most phlo one block may declare across all its deploys (AUDIT F-3).
///
/// Sized as a full block of deploys each at the protocol's default limit: `MAX_BLOCK_DEPLOYS` (255,
/// the width of the seed index) × 100 000 000 — the `phloLimit` a deploy carries unless its author asks
/// for more. A block at that bound is one the proposer could always have built, so this refuses only
/// what is already outside the protocol's own envelope, while still bounding the sum that nothing
/// bounded before.
///
/// **A protocol constant, not a config value, and that is the point.** Two operators running different
/// values would disagree about which blocks are valid, which is a fork; `MAX_BLOCK_DEPLOYS` is a
/// constant for the same reason. It is also the reason this is not threaded through `CasperConf`.
///
/// Declared below `block_summary` so it shifts no cited line — see [`deploy_signatures`].
pub const MAX_BLOCK_PHLO: i64 = 25_500_000_000; // MAX_BLOCK_DEPLOYS × 1e8

/// Validate that the block carries no more deploys than the protocol's seed index can address
/// (AUDIT F-3).
///
/// **Why this has to exist on the receiving side.** The proposer has always bounded its own selection
/// — `MAX_BLOCK_DEPLOYS` is the width of the `u8` seed index in `block_creator`, so the deploy count
/// plus the slash count has to fit in 255 — but that bound lived *only* in the proposer. A block
/// arriving from a peer was never checked against it, so a bonded proposer could pack a block
/// arbitrarily full and every validator would replay the lot. The check is a length, so it belongs
/// among the pre-replay checks beside `phlo_price` and `phlo_limit`.
///
/// `system_deploys` are not counted: they are seeded separately and are already bounded by
/// `max-number-of-parents`, which is what makes the proposer's budget a subtraction
/// (`per_block_deploy_budget`) rather than a joint cap.
pub fn deploy_count(b: &BlockMessage) -> BlockStatus {
    // Fully qualified rather than imported: a `use` line here would shift every line below it, and
    // `spec/laws.tsv` cites twenty of them by number (max line 429), inherited from the Lean register.
    // The same reason `deploy_signatures` is declared below its consumer rather than above it.
    if b.state.deploys.len() <= crate::blocks::proposer::proposer::MAX_BLOCK_DEPLOYS {
        BlockStatus::Valid
    } else {
        BlockStatus::TooManyDeploys
    }
}

/// Validate that the block's justification set is no wider than the protocol allows (#153).
///
/// **What makes this the input bound #153 asks for, and not a second `deploy_count`.** The merge is the
/// one part of block processing whose input a proposer chooses outright: the justification set decides
/// which chains every validator weighs, and until this check nothing on either side bounded it — the
/// proposer had no cap and the validator had no refusal. `max-number-of-parents` said otherwise in two
/// doc comments and in C123's fix rationale, and no code read the key at all.
///
/// **The bound is a property of the network, so it arrives as an argument.** A non-positive value
/// disables the check, which is how a chain that has not adopted it keeps the pre-#153 semantics
/// exactly. The caller resolves both bounds from committed state (or legacy config).
///
/// Refusing a set above a fixed width **is a consensus change**, because such a block is valid today.
/// It is registered in `spec/audit/passes.md` §6 and tracked on #51 §A, and the shipped default is the
/// protocol's existing per-block width rather than `i32::MAX`.
pub fn justification_count(b: &BlockMessage, max_number_of_parents: i64) -> BlockStatus {
    if max_number_of_parents <= 0 || b.justifications.len() as u128 <= max_number_of_parents as u128
    {
        BlockStatus::Valid
    } else {
        BlockStatus::TooManyJustifications
    }
}

/// Validate that the block's total declared phlo fits the block budget (AUDIT F-3).
///
/// **The sum is over the signed `phlo_limit`, not over `ProcessedDeploy.cost`.** The cost field is
/// proposer-supplied wire data that replay recomputes, so a cap keyed on it would be a cap the
/// proposer sets for itself. `phlo_limit` is what the deploy's author signed, and it is therefore the
/// honest statement of how much work the block asks for. The sum saturates rather than wrapping: the
/// deploy-level `phlo_limit` check rejects negatives in the same pass, but a cap that could itself
/// overflow on hostile input would be no cap at all.
///
/// **Deliberate divergence from the Scala reference**, which composes no per-block bound:
/// `blockSummary` runs the seven checks this port already had and nothing else. Without one, the
/// per-deploy budget bounds each deploy separately and a block can be arbitrarily expensive — which is
/// what the September 2026 audit measured, and the reason Sui's transaction bound and Solana's block
/// compute budget are cited against this node in the comparison.
///
/// See [`MAX_BLOCK_PHLO`] for why the bound is a constant rather than a config value. Declared here
/// rather than beside its consumers for the reason [`deploy_signatures`] records: inserting above
/// `block_summary` shifts line numbers that the law and audit registers cite.
pub fn block_phlo(b: &BlockMessage) -> BlockStatus {
    let total = b
        .state
        .deploys
        .iter()
        .map(|d| d.deploy.data.phlo_limit)
        .fold(0i64, i64::saturating_add);
    if total <= MAX_BLOCK_PHLO {
        BlockStatus::Valid
    } else {
        BlockStatus::ExceedsBlockPhloLimit
    }
}

/// Validate that the block's `version` is one this node supports (AUDIT F-6).
///
/// **A predicate that existed and was never called.** [`version`] has been in this file since the
/// port: `SUPPORTED = [1]`, a passing unit test at the bottom of this module, and — until now — no
/// production caller anywhere in the tree. The acceptance path read the block's hash, its signature,
/// its shard and its deploy data, and never the field that says which protocol the block is written
/// against. So a block stamped `version: 999`, self-consistently hashed and signed, satisfied every
/// predicate a validator ran.
///
/// It matters beyond tidiness because the field is inside `hash_block`'s cover, which makes it
/// consensus-visible: a future version bump would not have been enforced by anything, and two nodes
/// disagreeing about which versions they accept is exactly the divergence the field exists to
/// prevent. The gap is inherited rather than introduced — the Scala's `BlockReceiver` carries
/// `// TODO: check valid version` in the same conjunction, and its `Validate.version` has no
/// production caller either — but the port kept the TODO's behaviour and the Scala's.
///
/// Declared here rather than beside the other predicates for the reason [`deploy_signatures`]
/// records: inserting above `block_summary` shifts line numbers the law register cites.
///
/// **And the version has to be the one scheduled for the block's height**, not merely one this binary
/// knows (`rchain_models::block_version::SCHEDULE`). That is what makes a rule change a scheduled one
/// rather than a new genesis: below an activation height a block must still carry the old version, so
/// the old rules keep applying to the history a node replays, and at or above it every block must carry
/// the new one, so no proposer can opt out of the new rules or into them early. With the single shipped
/// row (version 1 from height 0) the two conditions are the same and no block changes.
pub fn block_version(b: &BlockMessage) -> BlockStatus {
    block_version_for_schedule(b, COMPILED)
}

fn block_version_for_schedule(b: &BlockMessage, schedule: VersionSchedule) -> BlockStatus {
    if b.version == schedule.version_at(i64::from(b.block_number)) {
        BlockStatus::Valid
    } else {
        BlockStatus::InvalidVersion
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    use rchain_models::casper::protocol::casper_message::{
        DeployData, PCost, ProcessedDeploy, RholangState, SignedDeployData,
    };
    use rchain_models::validator::Validator;

    fn deploy(valid_after: i64, phlo_price: i64, shard_id: &str) -> ProcessedDeploy {
        ProcessedDeploy {
            deploy: SignedDeployData {
                data: DeployData {
                    attachments: Vec::new(),
                    term: "Nil".to_string(),
                    timestamp: 0,
                    phlo_price,
                    phlo_limit: 100,
                    valid_after_block_number: valid_after,
                    shard_id: shard_id.to_string(),
                },
                deployer: vec![],
                sig: vec![1],
                sig_algorithm: "secp256k1".to_string(),
            },
            cost: PCost { cost: 0 },
            deploy_log: vec![],
            is_failed: false,
            system_deploy_error: None,
        }
    }

    /// A block whose `version` this node does not support is refused (AUDIT F-6).
    ///
    /// The predicate and its own test have been in this module since the port. What was missing was a
    /// *caller*: nothing on the acceptance path read the field, so a self-consistently hashed and
    /// signed block could name any version it liked.
    #[test]
    fn a_block_version_outside_the_supported_set_is_refused() {
        let mut b = block();
        assert_eq!(
            block_version(&b),
            BlockStatus::Valid,
            "the fixture carries a supported version, so the refusal below is the version and not the block"
        );

        b.version = 999;
        assert_eq!(block_version(&b), BlockStatus::InvalidVersion);
    }

    #[test]
    fn scheduled_block_versions_are_required_at_and_after_activation_and_history_replays() {
        use rchain_models::block_version::Activation;
        use rchain_shared::refined::BlockHeight;

        const STAGED: VersionSchedule = VersionSchedule::new(&[
            Activation {
                version: 1,
                from_height: 0,
            },
            Activation {
                version: 2,
                from_height: 10,
            },
        ]);
        for height in [0, 1, 9, 10, 11, 100] {
            let mut b = block();
            b.block_number = BlockHeight::try_from(height).unwrap();
            for version in [1, 2, 999] {
                b.version = version;
                let expected = if version == if height < 10 { 1 } else { 2 } {
                    BlockStatus::Valid
                } else {
                    BlockStatus::InvalidVersion
                };
                assert_eq!(
                    block_version_for_schedule(&b, STAGED),
                    expected,
                    "height {height}, version {version}"
                );
            }
        }
        for height in 0..10 {
            let mut historic = block();
            historic.block_number = BlockHeight::try_from(height).unwrap();
            historic.version = 1;
            assert_eq!(block_version(&historic), BlockStatus::Valid);
            assert_eq!(
                block_version_for_schedule(&historic, STAGED),
                BlockStatus::Valid
            );
        }
    }

    /// A block carrying more deploys than the seed index can address is refused, and the limit itself
    /// is allowed (AUDIT F-3).
    ///
    /// The proposer has always bounded its own selection — the seed in `block_creator` is indexed in a
    /// `u8` — but that bound lived only in the proposer: a block arriving from a peer was never checked
    /// against it, so a bonded proposer could pack a block arbitrarily full and every validator would
    /// replay the lot.
    #[test]
    fn a_block_carrying_more_deploys_than_the_seed_index_addresses_is_refused() {
        let cap = crate::blocks::proposer::proposer::MAX_BLOCK_DEPLOYS;

        let mut at_limit = block();
        at_limit.state.deploys = (0..cap).map(|_| deploy(0, 1, "root")).collect();
        assert_eq!(
            deploy_count(&at_limit),
            BlockStatus::Valid,
            "the limit itself is within the protocol's envelope"
        );

        let mut past = block();
        past.state.deploys = (0..=cap).map(|_| deploy(0, 1, "root")).collect();
        assert_eq!(deploy_count(&past), BlockStatus::TooManyDeploys);
    }

    /// A block whose justification set is wider than the network allows is refused, and the bound
    /// itself is allowed (#153).
    ///
    /// **This is the input bound, and it had no check on either side.** The proposer had no cap and the
    /// validator had no refusal, while `max-number-of-parents` sat in the configuration read by nothing —
    /// and two doc comments plus C123's fix rationale asserted it bounded exactly this. What makes it the
    /// *input* bound rather than a second `deploy_count` is that the justification set is the one block
    /// input a proposer chooses outright: it decides which chains every validator weighs.
    ///
    /// The pair is asserted together, at the bound and one past it, for the reason the F-3 tests above
    /// do: a check that refused everything would pass the refusal half alone.
    #[test]
    fn a_block_justifying_more_parents_than_the_network_allows_is_refused() {
        let cap = 255;

        let mut at_limit = block();
        at_limit.justifications = (0..cap).map(|i| BlockHash::new([i as u8; 32])).collect();
        assert_eq!(
            justification_count(&at_limit, cap),
            BlockStatus::Valid,
            "the bound itself is within the protocol's envelope"
        );

        let mut past = block();
        past.justifications = (0..=cap).map(|i| BlockHash::new([i as u8; 32])).collect();
        assert_eq!(
            justification_count(&past, cap),
            BlockStatus::TooManyJustifications
        );
    }

    /// **The C198 falsifier: a per-node knob changes the refusal and must never change the offence.**
    ///
    /// Each check below is configured **per node** with no value committed at genesis — the fee floor
    /// `casper.min-phlo-price`, the width `casper.max-number-of-parents`, and the compiled
    /// `SUPPORTED` set — so two correctly-configured operators can disagree about the *same block*.
    ///
    /// A disagreement about a refusal is harmless. A disagreement about an **offence** is a chain
    /// split: `slash_is_unjustified` (`interpreter_util.rs`) makes a node refuse any block whose
    /// slashes it cannot re-derive from its own metadata, so the strict node's `Slash(X)` is a block
    /// the permissive node rejects — permanently, and over a local setting, with X's whole bond gone.
    ///
    /// Red before C198: `slashable` was `matches!(cause, Attributable)`, all three of these are
    /// `Attributable`, and the assertions below were the wrong way round.
    #[test]
    fn a_local_knob_changes_the_refusal_and_never_the_offence() {
        // A fee floor above and below the same deploy's price.
        let mut priced = block();
        priced.state.deploys = vec![deploy(0, 1, "root")];
        let refused = phlo_price(&priced, 2);
        let accepted = phlo_price(&priced, 1);
        assert_ne!(refused, accepted, "the knob does move the refusal");
        assert_eq!(refused, BlockStatus::ContainsLowCostDeploy);
        assert_eq!(accepted, BlockStatus::Valid);
        assert!(
            !refused.is_slashing_offence() && !accepted.is_slashing_offence(),
            "neither node may be asked to take a bond for this block"
        );

        // An active-set width: 256 justifications is past one node's bound and inside another's,
        // since a non-positive bound disables the check (how a pre-#153 chain keeps its semantics).
        let mut wide = block();
        wide.justifications = (0..=255u8).map(|i| BlockHash::new([i; 32])).collect();
        let over = justification_count(&wide, 255);
        let under = justification_count(&wide, 0);
        assert_ne!(over, under, "the knob does move the refusal");
        assert_eq!(over, BlockStatus::TooManyJustifications);
        assert_eq!(under, BlockStatus::Valid);
        assert!(
            !over.is_slashing_offence() && !under.is_slashing_offence(),
            "and never the offence"
        );

        // A rule version. `SUPPORTED` is compiled in rather than configured, so this is the version
        // coupling rather than an operator choice — and during an upgrade it is the *older* nodes
        // refusing the *newer* ones' blocks, which is the last thing that may take a bond.
        let mut newer = block();
        newer.version = 2;
        assert_eq!(block_version(&newer), BlockStatus::InvalidVersion);
        assert!(!block_version(&newer).is_slashing_offence());
    }

    /// A non-positive bound disables the check, which is how a chain that has not adopted #153 keeps the
    /// pre-#153 semantics exactly.
    ///
    /// Without this the default would be the only thing standing between an operator and a network that
    /// refuses blocks it used to accept, and "0 means off" would be an assumption rather than a test.
    #[test]
    fn a_non_positive_parent_bound_disables_the_check() {
        let mut wide = block();
        wide.justifications = (0..300).map(|i| BlockHash::new([i as u8; 32])).collect();

        for off in [0, -1, i32::MIN] {
            assert_eq!(
                justification_count(&wide, i64::from(off)),
                BlockStatus::Valid,
                "a bound of {off} means the check is off, not that every block is refused"
            );
        }
    }

    /// A block whose deploys collectively declare more phlo than the block budget is refused, and the
    /// budget itself is allowed (AUDIT F-3).
    ///
    /// Before this there was no per-block bound at all. The per-deploy budget bounds each deploy
    /// separately, so a proposer could make one block arbitrarily expensive to replay on every
    /// validator — which is what the September 2026 audit measured, and the gap Sui's transaction bound
    /// and Solana's block compute budget are cited against.
    #[test]
    fn a_block_exceeding_the_block_phlo_budget_is_refused() {
        let half = MAX_BLOCK_PHLO / 2;

        let mut at_limit = block();
        let mut first = deploy(0, 1, "root");
        first.deploy.data.phlo_limit = half;
        let mut second = deploy(0, 1, "root");
        second.deploy.data.phlo_limit = MAX_BLOCK_PHLO - half;
        at_limit.state.deploys = vec![first, second];
        assert_eq!(
            block_phlo(&at_limit),
            BlockStatus::Valid,
            "a block that sums to exactly the budget is within it"
        );

        let mut over = block();
        let mut single = deploy(0, 1, "root");
        single.deploy.data.phlo_limit = MAX_BLOCK_PHLO + 1;
        over.state.deploys = vec![single];
        assert_eq!(block_phlo(&over), BlockStatus::ExceedsBlockPhloLimit);
    }

    /// The sum saturates rather than wrapping. Two maximal limits are the adversarial case: without
    /// saturation they would overflow back below the budget and the cap would admit exactly the block it
    /// exists to refuse.
    #[test]
    fn the_block_phlo_sum_saturates_rather_than_wrapping() {
        let mut b = block();
        let mut first = deploy(0, 1, "root");
        first.deploy.data.phlo_limit = i64::MAX;
        let mut second = deploy(0, 1, "root");
        second.deploy.data.phlo_limit = i64::MAX;
        b.state.deploys = vec![first, second];

        assert_eq!(
            block_phlo(&b),
            BlockStatus::ExceedsBlockPhloLimit,
            "two maximal limits must not wrap back under the budget"
        );
    }

    fn block() -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([0xab; 32]),
            block_number: 10.try_into().unwrap(),
            sender: Validator::new([0x11; 65]),
            seq_num: 0.try_into().unwrap(),
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([1u8; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([2u8; 32]),
            justifications: vec![],
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: RholangState {
                deploys: vec![],
                system_deploys: vec![],
            },
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![1],
            timestamp: 0,
        }
    }

    #[test]
    fn version_and_format_checks() {
        let mut b = block();
        assert!(version(&b));
        b.version = 2;
        assert!(!version(&b));
        b.version = 1;
        assert!(format_of_fields(&b));
        b.sig = vec![];
        assert!(!format_of_fields(&b));
    }

    #[test]
    fn format_of_fields_rejects_non_ascii_shard_id() {
        let mut b = block();
        b.shard_id = "røøt".to_string();
        assert!(!format_of_fields(&b));
    }

    #[test]
    fn block_hash_detects_tampering() {
        let mut b = block();
        let h = hash_block(&b);
        b.block_hash = h;
        assert!(block_hash(&b));
        b.block_number = 999.try_into().unwrap();
        assert!(!block_hash(&b));
    }

    #[test]
    fn deploy_validators() {
        let mut b = block();
        b.state.deploys = vec![deploy(5, 10, "root")];
        assert_eq!(future_transaction(&b), BlockStatus::Valid);
        assert_eq!(transaction_expiration(&b, 100), BlockStatus::Valid);
        assert_eq!(deploys_shard_identifier(&b, "root"), BlockStatus::Valid);
        assert_eq!(phlo_price(&b, 10), BlockStatus::Valid);

        b.state.deploys = vec![deploy(20, 10, "root")];
        assert_eq!(future_transaction(&b), BlockStatus::ContainsFutureDeploy);

        b.state.deploys = vec![deploy(0, 10, "other")];
        assert_eq!(
            deploys_shard_identifier(&b, "root"),
            BlockStatus::InvalidDeployShardId
        );

        b.state.deploys = vec![deploy(5, 1, "root")];
        assert_eq!(phlo_price(&b, 10), BlockStatus::ContainsLowCostDeploy);
    }

    #[test]
    fn deploy_expiration_boundary_is_inclusive() {
        let mut b = block();
        b.block_number = 51.try_into().unwrap();

        b.state.deploys = vec![deploy(1, 10, "root")];
        assert_eq!(
            transaction_expiration(&b, 50),
            BlockStatus::ContainsExpiredDeploy,
            "valid_after=1 is already expired in block 51: 1 <= 51 - 50"
        );

        b.state.deploys = vec![deploy(2, 10, "root")];
        assert_eq!(
            transaction_expiration(&b, 50),
            BlockStatus::Valid,
            "the next valid_after value must remain eligible at the same height"
        );
    }

    /// **The regression test for AUDIT C109**, and it is written against the *block* path on purpose.
    ///
    /// The mint was reachable two ways. A local deploy could not carry a negative limit — the ingress
    /// at `BlockApiImpl::deploy` checked for it — so a test placed there would have passed while the
    /// bug was live, which is the shape of a test that proves nothing. What *was* open is this path:
    /// a peer's block arrives, and the checks a validator runs on it (`deploys_shard_identifier`,
    /// `future_transaction`, `transaction_expiration`, `phlo_price`) all read the deploy data and none
    /// of them looked at the limit's sign. The block was accepted, replayed, and the negative charge
    /// credited its own author.
    ///
    /// Three assertions, because one is not enough to call it closed: the pure check rejects the
    /// block, the charge cannot be computed at all, and the arithmetic that used to invert is gone.
    #[test]
    fn a_negative_phlo_limit_cannot_reach_the_charge() {
        let mut negative = deploy(5, 10, "root");
        negative.deploy.data.phlo_limit = -100;
        assert_eq!(
            negative.deploy.data.total_phlo_charge(),
            None,
            "a negative limit must not produce a charge — this is the value that used to be \
             subtracted from the deployer's vault, i.e. added to it"
        );

        let mut b = block();
        b.state.deploys = vec![negative];
        assert_eq!(
            phlo_limit(&b),
            BlockStatus::InvalidPhloLimit,
            "a peer's block carrying a negative phlo limit must be refused by name, before replay"
        );
        assert_eq!(
            phlo_price(&b, 10),
            BlockStatus::Valid,
            "the rejection is the limit's, not the price's — the two checks must not be the same check"
        );

        // The control: the same deploy with a non-negative limit is valid, so the test above is
        // failing for the sign and not because the block builder produces something invalid anyway.
        let mut positive = deploy(5, 10, "root");
        positive.deploy.data.phlo_limit = 0;
        b.state.deploys = vec![positive];
        assert_eq!(phlo_limit(&b), BlockStatus::Valid);
    }

    /// **The regression test for AUDIT C110.** A `Slash` is a system deploy: unsigned by its victim,
    /// it moves that victim's whole bond to the Coop vault, and every validator re-executes it during
    /// replay. The rule that makes one justified — the victim is the sender of a `slashable`
    /// justification of the block — used to exist only on the proposer's side, so the validators that
    /// *executed* the punishment had nothing to check it against.
    ///
    /// This pins the two halves that make the receiver's check possible: which validators a block
    /// actually slashes (read off its system deploys), and which its evidence holds responsible (read
    /// off this node's own metadata). The subset test between them is the check itself.
    #[test]
    fn a_slash_is_justified_only_by_a_slashable_justification_from_its_victim() {
        use rchain_models::block_metadata::BlockMetadata;
        use rchain_models::casper::protocol::casper_message::{
            ProcessedSystemDeploy, SystemDeployData,
        };

        let offender = Validator::new([0x22; 65]);
        let innocent = Validator::new([0x33; 65]);

        let mut b = block();
        b.state.system_deploys = vec![ProcessedSystemDeploy::Succeeded {
            event_list: vec![],
            system_deploy: SystemDeployData::Slash {
                validator: offender,
                severity: SlashSeverity::Malicious,
                evidence: None,
            },
        }];
        assert_eq!(
            slashed_validators(&b),
            BTreeMap::from([(offender, SlashSeverity::Malicious)])
        );

        // Evidence that holds `offender` responsible **at the tier the slash claims**: covered.
        let mut meta = BlockMetadata::from_block(&b);
        meta.sender = offender;
        meta.slashable = true;
        meta.slash_severity = SlashSeverity::Malicious;
        let justified = slashable_senders(&[meta.clone()]);
        let covers = |b: &BlockMessage, justified: &BTreeMap<_, _>| {
            slashed_validators(b)
                .iter()
                .all(|(v, tier)| justified.get(v) == Some(tier))
        };
        assert!(
            covers(&b, &justified),
            "a slash of the validator its evidence condemns, at the tier the evidence derives, must \
             be covered"
        );

        // **The tier is checked, not taken** (AUDIT C199). The same victim, the same evidence — and a
        // slash claiming a harsher tier than the offence earns is refused. Without this a proposer
        // would size the confiscation freely and the tiers would be advice.
        let mut overclaimed = b.clone();
        overclaimed.state.system_deploys = vec![ProcessedSystemDeploy::Succeeded {
            event_list: vec![],
            system_deploy: SystemDeployData::Slash {
                validator: offender,
                severity: SlashSeverity::Misdemeanour,
                evidence: None,
            },
        }];
        assert!(
            !covers(&overclaimed, &justified),
            "a slash whose tier is not the one this node derives must be refused"
        );

        // The attack this check exists for: the block slashes `innocent`, and nothing in the
        // justifications holds `innocent` responsible. Before this check, the block was accepted on
        // the strength of its post-state hash alone.
        b.state.system_deploys = vec![ProcessedSystemDeploy::Succeeded {
            event_list: vec![],
            system_deploy: SystemDeployData::Slash {
                validator: innocent,
                severity: SlashSeverity::Malicious,
                evidence: None,
            },
        }];
        assert!(
            !covers(&b, &justified),
            "slashing a validator no justification holds responsible must be refused — this is the \
             stake-confiscation path a proposer could take against any bonded peer"
        );

        // `validation_failed` alone does not authorize: a node that could not replay a block locally
        // says nothing about its sender, so that flag must not make a slash justified.
        let mut merely_failed = BlockMetadata::from_block(&b);
        merely_failed.sender = innocent;
        merely_failed.validation_failed = true;
        merely_failed.slashable = false;
        assert!(
            !covers(&b, &slashable_senders(&[merely_failed])),
            "an unattributable local failure must not license a slash"
        );
    }
    /// The deploy-signature check (AUDIT C120), pinned at the pure level as well as through
    /// `block_summary` below.
    ///
    /// The `deploy(...)` fixture is unsigned by construction — an empty `deployer` with `sig =
    /// vec![1]`, the same shape `casper/tests/consensus.rs` builds — so it *is* the forged deploy the
    /// check exists to refuse; the control is the same fixture signed by a real key.
    ///
    /// The last case is the boundary, and it is the reason this check reads `state.deploys` and never
    /// `state.system_deploys`: a `Slash` is a system deploy, unsigned by its victim by design, and
    /// widening the check to "every deploy" would make every block carrying a legitimate slash
    /// unrefusable-or-not on a signature it was never supposed to have. What makes a slash legitimate
    /// is `slashed_validators`/`slashable_senders`, applied in `validate_block_checkpoint` (C110).
    #[test]
    fn a_deploy_that_its_named_key_did_not_sign_is_refused() {
        use rchain_models::casper::protocol::casper_message::{
            ProcessedSystemDeploy, SystemDeployData,
        };

        let mut b = block();
        b.state.deploys = vec![deploy(5, 10, "root")];
        assert_eq!(
            deploy_signatures(&b),
            BlockStatus::InvalidDeploySignature,
            "the unsigned fixture is the forged deploy: a block must not charge the key it names"
        );

        b.state.deploys = vec![super::effectful_tests::signed_deploy("Nil")];
        assert_eq!(deploy_signatures(&b), BlockStatus::Valid);

        b.state.deploys = vec![];
        b.state.system_deploys = vec![ProcessedSystemDeploy::Succeeded {
            event_list: vec![],
            system_deploy: SystemDeployData::Slash {
                validator: Validator::new([0x44; 65]),
                severity: SlashSeverity::Malicious,
                evidence: None,
            },
        }];
        assert_eq!(
            deploy_signatures(&b),
            BlockStatus::Valid,
            "a slash is a system deploy and is unsigned by its victim; this check must not read it"
        );
    }
}

#[cfg(test)]
mod effectful_tests {
    use super::*;
    use async_trait::async_trait;
    use rchain_block_storage::dag::codecs::{BlockHashCodec, BlockMessageCodec};
    use rchain_block_storage::dag::dag_storage::DeployId;
    use rchain_block_storage::dag::message_state::DagMessageState;
    use rchain_block_storage::dag::representation::DagRepresentation;
    use rchain_models::block_metadata::BlockMetadata;
    use rchain_models::casper::protocol::casper_message::{
        DeployData, PCost, ProcessedDeploy, SignedDeployData,
    };
    use rchain_shared::store::InMemoryKeyValueStore;
    use rchain_shared::typed_store::KeyValueTypedStoreCodec;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    fn hash(byte: u8) -> BlockHash {
        let mut bytes = [0u8; 32];
        bytes[0] = byte;
        BlockHash::new(bytes)
    }

    fn meta(
        hash: BlockHash,
        block_num: i64,
        sender_byte: u8,
        seq: i64,
        failed: bool,
    ) -> BlockMetadata {
        BlockMetadata {
            block_hash: hash,
            block_num: rchain_shared::refined::BlockHeight::try_from(block_num).unwrap(),
            sender: Validator::new([sender_byte; 65]),
            seq_num: rchain_shared::refined::SeqNum::try_from(seq).unwrap(),
            justifications: BTreeSet::new(),
            bonds_map: BTreeMap::new(),
            validated: true,
            validation_failed: failed,
            slashable: false,
            failure_cause: None,
            slash_severity: SlashSeverity::Unspecified,
            restore_attempts: 0,
            fringe: BTreeSet::new(),
            fringe_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            member_of_fringe: None,
        }
    }

    struct MockDag {
        metadata: BTreeMap<BlockHash, BlockMetadata>,
        representation: DagRepresentation,
    }

    #[async_trait]
    impl BlockDagStorage for MockDag {
        async fn get_representation(&self) -> Arc<DagRepresentation> {
            Arc::new(self.representation.clone())
        }
        async fn insert(&self, _m: BlockMetadata, _b: BlockMessage) -> Result<(), String> {
            Ok(())
        }
        async fn lookup(&self, h: &BlockHash) -> Result<Option<BlockMetadata>, String> {
            Ok(self.metadata.get(h).cloned())
        }
        async fn lookup_by_deploy_id(&self, _d: &DeployId) -> Result<Option<BlockHash>, String> {
            Ok(None)
        }
        async fn add_deploy(&self, _d: SignedDeployData) -> Result<(), String> {
            Ok(())
        }
        async fn pooled_deploys(&self) -> Result<BTreeMap<DeployId, SignedDeployData>, String> {
            Ok(BTreeMap::new())
        }
        async fn contains_deploy_in_pool(&self, _d: &DeployId) -> Result<bool, String> {
            Ok(false)
        }
    }

    fn mock(metadata: BTreeMap<BlockHash, BlockMetadata>) -> MockDag {
        MockDag {
            metadata,
            representation: DagRepresentation {
                dag_set: Arc::new(BTreeSet::new()),
                child_map: Arc::new(BTreeMap::new()),
                height_map: Arc::new(BTreeMap::new()),
                dag_message_state: DagMessageState::empty(),
            },
        }
    }

    fn block(
        sender_byte: u8,
        block_num: i64,
        seq: i64,
        justifications: Vec<BlockHash>,
    ) -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: hash(0xee),
            block_number: rchain_shared::refined::BlockHeight::try_from(block_num).unwrap(),
            sender: Validator::new([sender_byte; 65]),
            seq_num: rchain_shared::refined::SeqNum::try_from(seq).unwrap(),
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([1u8; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([2u8; 32]),
            justifications,
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: rchain_models::casper::protocol::casper_message::RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![1],
            timestamp: 0,
        }
    }

    #[tokio::test]
    async fn block_number_must_be_parent_max_plus_one() {
        let parent = hash(1);
        let dag = mock(BTreeMap::from([(parent, meta(parent, 4, 1, 0, false))]));
        let b = block(2, 5, 0, vec![parent]);
        assert_eq!(block_number(&dag, &b).await.unwrap(), Ok(()));

        let bad = block(2, 6, 0, vec![parent]);
        assert_eq!(
            block_number(&dag, &bad).await.unwrap(),
            Err(BlockStatus::InvalidBlockNumber)
        );
    }

    /// **A failed parent still sets the height its child claims to build on** (AUDIT C173, #125).
    ///
    /// Skipping the failed justification made a failure hide its own block: the child was refused
    /// `InvalidBlockNumber` for the parent's *record* alone, here in `block_summary`, before
    /// `neglected_invalid_block` could run. That refusal is what estranged a node from a bonded peer
    /// permanently on the 2026-09-29 testnet run — the child of the failed block could not be admitted
    /// at all, whatever else was true of it.
    ///
    /// Red before the change: with the failed parent at height 4, this child was refused because the
    /// maximum over non-failed parents was -1.
    #[tokio::test]
    async fn a_failed_parent_still_sets_its_childs_height() {
        let failed_parent = hash(1);
        let dag = mock(BTreeMap::from([(
            failed_parent,
            meta(failed_parent, 4, 1, 0, true),
        )]));

        // The child of the failed parent, claiming the height its parent implies, is admitted to the
        // other checks rather than refused here.
        let child = block(2, 5, 0, vec![failed_parent]);
        assert_eq!(block_number(&dag, &child).await.unwrap(), Ok(()));

        // And the claim is still checked: one above it is wrong, and still refused.
        let too_high = block(2, 6, 0, vec![failed_parent]);
        assert_eq!(
            block_number(&dag, &too_high).await.unwrap(),
            Err(BlockStatus::InvalidBlockNumber)
        );
    }

    /// The **H1b descent bound is unchanged**: a failed parent at or above the child's own height is
    /// refused, whether or not the maximum now counts it. The two rules are separate, and this pins
    /// that counting failed justifications did not quietly widen the bound that refuses a failed parent
    /// *above* its child — the property `dag.rs`'s `h1b_…` witness exists for.
    #[tokio::test]
    async fn a_failed_parent_at_or_above_the_child_is_still_refused() {
        let failed_parent = hash(1);
        let dag = mock(BTreeMap::from([(
            failed_parent,
            meta(failed_parent, 7, 1, 0, true),
        )]));

        let child = block(2, 5, 0, vec![failed_parent]);
        assert_eq!(
            block_number(&dag, &child).await.unwrap(),
            Err(BlockStatus::InvalidBlockNumber)
        );
    }

    #[tokio::test]
    async fn sequence_number_must_be_creator_latest_plus_one() {
        let parent = hash(1);
        let dag = mock(BTreeMap::from([(parent, meta(parent, 4, 1, 2, false))]));
        let b = block(1, 5, 3, vec![parent]);
        assert_eq!(sequence_number(&dag, &b).await.unwrap(), Ok(()));

        let bad = block(1, 5, 4, vec![parent]);
        assert_eq!(
            sequence_number(&dag, &bad).await.unwrap(),
            Err(BlockStatus::InvalidSequenceNumber)
        );
    }

    #[tokio::test]
    async fn neglected_invalid_block_detects_bonded_invalid_justification() {
        let invalid = hash(1);
        let dag = mock(BTreeMap::from([(invalid, meta(invalid, 0, 1, 0, true))]));
        let mut b = block(2, 1, 0, vec![invalid]);
        b.bonds
            .insert(Validator::new([1u8; 65]), 100.try_into().unwrap());
        assert_eq!(
            neglected_invalid_block(&dag, &b).await.unwrap(),
            Err(BlockStatus::NeglectedInvalidBlock)
        );

        b.bonds.clear();
        assert_eq!(neglected_invalid_block(&dag, &b).await.unwrap(), Ok(()));
    }

    fn deploy(sig: u8) -> ProcessedDeploy {
        ProcessedDeploy {
            deploy: SignedDeployData {
                data: DeployData {
                    attachments: Vec::new(),
                    term: "Nil".to_string(),
                    timestamp: 0,
                    phlo_price: 1,
                    phlo_limit: 1,
                    valid_after_block_number: 0,
                    shard_id: "root".to_string(),
                },
                deployer: vec![],
                sig: vec![sig],
                sig_algorithm: "secp256k1".to_string(),
            },
            cost: PCost { cost: 0 },
            deploy_log: vec![],
            is_failed: false,
            system_deploy_error: None,
        }
    }

    async fn block_store(blocks: Vec<BlockMessage>) -> BlockStore {
        let store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
            Arc::new(tokio::sync::Mutex::new(Box::new(
                InMemoryKeyValueStore::default(),
            ))),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMessageCodec),
        ));
        let pairs: Vec<(BlockHash, BlockMessage)> =
            blocks.into_iter().map(|b| (b.block_hash, b)).collect();
        store.put(&pairs).await.unwrap();
        store
    }

    /// A deploy signed by a real key, so its `deployer` field and its signature agree (AUDIT C120).
    ///
    /// The forged twins in the test below are built from this by changing *one* side of that
    /// agreement, because that is the whole attack surface: the signature an attacker can produce, and
    /// the identity it wants to spend from.
    pub(super) fn signed_deploy(term: &str) -> ProcessedDeploy {
        signed_deploy_with_phlo(term, 1)
    }

    /// As [`signed_deploy`], with a chosen phlo limit.
    ///
    /// The limit is part of the data the signature covers, so a test that mutates it *after* signing
    /// is testing `deploy_signatures` rather than whatever it meant to test — which is how this helper
    /// came to exist: the first version of the block-budget test below did exactly that and was refused
    /// with `InvalidDeploySignature`, correctly.
    pub(super) fn signed_deploy_with_phlo(term: &str, phlo_limit: i64) -> ProcessedDeploy {
        use rchain_crypto::signatures::secp256k1::Secp256k1;
        use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
        use rchain_crypto::signatures::signed::Signed;

        let (sec, _pk) = Secp256k1.new_key_pair();
        let data = DeployData {
            attachments: Vec::new(),
            term: term.to_string(),
            timestamp: 0,
            phlo_price: 1,
            phlo_limit,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        };
        let signed = Signed::new(data, &Secp256k1, &sec).expect("signing a well-formed deploy");
        ProcessedDeploy {
            deploy: SignedDeployData {
                data: signed.data,
                deployer: signed.pk.bytes().to_vec(),
                sig: signed.sig,
                sig_algorithm: signed.sig_algorithm.name().to_string(),
            },
            cost: PCost { cost: 0 },
            deploy_log: vec![],
            is_failed: false,
            system_deploy_error: None,
        }
    }

    /// A second, independent public key — the "victim" whose identity a proposer wants to borrow.
    pub(super) fn another_public_key() -> Vec<u8> {
        use rchain_crypto::signatures::secp256k1::Secp256k1;
        use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
        Secp256k1.new_key_pair().1.bytes().to_vec()
    }

    /// **The regression test for AUDIT C120**, and it is written against the *block* path because that
    /// is where the gap was.
    ///
    /// `SignedDeployData::verify_signature` was correct in itself and had exactly one production
    /// caller: the deploy *ingress* (`BlockApiImpl::deploy`). A block arriving from a peer carries its
    /// deploys as `ProcessedDeploy`s whose `deployer` and `sig` are copied off the wire
    /// (`ProcessedDeploy::from_proto`), and every check a validator ran on them — shard id, validity
    /// window, phlo price, phlo limit, signature *deduplication* — read those fields without ever
    /// asking whether the key they name had signed the data. So a bonded proposer could name a
    /// victim's key, put arbitrary bytes in `sig`, and have every other validator debit that victim,
    /// pay the attacker's term, and arrive at a post-state hash they computed honestly: `block_signature`
    /// passes (the proposer signed its own block), the summary passed, the bonds cache agreed, and
    /// nothing was attributable.
    ///
    /// The assertion is on `block_summary` rather than on the check alone on purpose. The defect was
    /// never "a check is missing from this file" — it was "the check is missing from this *path*", and
    /// a test of the check by itself would have passed while the bug was live.
    ///
    /// Four cases: an honest deploy (the control), an impersonation that carries a *valid* signature
    /// made by a different key, the register's own `sig = vec![1]` under a victim's key, and a term
    /// changed after signing. The control is first so a refusal below cannot be a refusal for some
    /// unrelated reason, and the third and fourth are separated deliberately: a check reading only the
    /// signature's validity would catch the fourth and miss the second.
    #[tokio::test]
    async fn a_block_whose_deploy_is_not_signed_by_its_named_deployer_is_refused() {
        let dag = mock(BTreeMap::new());
        let store = block_store(vec![]).await;
        let block_at = |deploys: Vec<ProcessedDeploy>| {
            let mut b = block(2, 0, 0, vec![]);
            b.state.deploys = deploys;
            b
        };

        let honest = signed_deploy("Nil");
        assert!(
            honest.deploy.verify_signature(),
            "the honest fixture must satisfy the check the block path is about to run"
        );
        assert_eq!(
            block_summary(&dag, &store, &block_at(vec![honest]), "root", 100, 1, 0)
                .await
                .unwrap(),
            Ok(()),
            "an honestly signed deploy must still reach the end of the pure checks — the control that \
             makes the refusals below about the signature and not about the fixture"
        );

        // The strongest form of the attack: the signature is *real*, made by the attacker's own key,
        // and only the `deployer` field names someone else. A check that asked "is this a valid
        // signature?" rather than "is it *this key's* signature?" would pass this.
        let mut impersonation = signed_deploy("Nil");
        impersonation.deploy.deployer = another_public_key();
        assert!(
            !impersonation.deploy.verify_signature(),
            "a deploy whose deployer names a key that did not sign it must not verify"
        );
        assert_eq!(
            block_summary(&dag, &store, &block_at(vec![impersonation]), "root", 100, 1, 0)
                .await
                .unwrap(),
            Err(BlockStatus::InvalidDeploySignature),
            "a block whose deploy names a key that did not sign it must be refused before replay — \
             this is the block that debits a victim's vault and pays the proposer's term"
        );

        // The register's own trigger, as written: a victim's key with `sig = vec![1]`.
        let mut garbage = signed_deploy("Nil");
        garbage.deploy.sig = vec![1];
        assert_eq!(
            block_summary(&dag, &store, &block_at(vec![garbage]), "root", 100, 1, 0)
                .await
                .unwrap(),
            Err(BlockStatus::InvalidDeploySignature),
            "a block carrying a deploy whose signature is arbitrary bytes must be refused"
        );

        // The signature is over the *message*, not only over the key: a term changed after signing
        // must not verify. This is the assertion that would fail if the check ever hashed a prefix of
        // the deploy data instead of the whole of it (the sibling defect in AUDIT C134).
        let mut tampered = signed_deploy("Nil");
        tampered.deploy.data.term = "Nil | Nil".to_string();
        assert_eq!(
            block_summary(&dag, &store, &block_at(vec![tampered]), "root", 100, 1, 0)
                .await
                .unwrap(),
            Err(BlockStatus::InvalidDeploySignature),
            "a deploy whose term does not match what was signed must be refused"
        );
    }

    /// **A block's timestamp is never a validity input** (W-N14-01, #43).
    ///
    /// The header timestamp is the proposer's informational wall clock, served to contracts through
    /// `rho:block:data`. It is not trusted and validators do not judge it: a block stamped an hour
    /// before its parent, or a day after it, is accepted exactly as one stamped on time. A bound here
    /// was briefly added and removed, because an honest proposer stamps its own clock, so a single
    /// parent stamped in the future made every honest child invalid until real time caught up.
    ///
    /// Falsified by reintroducing any timestamp predicate into `block_summary`.
    #[tokio::test]
    async fn a_block_timestamp_is_never_a_validity_input() {
        let parent_hash = hash(7);
        let dag = mock(BTreeMap::from([(
            parent_hash,
            meta(parent_hash, 0, 1, 0, false),
        )]));

        let parent_ms = 1_700_000_000_000i64;
        let mut parent = block(1, 0, 0, vec![]);
        parent.block_hash = parent_hash;
        parent.timestamp = parent_ms;
        let store = block_store(vec![parent]).await;

        // An equality-only oracle can pass vacuously when every variant fails
        // for an unrelated reason. First prove this fixture is otherwise valid.
        let mut on_time = block(1, 1, 1, vec![parent_hash]);
        on_time.timestamp = parent_ms;
        let reference = block_summary(&dag, &store, &on_time, "root", 100, 1, 0)
            .await
            .unwrap();
        assert_eq!(
            reference,
            Ok(()),
            "the on-time control must be admitted before comparing timestamps"
        );

        for stamp in [
            parent_ms - 3_600_000,  // an hour before the parent
            parent_ms + 86_400_000, // a day after it (a catching-up validator)
            0,
            i64::MAX,
        ] {
            let mut child = block(1, 1, 1, vec![parent_hash]);
            child.timestamp = stamp;
            assert_eq!(
                block_summary(&dag, &store, &child, "root", 100, 1, 0)
                    .await
                    .unwrap(),
                reference,
                "timestamp {stamp} must not change the verdict"
            );
        }
    }

    #[tokio::test]
    async fn repeat_deploy_detects_duplicate_sig_in_parent_chain() {
        let genesis = hash(1);
        let parent = hash(2);
        let dag = mock(BTreeMap::from([
            (genesis, meta(genesis, 0, 1, 0, false)),
            (parent, meta(parent, 1, 1, 1, false)),
        ]));

        let mut genesis_block = block(1, 0, 0, vec![]);
        genesis_block.block_hash = genesis;
        genesis_block.state.deploys = vec![deploy(9)];
        let mut parent_block = block(1, 1, 1, vec![genesis]);
        parent_block.block_hash = parent;
        parent_block.state.deploys = vec![deploy(1)];
        let store = block_store(vec![genesis_block, parent_block]).await;

        // Current block reuses the parent's deploy sig [1].
        let mut current = block(1, 2, 2, vec![parent]);
        current.state.deploys = vec![deploy(1)];
        assert_eq!(
            repeat_deploy(&dag, &store, &current, 100).await.unwrap(),
            Err(BlockStatus::InvalidRepeatDeploy)
        );

        // Current block with a fresh deploy sig is valid.
        let mut current = block(1, 2, 2, vec![parent]);
        current.state.deploys = vec![deploy(7)];
        assert_eq!(
            repeat_deploy(&dag, &store, &current, 100).await.unwrap(),
            Ok(())
        );
    }

    /// An unsupported block `version` is refused *by the summary* (AUDIT F-6).
    ///
    /// This is the wiring, not the predicate — and the distinction is the whole finding. `block_version`
    /// had a unit test from the port onward, so a test of the predicate alone would pass in exactly the
    /// state the field was in: a predicate, a test, and no caller. Only going through `block_summary`
    /// pins the check into the acceptance path, so removing it from the array turns this red.
    #[tokio::test]
    async fn the_block_summary_refuses_an_unsupported_version() {
        let dag = mock(BTreeMap::new());
        let store = block_store(vec![]).await;

        let mut supported = block(2, 0, 0, vec![]);
        supported.state.deploys = vec![signed_deploy("Nil")];
        assert_eq!(
            block_summary(&dag, &store, &supported, "root", 100, 1, 0)
                .await
                .unwrap(),
            Ok(()),
            "the control: a supported version reaches the end of the pure checks, so the refusal below \
             is about the version and not about the fixture"
        );

        let mut unsupported = supported.clone();
        unsupported.version = 999;
        assert_eq!(
            block_summary(&dag, &store, &unsupported, "root", 100, 1, 0)
                .await
                .unwrap(),
            Err(BlockStatus::InvalidVersion),
            "an unsupported version must be refused by the summary, not merely by a predicate nobody calls"
        );
    }

    /// The per-block phlo cap is enforced *by the summary*, not merely by its predicate (AUDIT F-3).
    ///
    /// The same distinction as the version test above, and it was a real gap in this change's first
    /// form: `a_block_exceeding_the_block_phlo_budget_is_refused` calls `block_phlo` directly, so it
    /// would have passed with the check absent from `block_summary`'s array — which is the state the
    /// version field sat in for the whole life of the port.
    ///
    /// The deploy-count cap has no equivalent here: building a 256-deploy block means 256 signatures,
    /// too slow for a unit test. It is pinned by its predicate and by reading the array, and this
    /// comment is the record that it is not pinned end to end.
    #[tokio::test]
    async fn the_block_summary_enforces_the_block_phlo_budget() {
        let dag = mock(BTreeMap::new());
        let store = block_store(vec![]).await;

        let mut over = block(2, 0, 0, vec![]);
        // Signed *with* the oversized limit rather than mutated after signing: the limit is inside the
        // signed data, so mutating it would be refused by `deploy_signatures` first and this test would
        // be green for the wrong reason.
        over.state.deploys = vec![signed_deploy_with_phlo("Nil", MAX_BLOCK_PHLO + 1)];

        assert_eq!(
            block_summary(&dag, &store, &over, "root", 100, 1, 0)
                .await
                .unwrap(),
            Err(BlockStatus::ExceedsBlockPhloLimit),
            "an over-budget block must be refused by the summary, not merely by a predicate nobody calls"
        );
    }

    // --- Equivocation evidence (AUDIT C200) ------------------------------------------------------

    use rchain_models::casper::protocol::casper_message::EquivocationEvidence;

    /// A real secp256k1 key pair, fixed by a seed byte so the fixture is deterministic.
    ///
    /// A *real* key rather than a byte pattern: the evidence's whole acceptance rule is that the
    /// signature verifies against the named sender, so a fixture that cannot sign proves nothing
    /// about it.
    fn identity(byte: u8) -> crate::validator_identity::ValidatorIdentity {
        use rchain_crypto::private_key::PrivateKey;
        crate::validator_identity::ValidatorIdentity::from_private_key(PrivateKey::new(vec![
            byte;
            32
        ]))
        .expect("a fixed 32-byte scalar is a valid key")
    }

    /// A signed block by `id` at `seq`, distinguishable from its twin by `marker` — which changes its
    /// content hash, so the two are a genuine conflict rather than the same block twice.
    fn signed_block(
        id: &crate::validator_identity::ValidatorIdentity,
        seq: i64,
        marker: u8,
    ) -> BlockMessage {
        let base = BlockMessage {
            version: rchain_models::block_version::CURRENT,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([0u8; 32]),
            block_number: rchain_shared::refined::BlockHeight::try_from(1).unwrap(),
            sender: Validator::from_slice(&id.public_key.bytes()),
            seq_num: rchain_shared::refined::SeqNum::try_from(seq).unwrap(),
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([marker; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([marker; 32]),
            justifications: Vec::new(),
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: rchain_models::casper::protocol::casper_message::RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![],
            timestamp: 0,
        };
        id.sign_block(&base).expect("signing with a real key")
    }

    /// A DAG holding exactly these blocks, as messages — the receiver's own view, which is what
    /// `equivocation_is_proved` checks the evidence against.
    fn dag_holding(blocks: &[BlockMessage]) -> MockDag {
        let msg_map = blocks
            .iter()
            .map(|b| {
                (
                    b.block_hash,
                    rchain_block_storage::dag::finalizer::Message {
                        id: b.block_hash,
                        height: b.block_number,
                        sender: b.sender,
                        sender_seq: b.seq_num,
                        bonds_map: BTreeMap::new(),
                        parents: BTreeSet::new(),
                        fringe: BTreeSet::new(),
                        seen: Arc::new(BTreeSet::new()),
                    },
                )
            })
            .collect();
        MockDag {
            metadata: BTreeMap::new(),
            representation: DagRepresentation {
                dag_set: Arc::new(BTreeSet::new()),
                child_map: Arc::new(BTreeMap::new()),
                height_map: Arc::new(BTreeMap::new()),
                dag_message_state: DagMessageState::from_parts(BTreeMap::new(), msg_map),
            },
        }
    }

    /// The header a proposer would attach for `block`.
    fn evidence_for(block: &BlockMessage) -> Vec<u8> {
        rchain_models::casper::protocol::casper_message::EquivocationEvidence::from_block(block)
            .encode()
    }

    /// **The C200 falsifier: evidence proves an equivocation only when it is a *signed conflict*, and
    /// the check needs nothing the receiver does not already have.**
    ///
    /// Two conditions, one arm each: the offender signed it, and the DAG already holds a **different**
    /// block by that sender at the same `seq_num`. The conflict arm is the only one that is an offence —
    /// and it is checked against the *receiver's own* DAG, which is why a node that never saw the
    /// refused block still needs no trust in the proposer. Red before this: the function did not exist,
    /// and an equivocation could be refused and never punished.
    #[tokio::test]
    async fn equivocation_evidence_is_proved_only_by_a_signed_conflict() {
        let me = identity(0x11);
        let other = identity(0x22);
        let held = signed_block(&me, 3, 0x01);
        let conflict = signed_block(&me, 3, 0x02);
        let offender = Validator::from_slice(&me.public_key.bytes());
        let dag = dag_holding(&[held.clone()]);

        // The offence: a second, differently-hashed block at a `(sender, seq_num)` the DAG holds.
        assert!(
            equivocation_is_proved(&dag, &offender, &evidence_for(&conflict))
                .await
                .unwrap(),
            "two signed blocks at one sequence number is an equivocation"
        );

        // A block at a *fresh* sequence number is just a block.
        assert!(
            !equivocation_is_proved(&dag, &offender, &evidence_for(&signed_block(&me, 4, 0x02)))
                .await
                .unwrap(),
            "a block the DAG has no counterpart for proves nothing"
        );

        // …and neither is a block signed by somebody else, whatever it says.
        assert!(
            !equivocation_is_proved(
                &dag,
                &offender,
                &evidence_for(&signed_block(&other, 3, 0x03))
            )
            .await
            .unwrap(),
            "the evidence must be the *offender's* own signature"
        );

        // **A signature that is not the offender's over the offender's own hash** — the proposer
        // relabelling somebody else's block as the victim's. The sender field is what catches it, and
        // this is the arm that shows the sender check is not redundant with the hash.
        let mut relabelled = signed_block(&other, 3, 0x03);
        relabelled.sender = offender;
        assert!(
            !equivocation_is_proved(&dag, &offender, &evidence_for(&relabelled))
                .await
                .unwrap(),
            "a signature by a key the header does not name proves nothing"
        );

        // A payload that is not evidence proves nothing rather than erroring: a refusal is what a
        // payload that proves nothing earns.
        assert!(
            !equivocation_is_proved(&dag, &offender, b"not a block")
                .await
                .unwrap(),
            "garbage must be refused, not accepted or raised"
        );
    }

    /// **The same evidence is proved by one receiver and not by another** (W-N9-06).
    ///
    /// The row: a slash is justified by equivocation evidence, and that route is adjudicated against
    /// the *receiving* node's own DAG — so one slashing block is valid for a caught-up node and refused
    /// `UnjustifiedSlash` by an honest node that has not yet received the victim's first block. Bond
    /// maps then diverge between the two branching views, which is the split-shaped consequence the
    /// adversary statement forbids.
    ///
    /// The test above exercises one DAG and never compares two receivers' views. This is the second
    /// receiver, and the difference between the two assertions is the whole of the row.
    ///
    /// Falsified by putting the evidence *inside* the slashing block (which would make the verdict a
    /// function of what was signed rather than of what the receiver happens to hold) — the second
    /// assertion then reads `true`.
    #[tokio::test]
    async fn the_same_evidence_is_proved_by_one_receiver_and_not_by_another() {
        let me = identity(0x11);
        let held = signed_block(&me, 3, 0x01);
        let conflict = signed_block(&me, 3, 0x02);
        let offender = Validator::from_slice(&me.public_key.bytes());
        let evidence = evidence_for(&conflict);

        let caught_up = dag_holding(&[held.clone()]);
        let behind = dag_holding(&[]);

        assert!(
            equivocation_is_proved(&caught_up, &offender, &evidence)
                .await
                .unwrap(),
            "a node holding the victim's first block adjudicates the evidence"
        );
        assert!(
            !equivocation_is_proved(&behind, &offender, &evidence)
                .await
                .unwrap(),
            "**and a node that has not received it refuses the same evidence** (W-N9-06): the \
             verdict is a fact about the receiver's DAG, not about what was signed, so two honest \
             nodes disagree about a slashing block and their bond maps follow them apart"
        );
    }

    /// **The evidence is a header, and its size is fixed by its own fields** (AUDIT C200).
    ///
    /// This is the arm that keeps a peer from writing into consensus state: the payload rides in a
    /// slashing block's `state`, so if it carried the refused *block* its size would be whatever that
    /// peer chose — a block refused at the H-1 gate has passed no check at all. The header cannot
    /// exceed its fields no matter what the offender signs, and the bound is asserted rather than
    /// argued because the next person to "simplify" this into a whole-block payload would otherwise
    /// only find out from a chain.
    #[test]
    fn the_evidence_is_a_fixed_size_header_whatever_the_block_it_names() {
        let me = identity(0x11);
        let plain = signed_block(&me, 3, 0x01);

        // The same header, whose block carries a deploy of a megabyte. A whole-block payload would
        // grow by that megabyte; the header cannot, because no field of it holds the block.
        let mut fat = plain.clone();
        fat.state.deploys = vec![ProcessedDeploy {
            deploy: SignedDeployData {
                deployer: vec![1u8; 65],
                data: DeployData {
                    term: "x".repeat(1 << 20),
                    timestamp: 0,
                    phlo_price: 0,
                    phlo_limit: 0,
                    valid_after_block_number: 0,
                    shard_id: "root".to_string(),
                    attachments: vec![],
                },
                sig_algorithm: "secp256k1".to_string(),
                sig: vec![],
            },
            cost: PCost { cost: 0 },
            deploy_log: vec![],
            is_failed: false,
            system_deploy_error: None,
        }];
        let fat_header = EquivocationEvidence::from_block(&fat).encode();
        let plain_header = EquivocationEvidence::from_block(&plain).encode();

        assert_eq!(
            fat_header.len(),
            plain_header.len(),
            "the evidence's size must not depend on the block it names"
        );
        assert!(
            plain_header.len() < 1024,
            "a header is a few hundred bytes, not a block: got {}",
            plain_header.len()
        );
        assert!(
            EquivocationEvidence::decode(&fat_header).is_ok(),
            "and it round-trips"
        );
    }
    /// The stalled-round escape must produce a block accepted by the *actual*
    /// justification regression validator, not just the expected parent IDs.
    /// The same DAG with the stale peer parent must be rejected with the named
    /// BlockStatus. This is the before/after validation control for PR #492.
    #[tokio::test]
    async fn escaped_child_passes_real_justification_validation() {
        use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};

        let a = Validator::new([0u8; 65]);
        let b = Validator::new([1u8; 65]);
        let bonds = BTreeMap::from([
            (a, NonNegI64::try_from(250).unwrap()),
            (b, NonNegI64::try_from(250).unwrap()),
            (Validator::new([2u8; 65]), NonNegI64::try_from(250).unwrap()),
            (Validator::new([3u8; 65]), NonNegI64::try_from(250).unwrap()),
        ]);
        let mut state: DagMessageState<BlockHash, Validator> = DagMessageState::empty();
        let a1 = state.create_message(
            hash(10),
            BlockHeight::try_from(1).unwrap(),
            a,
            SeqNum::try_from(1).unwrap(),
            bonds.clone(),
            &BTreeSet::new(),
        );
        let b1 = state.create_message(
            hash(20),
            BlockHeight::try_from(1).unwrap(),
            b,
            SeqNum::try_from(1).unwrap(),
            bonds.clone(),
            &BTreeSet::new(),
        );
        state.insert_msg_mut(&a1);
        state.insert_msg_mut(&b1);
        let b2 = state.create_message(
            hash(21),
            BlockHeight::try_from(2).unwrap(),
            b,
            SeqNum::try_from(2).unwrap(),
            bonds.clone(),
            &[b1.clone()].into_iter().collect(),
        );
        state.insert_msg_mut(&b2);
        let a2 = state.create_message(
            hash(11),
            BlockHeight::try_from(3).unwrap(),
            a,
            SeqNum::try_from(2).unwrap(),
            bonds.clone(),
            &[a1.clone(), b2.clone()].into_iter().collect(),
        );
        state.insert_msg_mut(&a2);

        // The round snapshot predates B2, which A2 already justified.
        state.round_parents = [(a, a1), (b, b1.clone())].into_iter().collect();
        let repaired = state.parents_for_new_block_escaping_checked(&a).unwrap();
        let actual_child = state.create_message(
            hash(12),
            BlockHeight::try_from(4).unwrap(),
            a,
            SeqNum::try_from(3).unwrap(),
            bonds,
            &repaired,
        );
        let repaired_ids = actual_child.parents.iter().copied().collect();
        let stale_ids = vec![a2.id, b1.id];

        let mut dag = mock(BTreeMap::new());
        dag.representation.dag_message_state = state;
        let accepted = block(0, 4, 3, repaired_ids);
        let refused = block(0, 4, 3, stale_ids);

        assert_eq!(
            justification_regressions(&dag, &accepted).await.unwrap(),
            Ok(()),
            "the actual block validator must admit the escaped child's justifications"
        );
        assert_eq!(
            justification_regressions(&dag, &refused).await.unwrap(),
            Err(BlockStatus::JustificationRegression),
            "the original stale round parent must be rejected by the real validator"
        );
        // The proposer-produced child carries the same selected justifications;
        // its other validation stages (execution, signatures and finality) are
        // deliberately outside this focused rejection-path regression test.
    }

    /// **An unresolvable justification is named, and the verdict is unchanged** (Law 88d). The
    /// regression check cannot evaluate a justification absent from the message map; the current
    /// rule passes (Scala fidelity), so the block reaches the height check, where it fails naming
    /// the hash. Turning this into a refusal would be a consensus change and fails this test.
    #[tokio::test]
    async fn an_unknown_justification_passes_the_regression_check_and_is_named() {
        let dag = mock(BTreeMap::new());
        let b = block(9, 1, 1, vec![hash(7)]);

        assert_eq!(
            check_justification_regression(&dag, &b).await.unwrap(),
            JustificationOutcome::Unresolved(vec![hash(7)]),
            "the unresolved shape carries the hash the node could not resolve"
        );
        assert_eq!(
            justification_regressions(&dag, &b).await.unwrap(),
            Ok(()),
            "the verdict is unchanged: the rule passes, exactly as the Scala's `None` did"
        );
    }
}
