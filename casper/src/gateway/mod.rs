//! The multi-shard gateway: the on-node two-phase-commit coordinator (Laws 26–29, "Layer 2").
//!
//! The node-side realization of `docs/src/node/shard-invoke.md`'s *"a gateway peer that is a member
//! of both shards"*: a node that validates several shards can drive a cross-shard transaction
//! **itself**, submitting each leg as an ordinary signed deploy to the shard that owns it, instead
//! of relying on an off-chain client to reach both shards over the network.
//!
//! What is and is not consensus-critical:
//!
//! * the **legs** are ordinary signed deploys through the real pipeline — pooled, included in a
//!   block, executed by the `rho:txn` participant. They are never evaluated directly on the play
//!   runtime: that would mutate state outside a block and be discarded by the next proposal's
//!   merged pre-state;
//! * the **decision** (and its durable record, [`ledger`]) is coordinator-side. Law 27's
//!   all-or-nothing is supplied here — no shard's reducer does network I/O, and no other validator
//!   runs a coordinator;
//! * the **routing** — which shard a leg goes to — is this node's business, not part of any shard's
//!   consensus.

pub mod ledger;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_models::casper::protocol::casper_message::SignedDeployData;
use rchain_models::casper::protocol::deploy_service::{
    BlockQuery, BlocksQuery, BondStatusQuery, ContinuationAtNameQuery, ContinuationsWithBlockInfo,
    DataAtNameQuery, DataWithBlockInfo, DeployExecStatus, FindDeployQuery, IsFinalizedQuery,
    MachineVerifyQuery, VisualizeDagQuery,
};
use rchain_models::rholang::RhoType::{RhoByteArray, RhoDeployId, RhoNumber, RhoString};
use rchain_shared::base16;
use rchain_shared::refined::{NonNegI64, ShardId};

use crate::api::block_api::BlockApi;
use crate::protocol::client::DeployService;
use crate::shard_invoke::{await_reply, signed_invoke, ShardOutcome};
use crate::txn_coordinator::txn_term;

pub use ledger::{CoordRecord, CoordState, LegRecord, TxnLedger, Vote};

/// One of the node's own shards, as a 2PC participant.
pub struct LocalShard {
    pub shard_id: ShardId,
    /// The shard's block API: used to pool a phase deploy (a real deploy, included in a block by the
    /// shard's own proposer) and to read the reply back.
    pub block_api: Arc<dyn BlockApi>,
    /// The shard's configured reply-listen limit (`api-server.max-blocks-limit`).
    pub max_listen_depth: i32,
}

/// A [`DeployService`] over the node's **own** shards, so a leg costs a local pool insert instead of
/// a network round trip.
///
/// Shaped after the `InProcDeployService` test double, with the two differences production needs: it
/// goes through the shard's `BlockApi` (not a direct runtime evaluation, which would write state
/// outside a block), and it clamps the reply-listen depth — `await_reply` asks for the maximum
/// depth, which the block API rejects outright, so an unclamped listen would never see the reply.
pub struct LocalShardDeployService {
    shards: BTreeMap<ShardId, LocalShard>,
    /// Which shard each submitted deploy went to, so a reply is read back from the same shard.
    deployed: Mutex<BTreeMap<Vec<u8>, ShardId>>,
}

impl LocalShardDeployService {
    pub fn new(shards: BTreeMap<ShardId, LocalShard>) -> Self {
        LocalShardDeployService {
            shards,
            deployed: Mutex::new(BTreeMap::new()),
        }
    }

    /// The member with this id, or the membership error.
    pub fn member(&self, shard_id: &str) -> Result<&LocalShard, Vec<String>> {
        self.shards
            .iter()
            .find(|(id, _)| id.to_string() == shard_id)
            .map(|(_, shard)| shard)
            .ok_or_else(|| {
                vec![format!(
                    "Deploy shardId '{shard_id}' is not a member of this node's shards: [{}]",
                    self.shards
                        .keys()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )]
            })
    }

    /// The shard a previously-submitted deploy went to.
    fn shard_of(&self, sig: &[u8]) -> Option<&LocalShard> {
        let recorded = self.deployed.lock().ok()?.get(sig).cloned()?;
        self.shards.get(&recorded)
    }

    /// The shard's current height, which a phase deploy is anchored at: with a hardcoded
    /// `valid_after = 0` the deploy is *born expired* once the chain reaches `DEPLOY_LIFESPAN`
    /// blocks past genesis, and the participant never sees it.
    ///
    /// **A head that cannot be read is an `Err`, not `0`** (AUDIT C67). `0` anchors the deploy at
    /// genesis, so a failed read did not fail the phase — it submitted a deploy that expires on
    /// arrival, invisibly: nothing downstream reports an error, the participant simply never sees the
    /// leg. The gateway is Rust-first (`docs/src/node/shard-invoke.md`), so there is no Scala `F` to
    /// conform to; its own submit path is the oracle, and that path already refuses a phase it cannot
    /// build (`signed_invoke`'s `Err` → `ShardOutcome::Error` in [`Self::phase_with_term`]), which is
    /// the same answer this read gets now.
    ///
    /// An **unknown** shard is still `Ok(0)`: that is a routing fact rather than a read failure, and
    /// refusing a phase for it would be a different decision.
    async fn current_height(&self, shard_id: &ShardId) -> Result<i64, String> {
        match self.shards.get(shard_id) {
            Some(shard) => match shard.block_api.get_latest_message().await {
                Ok(metadata) => Ok(i64::from(metadata.block_num)),
                Err(e) => Err(format!(
                    "cannot anchor a phase deploy: the head of shard {shard_id} could not be read: {e}"
                )),
            },
            None => Ok(0),
        }
    }
}

#[async_trait]
impl DeployService for LocalShardDeployService {
    async fn deploy(&self, d: &SignedDeployData) -> Result<String, Vec<String>> {
        let shard = self.member(&d.data.shard_id)?;
        let sig_hex = base16::encode(&d.sig);
        shard.block_api.deploy(d).await.map_err(|e| vec![e])?;
        if let Ok(mut deployed) = self.deployed.lock() {
            deployed.insert(d.sig.clone(), shard.shard_id.clone());
        }
        Ok(sig_hex)
    }

    async fn listen_for_data_at_name(
        &self,
        q: &DataAtNameQuery,
    ) -> Result<Vec<DataWithBlockInfo>, Vec<String>> {
        let sig = RhoDeployId::unapply(&q.name)
            .ok_or_else(|| vec!["name is not a deploy id".to_string()])?;
        let shard = self
            .shard_of(sig)
            .ok_or_else(|| vec!["no shard is known for this deploy id".to_string()])?;
        let depth = q.depth.min(shard.max_listen_depth);
        // The shard's own reply data, with its own block info — no re-wrapping.
        let (data, _depth) = shard
            .block_api
            .get_listening_name_data_response(depth, &q.name)
            .await
            .map_err(|e| vec![e])?;
        Ok(data)
    }

    // Driving a 2PC only submits phase deploys and reads their replies. The remaining queries are
    // reported as unsupported rather than answered wrongly.
    async fn deploy_status(&self, _: &FindDeployQuery) -> Result<DeployExecStatus, Vec<String>> {
        Err(vec!["deploy_status is not used by the gateway".to_string()])
    }
    async fn get_block(&self, _: &BlockQuery) -> Result<String, Vec<String>> {
        Err(vec!["get_block is not used by the gateway".to_string()])
    }
    async fn get_blocks(&self, _: &BlocksQuery) -> Result<String, Vec<String>> {
        Err(vec!["get_blocks is not used by the gateway".to_string()])
    }
    async fn visualize_dag(&self, _: &VisualizeDagQuery) -> Result<String, Vec<String>> {
        Err(vec!["visualize_dag is not used by the gateway".to_string()])
    }
    async fn machine_verifiable_dag(&self, _: &MachineVerifyQuery) -> Result<String, Vec<String>> {
        Err(vec![
            "machine_verifiable_dag is not used by the gateway".to_string()
        ])
    }
    async fn find_deploy(&self, _: &FindDeployQuery) -> Result<String, Vec<String>> {
        Err(vec!["find_deploy is not used by the gateway".to_string()])
    }
    async fn listen_for_continuation_at_name(
        &self,
        _: &ContinuationAtNameQuery,
    ) -> Result<Vec<ContinuationsWithBlockInfo>, Vec<String>> {
        Err(vec![
            "listen_for_continuation_at_name is not used by the gateway".to_string(),
        ])
    }
    async fn last_finalized_block(&self) -> Result<String, Vec<String>> {
        Err(vec![
            "last_finalized_block is not used by the gateway".to_string()
        ])
    }
    async fn is_finalized(&self, _: &IsFinalizedQuery) -> Result<String, Vec<String>> {
        Err(vec!["is_finalized is not used by the gateway".to_string()])
    }
    async fn bond_status(&self, _: &BondStatusQuery) -> Result<String, Vec<String>> {
        Err(vec!["bond_status is not used by the gateway".to_string()])
    }
    async fn status(&self) -> Result<String, Vec<String>> {
        Err(vec!["status is not used by the gateway".to_string()])
    }
}

/// One leg of a transaction, as the caller asks for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayLeg {
    pub shard_id: ShardId,
    pub amount: i64,
    pub to: String,
}

/// The on-node 2PC coordinator: opens a durable record for a transaction and drives it to a terminal
/// state, resuming an interrupted one on restart (Law 29).
pub struct GatewayTxn {
    local: Arc<LocalShardDeployService>,
    ledger: Arc<TxnLedger>,
    /// The signing key — the node's validator key. The participant gates `commit`/`abort` on
    /// `deployerId == the coordinator recorded at prepare`, so the same key must run both phases.
    key: PrivateKey,
    coordinator: PublicKey,
    /// Per-phase wait for a deploy to be included in a block.
    phase_timeout: Duration,
    /// One lock per `txn_id`, so two concurrent callers for the same transaction cannot both decide the
    /// ledger holds no record for it (AUDIT C125).
    ///
    /// **Why a lock and not a compare-and-set.** The ledger is a plain `get`/`put` over the node's own
    /// store, and the coordinator is one process — the register's own disposition for this finding is
    /// that an in-process keyed mutex is sufficient, and it covers the *drive* as well as the open,
    /// which a store-level CAS would not.
    ///
    /// **The entries are removed when the last holder releases one**, which is the difference from the
    /// `MultiLock` whose never-cleaned map the register records: an operator's node sees a distinct
    /// `txn_id` per transaction, and a lock kept per transaction forever is a map nothing prunes.
    inflight: Arc<tokio::sync::Mutex<BTreeMap<Vec<u8>, Arc<tokio::sync::Mutex<()>>>>>,
}

impl GatewayTxn {
    pub fn new(
        local: Arc<LocalShardDeployService>,
        ledger: Arc<TxnLedger>,
        key: PrivateKey,
        coordinator: PublicKey,
        phase_timeout: Duration,
    ) -> Self {
        GatewayTxn {
            local,
            ledger,
            key,
            coordinator,
            phase_timeout,
            inflight: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
        }
    }

    /// The durable record for a transaction, if the node has one.
    pub async fn status(&self, txn_id: &[u8]) -> Result<Option<CoordRecord>, String> {
        self.ledger.get(txn_id).await
    }

    /// Every transaction still awaiting a decision.
    pub async fn list(&self) -> Result<Vec<CoordRecord>, String> {
        self.ledger.in_flight().await
    }

    /// Open (or resume) the transaction and drive it to a terminal state.
    ///
    /// Idempotent under `txn_id` (Law 28's coordinator-side analogue): a terminal record is returned
    /// untouched and **no participant is contacted**, so re-issuing a completed transaction cannot
    /// move funds again.
    pub async fn run(&self, txn_id: &[u8], legs: &[GatewayLeg]) -> Result<CoordRecord, String> {
        // **One transaction at a time per `txn_id`** (AUDIT C125). Everything below decides whether to
        // open a transaction and then drives it — a check-then-act with no serialization of its own, so
        // two callers that both read "no record" both open one: each builds a record from *its own* leg
        // list, each drives its own phase one, and the durable record that `recover_in_flight` trusts
        // afterwards can describe a different transaction than the effects applied under that `txn_id`.
        let lock = self.acquire_txn_lock(txn_id).await;
        let held = lock.lock().await;
        let outcome = self.run_under_lock(txn_id, legs).await;
        drop(held);
        self.release_txn_lock(txn_id, &lock).await;
        outcome
    }

    /// `run`'s body, with the per-`txn_id` lock already held.
    async fn run_under_lock(
        &self,
        txn_id: &[u8],
        legs: &[GatewayLeg],
    ) -> Result<CoordRecord, String> {
        if let Some(existing) = self.ledger.get(txn_id).await? {
            if existing.state.is_terminal() {
                return Ok(existing);
            }
            return self.drive(existing).await;
        }
        if legs.is_empty() {
            return Err("a cross-shard transaction needs at least one leg".to_string());
        }
        // Membership is a *caller* error, not a participant failure: a leg on a shard this node does
        // not validate can never prepare, so reject the request up front instead of opening a
        // transaction that is guaranteed to abort. (A leg that fails at the participant — short
        // escrow, timeout — is different: that aborts and compensates, which is Law 27 working.)
        for leg in legs {
            self.local
                .member(&leg.shard_id.to_string())
                .map_err(|err| err.join("; "))?;
        }
        let record = CoordRecord {
            txn_id: txn_id.to_vec(),
            state: CoordState::Proposed,
            coordinator: self.coordinator.clone(),
            legs: legs
                .iter()
                .map(|leg| {
                    Ok(LegRecord {
                        shard_id: leg.shard_id.clone(),
                        amount: NonNegI64::try_from(leg.amount)
                            .map_err(|e| format!("leg amount {}: {e}", leg.amount))?,
                        to: leg.to.clone(),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            votes: Vec::new(),
            reason: None,
        };
        // The record is durable *before* the first leg is sent: a crash after a participant locked
        // its escrow must be recoverable.
        self.ledger.put(&record).await?;
        self.drive(record).await
    }

    /// Every in-flight record, resumed. Called once at startup so a prepared participant's escrow is
    /// not left locked by a node that restarted mid-transaction.
    pub async fn recover_in_flight(&self) -> Result<Vec<CoordRecord>, String> {
        let mut recovered = Vec::new();
        for record in self.ledger.in_flight().await? {
            // The same per-`txn_id` lock a client's `run` takes, because the race is the documented
            // retry and not an exotic one: re-issuing the same `txnId` after a restart is how a client is
            // told to resume, so this task and that retry arrive together by construction — and two
            // drives of one in-flight record each run their own phase one with their own timeout, so one
            // can record `Committed` while the other records `Aborted` (AUDIT C125's second half).
            let txn_id = record.txn_id.clone();
            let lock = self.acquire_txn_lock(&txn_id).await;
            let held = lock.lock().await;
            let outcome = self.drive(record).await;
            drop(held);
            self.release_txn_lock(&txn_id, &lock).await;
            recovered.push(outcome?);
        }
        Ok(recovered)
    }

    /// The per-`txn_id` lock, created on first use. The map is held *while* the entry is looked up: a
    /// get-then-insert outside the guard is the race the register records for `MultiLock`.
    async fn acquire_txn_lock(&self, txn_id: &[u8]) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.inflight.lock().await;
        map.entry(txn_id.to_vec())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Drop the entry for `txn_id` when this caller is its last holder, so an operator's node does not
    /// keep a lock per transaction it has ever seen. `strong_count == 1` means nobody else can be holding
    /// or waiting on it — a caller that has the handle holds a clone — and the check runs under the map's
    /// own lock, so a caller cannot be about to acquire the handle while the entry is removed.
    async fn release_txn_lock(&self, txn_id: &[u8], lock: &Arc<tokio::sync::Mutex<()>>) {
        let mut map = self.inflight.lock().await;
        if Arc::strong_count(lock) == 1 {
            map.remove(txn_id);
        }
    }

    /// Drive a record to a terminal state: prepare the legs that have no vote, write the decision,
    /// then apply phase two.
    async fn drive(&self, mut record: CoordRecord) -> Result<CoordRecord, String> {
        // Phase one: prepare every leg that has not voted yet. A leg that already voted is not
        // re-prepared — that is what makes a resume safe.
        if !record.state.is_terminal() {
            for leg in record.legs.clone() {
                if record.vote_for(&leg.shard_id).is_some() {
                    continue;
                }
                let vote = self.prepare_leg(&mut record, &leg).await;
                // Durable as each vote arrives, so a crash mid-phase-one loses nothing.
                self.ledger.put(&record).await?;
                if vote == Vote::Abort {
                    break;
                }
            }
        }

        // Decide. `record_vote` derives the state from the votes: `Committed` once every leg has
        // voted ready, `Aborted` on any abort. A record that is still undecided here has no legs at
        // all (a well-formed transaction always has at least one) — abort it rather than leaving it
        // in flight forever.
        if !record.state.is_terminal() {
            record.state = CoordState::Aborted;
            record.reason = Some("transaction has no legs".to_string());
        }
        // The commit point: the decision is durable *before* any phase-two message, so a crash
        // between the two is finished by `recover_in_flight`.
        self.ledger.put(&record).await?;

        self.apply_phase_two(&record).await;
        // Idempotent rewrite: the phase-two replies are not part of the decision.
        self.ledger.put(&record).await?;
        Ok(record)
    }

    /// Send phase two to every leg that voted ready — the only legs that locked an escrow.
    async fn apply_phase_two(&self, record: &CoordRecord) {
        let method = match record.state {
            CoordState::Committed => "commit",
            CoordState::Aborted => "abort",
            // Undecided records never reach phase two.
            CoordState::Proposed | CoordState::Prepared => return,
        };
        for leg in &record.legs {
            if record.vote_for(&leg.shard_id) != Some(Vote::Ready) {
                continue;
            }
            let _ = self.phase(record, leg, method).await;
        }
    }

    /// Prepare one leg: submit the phase-one deploy and record the participant's vote.
    async fn prepare_leg(&self, record: &mut CoordRecord, leg: &LegRecord) -> Vote {
        let args = vec![
            RhoByteArray::apply(self.coordinator.bytes().to_vec()),
            RhoNumber::apply(i64::from(leg.amount)),
            RhoString::apply(leg.to.clone()),
        ];
        // The destination is a caller-supplied string and this node's validator key signs the phase
        // (AUDIT C220), so a value that cannot be written as a literal votes **abort with the reason**
        // rather than being built into the term as code.
        let term = match txn_term("prepare", &record.txn_id, &args, true) {
            Ok(term) => term,
            Err(reason) => {
                record.record_vote(leg.shard_id.clone(), Vote::Abort, Some(reason));
                return Vote::Abort;
            }
        };
        let outcome = self.phase_with_term(leg, &term).await;
        let vote = vote_of(&outcome);
        let reason = match (&outcome, vote) {
            (ShardOutcome::Error(err), _) => Some(format!("{}: {err}", leg.shard_id)),
            (_, Vote::Abort) => Some(format!("{} voted abort", leg.shard_id)),
            (_, Vote::Ready) => None,
        };
        record.record_vote(leg.shard_id.clone(), vote, reason);
        vote
    }

    /// Submit one unbundled phase deploy to a leg's shard and await its reply.
    async fn phase(&self, record: &CoordRecord, leg: &LegRecord, method: &str) -> ShardOutcome {
        let term = match txn_term(method, &record.txn_id, &[], true) {
            Ok(term) => term,
            Err(reason) => return ShardOutcome::Error(reason),
        };
        self.phase_with_term(leg, &term).await
    }

    async fn phase_with_term(&self, leg: &LegRecord, term: &str) -> ShardOutcome {
        let shard_id = leg.shard_id.to_string();
        let valid_after = match self.local.current_height(&leg.shard_id).await {
            Ok(height) => height,
            // Same shape as the signed-invoke failure below: a phase we cannot anchor is a phase we
            // refuse, rather than one submitted to expire (AUDIT C64).
            Err(err) => return ShardOutcome::Error(err),
        };
        // `Signed<DeployData>` borrows a `&dyn SignaturesAlg`, which is not `Sync`, so it must not be
        // held across an `.await` — the future would stop being `Send` and could not be spawned.
        let (deploy, sig) = {
            let signed =
                match signed_invoke(term, &self.key, 0, 1_000_000, 1, valid_after, &shard_id) {
                    Ok(signed) => signed,
                    Err(err) => return ShardOutcome::Error(err),
                };
            let deploy = SignedDeployData {
                data: signed.data,
                deployer: signed.pk.bytes().to_vec(),
                sig: signed.sig.clone(),
                sig_algorithm: "secp256k1".to_string(),
            };
            (deploy, signed.sig.clone())
        };
        if let Err(err) = self.local.deploy(&deploy).await {
            return ShardOutcome::Error(err.join("; "));
        }
        await_reply(
            &*self.local,
            &sig,
            Duration::from_millis(250),
            self.phase_timeout,
        )
        .await
    }
}

/// Interpret a participant's phase-one reply as a vote — the same reading the client-side
/// coordinator uses ([`crate::txn_coordinator::vote_from_reply`]), so the two cannot drift on what
/// counts as ready.
fn vote_of(outcome: &ShardOutcome) -> Vote {
    if crate::txn_coordinator::vote_from_reply(outcome) {
        Vote::Ready
    } else {
        Vote::Abort
    }
}

#[cfg(test)]
mod tests {
    use rchain_models::block_metadata::SlashSeverity;
    use std::sync::Mutex;

    use crate::runtime_manager::CapturedReply;

    use super::*;
    use crate::api::block_api::{ApiErr, Capabilities, FinalityHealth, ShardHealthEntry};
    use crate::construct_deploy;
    use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
    use rchain_models::ast::Par;
    use rchain_models::block_metadata::BlockMetadata;
    use rchain_models::casper::protocol::deploy_service::{
        BlockInfo, ContinuationsWithBlockInfo, DeployExecStatus, LightBlockInfo, Status,
    };
    use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};

    /// A [`BlockApi`] for one shard that records what the gateway asks it to do. The counts are the
    /// point: several gateway branches are only observable by *how many* deploys a phase produced.
    struct CountingShard {
        deployed: Mutex<Vec<String>>,
        listen_depths: Mutex<Vec<i32>>,
        latest_message: Result<i64, String>,
    }

    impl CountingShard {
        fn new(height: Result<i64, String>) -> Self {
            CountingShard {
                deployed: Mutex::new(Vec::new()),
                listen_depths: Mutex::new(Vec::new()),
                latest_message: height,
            }
        }

        fn deploy_count(&self) -> usize {
            self.deployed.lock().unwrap().len()
        }

        fn last_depth(&self) -> Option<i32> {
            self.listen_depths.lock().unwrap().last().copied()
        }
    }

    #[async_trait::async_trait]
    impl BlockApi for CountingShard {
        async fn deploy(&self, deploy: &SignedDeployData) -> ApiErr<String> {
            self.deployed.lock().unwrap().push(deploy.data.term.clone());
            Ok("deployed".to_string())
        }

        async fn get_latest_message(&self) -> ApiErr<BlockMetadata> {
            let height = self.latest_message.clone()?;
            Ok(BlockMetadata {
                block_hash: rchain_models::block_hash::BlockHash::new([0u8; 32]),
                block_num: BlockHeight::try_from(height).map_err(|e| e.to_string())?,
                sender: rchain_models::validator::Validator::from_slice(&[0u8; 65]),
                seq_num: SeqNum::zero(),
                justifications: std::collections::BTreeSet::new(),
                bonds_map: std::collections::BTreeMap::new(),
                validated: true,
                validation_failed: false,
                slashable: false,
                failure_cause: None,
                slash_severity: SlashSeverity::Unspecified,
                restore_attempts: 0,
                member_of_fringe: None,
                fringe: std::collections::BTreeSet::new(),
                fringe_state_hash: Blake2b256Hash::from_bytes([0u8; 32]).into(),
                pre_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
                post_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
                sig: Vec::new(),
                sig_algorithm: String::new(),
            })
        }

        /// Delegates to the floored read: this double has no unfinalised view of its own, and the
        /// method is required rather than defaulted so that is a decision rather than an oversight
        /// (`ShardRoutingBlockApi` is the implementation that had to forward it for real).
        async fn clear_autopropose_halt(
            &self,
            _acknowledge_divergence: Option<&str>,
        ) -> Result<(), String> {
            Ok(())
        }

        /// A stub has no finality cell to read, so it reports **nothing observed** — which this
        /// trait's docs distinguish from "observed healthy". See [`Self::finality_health`].
        async fn finality_health(&self) -> FinalityHealth {
            FinalityHealth::default()
        }

        /// A stub is not a node, so it has no member shards to map.
        async fn health_by_shard(&self) -> BTreeMap<String, ShardHealthEntry> {
            BTreeMap::new()
        }

        async fn get_unfinalised_name_data_response(
            &self,
            depth: i32,
            listening_name: &Par,
        ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)> {
            self.get_listening_name_data_response(depth, listening_name)
                .await
        }

        async fn get_listening_name_data_response(
            &self,
            depth: i32,
            _listening_name: &Par,
        ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)> {
            self.listen_depths.lock().unwrap().push(depth);
            Ok((Vec::new(), depth))
        }

        // Not used by the paths under test.
        async fn status(&self) -> Status {
            unreachable!("not used")
        }
        async fn deploy_status(
            &self,
            _: &rchain_block_storage::dag::dag_storage::DeployId,
        ) -> ApiErr<DeployExecStatus> {
            Err("not used".to_string())
        }
        async fn pooled_deploys(&self) -> ApiErr<Vec<SignedDeployData>> {
            Err("not used".to_string())
        }
        async fn capabilities(&self) -> Capabilities {
            unreachable!("not used")
        }
        async fn create_block(&self, _: bool) -> ApiErr<String> {
            Err("not used".to_string())
        }
        async fn get_propose_result(&self) -> ApiErr<String> {
            Err("not used".to_string())
        }
        async fn get_listening_name_continuation_response(
            &self,
            _: i32,
            _: &[Par],
        ) -> ApiErr<(Vec<ContinuationsWithBlockInfo>, i32)> {
            Err("not used".to_string())
        }
        async fn get_blocks_by_heights(&self, _: i64, _: i64) -> ApiErr<Vec<LightBlockInfo>> {
            Err("not used".to_string())
        }
        async fn visualize_dag(&self, _: i32, _: i32, _: bool) -> ApiErr<Vec<String>> {
            Err("not used".to_string())
        }
        async fn machine_verifiable_dag(&self, _: i32) -> ApiErr<String> {
            Err("not used".to_string())
        }
        async fn get_blocks(&self, _: i32) -> ApiErr<Vec<LightBlockInfo>> {
            Err("not used".to_string())
        }
        async fn find_deploy(
            &self,
            _: &rchain_block_storage::dag::dag_storage::DeployId,
        ) -> ApiErr<LightBlockInfo> {
            Err("not used".to_string())
        }
        async fn get_block(&self, _: &str) -> ApiErr<BlockInfo> {
            Err("not used".to_string())
        }
        async fn bond_status(&self, _: &[u8]) -> ApiErr<bool> {
            Err("not used".to_string())
        }
        async fn exploratory_deploy(
            &self,
            _: &str,
            _: Option<&str>,
            _: bool,
        ) -> ApiErr<(CapturedReply, LightBlockInfo)> {
            Err("not used".to_string())
        }
        async fn get_data_at_par(
            &self,
            _: &Par,
            _: &str,
            _: bool,
        ) -> ApiErr<(Vec<Par>, LightBlockInfo)> {
            Err("not used".to_string())
        }
        async fn last_finalized_block(&self) -> ApiErr<BlockInfo> {
            Err("not used".to_string())
        }
        async fn is_finalized(&self, _: &str) -> ApiErr<bool> {
            Err("not used".to_string())
        }
    }

    fn shard(id: &str) -> ShardId {
        ShardId::try_from(id.to_string()).unwrap()
    }

    /// A record with two legs and no votes.
    fn record() -> CoordRecord {
        CoordRecord {
            txn_id: b"txn".to_vec(),
            state: CoordState::Proposed,
            coordinator: PublicKey::new(vec![7u8; 65]),
            legs: vec![
                LegRecord {
                    shard_id: shard("/root"),
                    amount: NonNegI64::try_from(30).unwrap(),
                    to: "dest".to_string(),
                },
                LegRecord {
                    shard_id: shard("/root/child"),
                    amount: NonNegI64::try_from(40).unwrap(),
                    to: "dest".to_string(),
                },
            ],
            votes: Vec::new(),
            reason: None,
        }
    }

    /// A gateway over two counting shards, with the counting handles handed back.
    async fn gateway_with_counters() -> (
        GatewayTxn,
        Arc<CountingShard>,
        Arc<CountingShard>,
        Arc<LocalShardDeployService>,
    ) {
        let api_a = Arc::new(CountingShard::new(Ok(7)));
        let api_b = Arc::new(CountingShard::new(Ok(3)));
        let mut shards = BTreeMap::new();
        shards.insert(
            shard("/root"),
            LocalShard {
                shard_id: shard("/root"),
                block_api: api_a.clone(),
                max_listen_depth: 50,
            },
        );
        shards.insert(
            shard("/root/child"),
            LocalShard {
                shard_id: shard("/root/child"),
                block_api: api_b.clone(),
                max_listen_depth: 50,
            },
        );
        let local = Arc::new(LocalShardDeployService::new(shards));
        let (key, pub_key) = construct_deploy::default_key_pair().unwrap();
        let manager = rchain_shared::store_manager::InMemoryStoreManager::default();
        let ledger = Arc::new(TxnLedger::open(&manager).await.expect("ledger"));
        let gateway = GatewayTxn::new(
            local.clone(),
            ledger,
            key,
            pub_key,
            Duration::from_millis(50),
        );
        (gateway, api_a, api_b, local)
    }

    /// Phase one's reply mapping: `ready`/`prepared`/`committed` are the three ways a participant
    /// says yes (the last because it is idempotent under `txn_id`), everything else is a no.
    #[test]
    fn vote_of_maps_every_shard_outcome() {
        let value = |s: &str| ShardOutcome::Value(RhoString::apply(s.to_string()));
        for yes in ["ready", "prepared", "committed"] {
            assert_eq!(vote_of(&value(yes)), Vote::Ready, "{yes} must vote ready");
        }
        for no in ["abort", "aborted", "weird", ""] {
            assert_eq!(vote_of(&value(no)), Vote::Abort, "{no} must vote abort");
        }
        // A non-string reply and a transport error are both aborts.
        assert_eq!(
            vote_of(&ShardOutcome::Value(RhoNumber::apply(1))),
            Vote::Abort
        );
        assert_eq!(
            vote_of(&ShardOutcome::Error("timeout".to_string())),
            Vote::Abort
        );
    }

    /// Every `DeployService` query the gateway does not drive reports itself rather than answering
    /// wrongly — an unsupported query must never look like a successful empty reply.
    #[tokio::test]
    async fn local_deploy_service_reports_every_unsupported_query() {
        let (_, _, _, local) = gateway_with_counters().await;
        let par = RhoString::apply("x".to_string());
        let deploy_id = vec![1u8, 2, 3];

        let errors = vec![
            local
                .deploy_status(&FindDeployQuery {
                    deploy_id: deploy_id.clone(),
                })
                .await
                .err(),
            local
                .get_block(&BlockQuery {
                    hash: "h".to_string(),
                })
                .await
                .err(),
            local.get_blocks(&BlocksQuery { depth: 1 }).await.err(),
            local
                .visualize_dag(&VisualizeDagQuery {
                    depth: 1,
                    show_justification_lines: false,
                    start_block_number: 0,
                })
                .await
                .err(),
            local
                .machine_verifiable_dag(&MachineVerifyQuery { depth: 1 })
                .await
                .err(),
            local
                .find_deploy(&FindDeployQuery {
                    deploy_id: deploy_id.clone(),
                })
                .await
                .err(),
            local
                .listen_for_continuation_at_name(&ContinuationAtNameQuery {
                    depth: 1,
                    names: vec![par.clone()],
                })
                .await
                .err(),
            local.last_finalized_block().await.err(),
            local
                .is_finalized(&IsFinalizedQuery {
                    hash: "h".to_string(),
                })
                .await
                .err(),
            local
                .bond_status(&BondStatusQuery {
                    public_key: deploy_id.clone(),
                })
                .await
                .err(),
            local.status().await.err(),
        ];
        for (i, error) in errors.iter().enumerate() {
            let error = error
                .as_ref()
                .unwrap_or_else(|| panic!("query {i} must error"));
            assert!(
                error.join(" ").contains("not used by the gateway"),
                "query {i}: {error:?}"
            );
        }
    }

    /// Phase two must not fire for a record that has not been decided — otherwise a `Proposed`
    /// record would send commits for legs that never locked anything.
    #[tokio::test]
    async fn apply_phase_two_returns_early_for_an_undecided_record() {
        let (gateway, api_a, api_b, _) = gateway_with_counters().await;
        gateway.apply_phase_two(&record()).await; // Proposed
        assert_eq!(api_a.deploy_count(), 0);
        assert_eq!(api_b.deploy_count(), 0);

        let mut prepared = record();
        prepared.record_vote(shard("/root"), Vote::Ready, None);
        assert_eq!(prepared.state, CoordState::Prepared);
        gateway.apply_phase_two(&prepared).await;
        assert_eq!(api_a.deploy_count(), 0, "Prepared is not a decision");
    }

    /// Phase two reaches exactly the legs that voted ready: the others never locked an escrow, so
    /// committing or compensating them would be a no-op at best and a double-spend at worst.
    #[tokio::test]
    async fn apply_phase_two_skips_legs_that_did_not_vote_ready() {
        let (gateway, api_a, api_b, _) = gateway_with_counters().await;

        let mut record = record();
        record.record_vote(shard("/root"), Vote::Ready, None);
        record.record_vote(shard("/root/child"), Vote::Abort, Some("short".to_string()));
        assert_eq!(record.state, CoordState::Aborted);

        gateway.apply_phase_two(&record).await;
        assert_eq!(api_a.deploy_count(), 1, "the locked leg is compensated");
        assert_eq!(api_b.deploy_count(), 0, "the aborting leg never locked");
    }

    /// A phase deploy is anchored at its shard's head: `valid_after = 0` would make it born expired
    /// once the chain passes `DEPLOY_LIFESPAN`.
    #[tokio::test]
    async fn current_height_reads_the_shards_head() {
        let (_, _, _, local) = gateway_with_counters().await;
        assert_eq!(
            local
                .current_height(&shard("/root"))
                .await
                .expect("a readable head"),
            7
        );
        assert_eq!(
            local
                .current_height(&shard("/root/child"))
                .await
                .expect("a readable head"),
            3
        );
    }

    /// An **unknown** shard still falls back to 0 — a routing fact, not a read failure: the shard may
    /// simply be one this node does not validate, and refusing the phase for that is a different
    /// decision from refusing it for an unreadable store.
    #[tokio::test]
    async fn current_height_is_zero_when_the_shard_is_unknown() {
        let api = Arc::new(CountingShard::new(Err("no head".to_string())));
        let mut shards = BTreeMap::new();
        shards.insert(
            shard("/root"),
            LocalShard {
                shard_id: shard("/root"),
                block_api: api,
                max_listen_depth: 50,
            },
        );
        let local = LocalShardDeployService::new(shards);
        assert_eq!(
            local
                .current_height(&shard("/elsewhere"))
                .await
                .expect("an unknown shard has no head to read"),
            0
        );
    }

    /// **A head that cannot be read is not `valid_after = 0`** (AUDIT C64).
    ///
    /// `0` anchors the deploy at genesis, and once the chain reaches `DEPLOY_LIFESPAN` past it
    /// the phase deploy is *born expired*: the participant never sees the leg and nothing reports an
    /// error. The gateway has no Scala oracle (it is this port's own coordinator), but its submit
    /// path is its own oracle here and already refuses a phase it cannot build, one line below the
    /// anchor read.
    ///
    /// Falsifier in its pre-fix (witnessing) form: the failed read *was* answered with `0`, and the
    /// assertion below passed on exactly that (run 2026-09-24 as part of
    /// `current_height_is_zero_when_the_shard_is_unknown_or_errors`, which this test replaces).
    #[tokio::test]
    async fn current_height_refuses_a_head_it_cannot_read() {
        let api = Arc::new(CountingShard::new(Err("no head".to_string())));
        let mut shards = BTreeMap::new();
        shards.insert(
            shard("/root"),
            LocalShard {
                shard_id: shard("/root"),
                block_api: api,
                max_listen_depth: 50,
            },
        );
        let local = LocalShardDeployService::new(shards);
        let err = local
            .current_height(&shard("/root"))
            .await
            .expect_err("a head that cannot be read must not anchor the deploy at genesis");
        assert!(
            err.contains("no head"),
            "and the refusal must name the failure, got: {err}"
        );
    }

    /// The reply channel must be a deploy id: anything else is a caller mistake, reported rather
    /// than answered from some arbitrary shard.
    #[tokio::test]
    async fn listen_for_data_at_name_rejects_a_name_that_is_not_a_deploy_id() {
        let (_, _, _, local) = gateway_with_counters().await;
        let err = local
            .listen_for_data_at_name(&DataAtNameQuery {
                depth: 1,
                name: RhoString::apply("not-a-deploy-id".to_string()),
            })
            .await
            .expect_err("a non-deploy-id name must be rejected");
        assert!(err.join(" ").contains("not a deploy id"), "{err:?}");
    }

    /// A deploy id the gateway never submitted has no shard to read from — reported, not guessed.
    #[tokio::test]
    async fn listen_for_data_at_name_rejects_an_unknown_deploy_id() {
        let (_, _, _, local) = gateway_with_counters().await;
        let err = local
            .listen_for_data_at_name(&DataAtNameQuery {
                depth: 1,
                name: RhoDeployId::apply(vec![9u8; 64]),
            })
            .await
            .expect_err("an unknown deploy id must be reported");
        assert!(err.join(" ").contains("no shard is known"), "{err:?}");
    }

    /// `await_reply` asks for the maximum depth, which the block API rejects outright; the service
    /// must clamp it to the shard's configured limit or no reply is ever seen.
    #[tokio::test]
    async fn listen_for_data_at_name_clamps_the_depth() {
        let (gateway, api_a, _, local) = gateway_with_counters().await;

        // Submit one phase deploy to the primary so the service knows which shard the id belongs to.
        let record = record();
        let leg = record.legs[0].clone();
        gateway.phase(&record, &leg, "prepare").await;
        assert_eq!(api_a.deploy_count(), 1);

        let sig = {
            // The deploy id the service recorded is the signature of the last submitted deploy; the
            // counting shard does not model signatures, so drive the map directly through a submit.
            let deployed = local.deployed.lock().unwrap();
            deployed.keys().next().cloned().expect("a submitted deploy")
        };
        let _ = local
            .listen_for_data_at_name(&DataAtNameQuery {
                depth: i32::MAX,
                name: RhoDeployId::apply(sig),
            })
            .await
            .expect("the listen itself succeeds");
        assert_eq!(
            api_a.last_depth(),
            Some(50),
            "the depth must be clamped to the shard's limit"
        );
    }

    /// A poisoned submit-map lock must degrade to "unknown shard" rather than aborting the node.
    #[tokio::test]
    async fn shard_of_returns_none_for_a_poisoned_lock() {
        let (_, _, _, local) = gateway_with_counters().await;
        let poisoner = local.clone();
        let handle = std::thread::spawn(move || {
            let _guard = poisoner.deployed.lock().expect("lock to poison");
            panic!("poison the map");
        });
        assert!(handle.join().is_err(), "the poisoner must panic");
        assert!(
            local.shard_of(&[1, 2, 3]).is_none(),
            "a poisoned lock reads as unknown, not as a panic"
        );
    }
}
