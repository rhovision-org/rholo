//! The PoS read surface the node did not have (AUDIT C148).
//!
//! `docs/src/node/operating.md` explains the epoch rule ("withdraw … only stages a deadline
//! (`pendingWithdrawers`) … the boundary is `blockNumber % epochLength == 0`") and then leaves the
//! operator to derive the boundary from block heights: the epoch counter, the active validator set
//! and `pendingWithdrawers` live in native state and were projected by no route and no subcommand.
//! `bond-status` answers a bool; `GET /api/v1/shards` answers heights. So an epoch or unbonding
//! question got answered by inference, or by hand-deploying rholang from a console on a live
//! validator — exactly the poking the REPL's isolated eval store exists to avoid for terms.
//!
//! **A trait of its own rather than a method on `BlockApi`.** The read needs the shard's
//! `RuntimeManager` and the status API, neither of which belongs on the block API — and adding a
//! method to `BlockApi` would have meant touching every mock of it in the tree, which is a large
//! diff for a read that no block-path caller wants.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use rchain_casper::runtime_manager::RuntimeManager;
use rchain_models::validator::Validator;
use rchain_rholang::native_state::NativeSystemState;
use rchain_rspace::native_store::InMemNativeStore;

use crate::api::web_api::WebApi;

/// A withdrawal staged and waiting for its boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingWithdrawal {
    pub validator: Validator,
    /// **The block the boundary may pay it at** — `pendingWithdrawers`' value, which is law 47's
    /// *deadline* (`quarantine_length + divisor·(1 + block_number/divisor)`), not the height the
    /// request was made at. The two differ by the quarantine and more, and the earlier version of this
    /// struct called it `stagedAtBlock` and carried a number an operator could not use (AUDIT C206).
    pub deadline: i64,
    /// Blocks to that deadline: `deadline − latest`, floored at zero. **It is not `deadline +
    /// quarantine − latest`** — the stored value already contains the quarantine, so adding it again
    /// reported a withdrawal as 50 000 blocks further away than it was (AUDIT C206).
    pub blocks_remaining: i64,
}

/// **One operator a delegator has staked with** (law 57, #193).
///
/// The three numbers come from three leaves — the principal from `pos:delegations`, the accrued
/// reward from `pos:delegated_rewards`, the staged exit from `pos:pending_delegations` — and they are
/// reported together because a delegator's question is "what is my position", not "what does leaf X
/// say".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatorPosition {
    /// The operator's public key, as lowercase hex (`Validator`'s serialization).
    pub operator: Validator,
    /// The principal delegated to this operator and still attributed to the delegator.
    pub amount: i64,
    /// Reward accrued to this delegation and not yet paid. Held apart from the operator's committed
    /// rewards, which is what makes it the delegator's to see.
    pub accrued_rewards: i64,
    /// The staged exit, if one is in flight; `None` when nothing is staged.
    pub pending_undelegation: Option<PendingUndelegation>,
}

/// An undelegation that has been requested and not yet acted on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingUndelegation {
    /// **The block the boundary may act on it at** — `pending_delegations`' value, which is law 47's
    /// *deadline* (`quarantine_length + divisor·(1 + block_number/divisor)`) and already contains the
    /// quarantine. **Named `deadline` rather than `stagedAtBlock` for the reason AUDIT C206 landed**:
    /// the stored value is not the height the request was made at, and calling it one is the mistake
    /// that made the withdrawal countdown wrong by the whole quarantine.
    pub deadline: i64,
    /// Blocks to that deadline, floored at zero. The **same** `blocks_remaining` the withdrawal
    /// countdown uses — one definition of the countdown, not two.
    pub blocks_remaining: i64,
}

/// What the node can say about its own PoS state, in one read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PosStatus {
    pub latest_block_number: i64,
    /// `epochLength` from the shard's PoS parameters (`<= 1` means every block is a boundary).
    pub epoch_length: i64,
    /// `quarantineLength` — the distance between a staged withdrawal and its payout.
    pub quarantine_length: i64,
    /// The current epoch: the number of boundaries behind the head. `0` while `epochLength <= 1`,
    /// because then every block *is* a boundary and counting them would name nothing.
    pub epoch: i64,
    /// Blocks to the next boundary: `0` exactly when the head is one.
    pub blocks_until_epoch_boundary: i64,
    pub active_validators: Vec<Validator>,
    pub pending_withdrawals: Vec<PendingWithdrawal>,
}

/// The PoS read, as the HTTP layer sees it.
#[async_trait]
pub trait PosReadApi: Send + Sync {
    async fn pos_status(&self) -> Result<PosStatus, String>;
    /// **One delegator's positions, across every operator it has staked with** (#193).
    ///
    /// Scoped to the caller-supplied key, and that is a deliberate choice rather than a shape: the
    /// `pos:delegations` ledger is unbounded in the number of *delegators* per operator, so the read
    /// that is bounded is the one keyed by the delegator. The operator-scoped listing — every
    /// delegator of one key — is the unbounded direction and is not offered.
    ///
    /// It is a separate method rather than a field on [`PosStatus`] for the same reason: that read is
    /// a single object about a shard, and hanging an unbounded ledger off it would make one status
    /// call's size a function of how many people have ever delegated.
    async fn delegator_positions(
        &self,
        delegator: &Validator,
    ) -> Result<Vec<DelegatorPosition>, String>;
}

/// Reads the primary shard's committed native state and the status API's head height.
pub struct ShardPosRead {
    runtime: Arc<RuntimeManager>,
    web_api: Arc<dyn WebApi>,
}

impl ShardPosRead {
    pub fn new(runtime: Arc<RuntimeManager>, web_api: Arc<dyn WebApi>) -> Self {
        ShardPosRead { runtime, web_api }
    }

    async fn committed_native_state(&self) -> Result<NativeSystemState, String> {
        // Capture one published root for the whole response. The play store belongs to execution
        // and can hold an empty or unfinished state while the node has a committed root.
        let root = self.runtime.current_root().await?;
        let reader = self
            .runtime
            .get_history_repo()
            .get_native_reader(root)
            .await;
        let store = Arc::new(InMemNativeStore::new(reader));
        Ok(NativeSystemState::new(store))
    }
}

#[async_trait]
impl PosReadApi for ShardPosRead {
    async fn pos_status(&self) -> Result<PosStatus, String> {
        let native = self.committed_native_state().await?;
        let params = native
            .params_opt()
            .await?
            .ok_or_else(|| "PoS params leaf not found in committed state".to_string())?;
        let active = native.active_validators().await?;
        let pending = native.pending_withdrawers().await?;
        // The head's height, from the same read `/api/status` answers with — one definition of
        // "latest block" in the node rather than a second one here.
        let latest = self
            .web_api
            .status()
            .await
            .map_err(|e| e.to_string())?
            .latest_block_number;

        Ok(PosStatus {
            latest_block_number: latest,
            epoch_length: params.epoch_length,
            quarantine_length: params.quarantine_length,
            epoch: epoch_of(latest, params.epoch_length),
            blocks_until_epoch_boundary: blocks_until_boundary(latest, params.epoch_length),
            active_validators: active.into_iter().collect(),
            pending_withdrawals: pending
                .into_iter()
                .map(|(validator, deadline)| PendingWithdrawal {
                    validator,
                    deadline,
                    blocks_remaining: blocks_remaining(latest, deadline),
                })
                .collect(),
        })
    }

    async fn delegator_positions(
        &self,
        delegator: &Validator,
    ) -> Result<Vec<DelegatorPosition>, String> {
        let native = self.committed_native_state().await?;
        let latest = self
            .web_api
            .status()
            .await
            .map_err(|e| e.to_string())?
            .latest_block_number;
        // **Accessors only — a read path never writes.** Each absent leaf reads as an empty map; a
        // `set_*` here would put a trie leaf under a chain that has never delegated and move its root
        // (law 57's dormancy requirement).
        let ledger = native.delegations().await?;
        let pending = native.pending_delegations().await?;
        let rewards = native.delegated_rewards().await?;
        Ok(ledger
            .iter()
            .filter(|(key, _)| key.delegator == *delegator)
            .map(|(key, amount)| DelegatorPosition {
                operator: key.operator,
                amount: i64::from(*amount),
                accrued_rewards: rewards.get(key).map_or(0, |r| i64::from(*r)),
                pending_undelegation: pending.get(key).map(|deadline| PendingUndelegation {
                    deadline: *deadline,
                    blocks_remaining: blocks_remaining(latest, *deadline),
                }),
            })
            .collect())
    }
}

/// How many boundaries have passed at `block_number`.
///
/// `epochLength <= 1` is the "every block is a boundary" setting (`PosParams`'s own words), where
/// counting boundaries would name a number no rule reads — so it answers `0` rather than dividing.
fn epoch_of(block_number: i64, epoch_length: i64) -> i64 {
    if epoch_length <= 1 {
        0
    } else {
        block_number.div_euclid(epoch_length)
    }
}

/// Blocks until the next boundary; `0` at a boundary, and `0` when every block is one.
fn blocks_until_boundary(block_number: i64, epoch_length: i64) -> i64 {
    if epoch_length <= 1 {
        0
    } else {
        epoch_length - block_number.rem_euclid(epoch_length)
    }
}

/// Blocks left before a withdrawal whose deadline is `deadline` is paid out, floored at zero.
///
/// `deadline` is `pendingWithdrawers`' stored value, which **already contains the quarantine** (law
/// 47: it is `quarantine_length + divisor·(1 + block/divisor)`). The earlier version took the value as
/// a staging height and added `quarantine_length` to it a second time, reporting a withdrawal as
/// 50 000 blocks further away than it was (AUDIT C206).
fn blocks_remaining(latest: i64, deadline: i64) -> i64 {
    deadline.saturating_sub(latest).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arithmetic the operator is currently expected to do by hand. Both directions and the
    /// degenerate setting, because `epochLength` of `1` (or `0`) is the shape a modulo by it would
    /// divide by — and "every block is a boundary" is a real genesis setting, not a corner case.
    #[test]
    fn the_boundary_arithmetic_answers_the_operators_question() {
        // The default devnet epoch length of the genesis PoS parameters.
        assert_eq!(epoch_of(250, 100), 2);
        assert_eq!(blocks_until_boundary(250, 100), 50);
        // On a boundary: the next one is a whole epoch away, not zero away.
        assert_eq!(epoch_of(300, 100), 3);
        assert_eq!(blocks_until_boundary(300, 100), 100);
        assert_eq!(blocks_until_boundary(0, 100), 100);

        // `epochLength <= 1`: every block is a boundary, so nothing is counted and nothing is
        // waited for — and nothing divides by zero.
        assert_eq!(epoch_of(42, 1), 0);
        assert_eq!(blocks_until_boundary(42, 1), 0);
        assert_eq!(epoch_of(42, 0), 0);
        assert_eq!(blocks_until_boundary(42, 0), 0);
    }

    /// **AUDIT C206's falsifier.** A staged withdrawal counts down to the deadline the *store* holds,
    /// and the values are **derived from the rule** rather than chosen — `withdraw` writes
    /// `quarantine_length + divisor·(1 + block/divisor)` (`rholang/src/native_state.rs`), so a request
    /// at block 250 of an epoch of length 100 under a quarantine of 50 holds **350**.
    ///
    /// The earlier version of this test passed `(latest, staged, quarantine)` and asserted against a
    /// *staging height* the store never contains, so it agreed with an implementation that added the
    /// quarantine a second time and reported every pending withdrawal 50 000 blocks too far away.
    #[test]
    fn a_pending_withdrawal_counts_down_to_the_deadline_the_store_holds() {
        let deadline = 50 + 100 * (1 + 250 / 100);
        assert_eq!(
            deadline, 350,
            "the deadline `withdraw` writes for a request at block 250"
        );
        assert_eq!(blocks_remaining(250, deadline), 100);
        assert_eq!(blocks_remaining(300, deadline), 50);
        assert_eq!(blocks_remaining(350, deadline), 0);
        // Past due is zero rather than a negative countdown: a withdrawal that has not been paid out
        // yet is not an operator error to be reported as a negative number of blocks.
        assert_eq!(blocks_remaining(400, deadline), 0);
        // The degenerate quarantine: a deadline of the current height is due now.
        assert_eq!(blocks_remaining(10, 10), 0);

        // **The mutation the defect was, stated so it cannot come back**: adding the quarantine to a
        // value that already contains it reads 150 here where the answer is 100.
        assert_ne!(
            blocks_remaining(250, deadline),
            deadline + 50 - 250,
            "the stored value already contains the quarantine; adding it again is AUDIT C206"
        );
    }

    /// **A staged undelegation is read with the same arithmetic** (#193). `undelegate` writes its
    /// deadline with the identical rule `withdraw` uses — the same `quarantine_length +
    /// divisor·(1 + block/divisor)` — so the two countdowns must not be two definitions. This asserts
    /// they agree rather than restating the formula a third time.
    #[test]
    fn a_staged_undelegation_counts_down_with_the_same_arithmetic() {
        let (latest, epoch, quarantine) = (250, 100, 50);
        let withdrawal = quarantine + epoch * (1 + latest / epoch);
        let undelegation = quarantine + epoch * (1 + latest / epoch);
        assert_eq!(withdrawal, undelegation, "one rule, two callers");
        assert_eq!(blocks_remaining(latest, undelegation), 100);
        assert_eq!(blocks_remaining(350, undelegation), 0);
        // Floored, not negative: a deadline that has passed is not an operator error to report as a
        // negative number of blocks.
        assert_eq!(blocks_remaining(400, undelegation), 0);
    }
}
