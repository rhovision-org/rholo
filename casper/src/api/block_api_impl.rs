//! The `BlockApi` implementation (port of `BlockApiImpl.scala`).
//!
//! Read paths hit the block store / DAG; write paths (`deploy`, `createBlock`, `getProposeResult`)
//! drive propose through a caller-supplied trigger + the shared `ProposerState`.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::{BlockDagStorage, DeployId};
use rchain_block_storage::dag::representation::DagRepresentation;
use rchain_graphz::ListSerializer;
use rchain_models::ast::Par;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::casper::protocol::casper_message::{BlockMessage, SignedDeployData};
use rchain_models::casper::protocol::deploy_service::{
    BlockInfo, ContinuationsWithBlockInfo, DataWithBlockInfo, DeployExecStatus,
    FinalityAttestation, LightBlockInfo, RawBlock, Status, VersionInfo, WaitingContinuationInfo,
};
use rchain_models::rholang::RhoType::RhoDeployId;
use rchain_models::sorter::sort_par_term;
use rchain_models::validator::Validator;
use rchain_rspace::hashing::stable_hash_provider::{hash_channel, hash_channels, hash_seq};
use rchain_rspace::trace::event::Event as REvent;
use rchain_shared::base16;

use crate::api::block_api::{
    get_full_block_info, get_light_block_info, ApiErr, BlockApi, Capabilities, DeployPoolStats,
    DeployerInfo, FinalityHealth, ProposeHealth, ProposerHealth, ShardHealthEntry,
};
use crate::api::graph_generator::{dag_as_cluster, ValidatorBlock};
use crate::api::machine_verifiable_dag::machine_verifiable_dag;
use crate::blocks::proposer::propose_result::{ProposeResult, ProposeStatus};
use crate::blocks::proposer::proposer::ProposerResult;
use crate::engine::node_phase::NodePhaseCell;
use crate::event_converter::to_rspace_event;
use crate::interpreter_util::ShardHealth;
use crate::runtime_manager::{CapturedReply, RuntimeManager};
use crate::state::ProposerState;
use crate::validator_identity::ValidatorIdentity;

/// The network snapshot needed by `status` (address + peer/node counts; the Scala
/// `F[(PeerNode, Connections, Seq[PeerNode])]` collapses to its rendered form).
#[derive(Clone, Debug)]
pub struct NetworkStatus {
    pub address: String,
    pub peers: i32,
    pub nodes: i32,
}

/// A propose trigger (port of `ProposeFunction[F] = Boolean => F[ProposerResult]`).
pub type ProposeFunction = Box<
    dyn Fn(bool) -> Pin<Box<dyn Future<Output = ProposerResult> + Send + 'static>> + Send + Sync,
>;

/// A network-status provider (port of `getNetworkStatus`). Async so it can read the live connection
/// cell and Kademlia discovery table (peers/nodes) rather than returning a fixed value.
pub type NetworkStatusFn =
    Box<dyn Fn() -> Pin<Box<dyn Future<Output = NetworkStatus> + Send + 'static>> + Send + Sync>;

/// **The pending-work relay** (Law 86e). Called after a deploy is admitted, with the signed deploy;
/// the implementation offers it to peers and returns immediately (it owns any spawned task). It is
/// an `Fn`, not an `async fn`, so the deploy response does not wait on a peer round-trip.
pub type RelayFunction = Arc<dyn Fn(SignedDeployData) + Send + Sync>;

/// The concrete `BlockApi` (port of `BlockApiImpl`).
pub struct BlockApiImpl {
    dag: Arc<dyn BlockDagStorage>,
    block_store: BlockStore,
    runtime: Arc<RuntimeManager>,
    validator_opt: Option<ValidatorIdentity>,
    network_id: String,
    shard_id: String,
    min_phlo_price: i64,
    version: String,
    network_status: NetworkStatusFn,
    is_node_read_only: bool,
    max_depth_limit: i32,
    dev_mode: bool,
    trigger_propose: Option<ProposeFunction>,
    proposer_state: Option<Arc<tokio::sync::Mutex<ProposerState>>>,
    auto_propose: bool,
    propose_on_deploy: bool,
    admin_http: bool,
    system_public_keys: BTreeSet<Vec<u8>>,
    /// #157: the shard's proposer health, read live. Not a `Capabilities` field: that reports what the
    /// node was configured to do, and this reports what it is doing — see `to_api_status`.
    propose_health: ProposeHealth,
    /// **This shard's finality-and-merge health** (W-N10-02, W-N10-03) — the cell the validation path
    /// writes, one per shard, beside `propose_health` and for the same reason. See [`ShardHealth`].
    shard_health: ShardHealth,
    /// **This shard's launch phase** (B47). Deploy admission is judged by the shard the deploy names,
    /// not by a process-wide flag: one syncing shard must not refuse every shard's work.
    phase: NodePhaseCell,
    /// The pending-work relay, when the operator enabled it (Law 86e). `None` admits locally only.
    relay: Option<RelayFunction>,
}

/// The messages at or above `lowest_height`, in the shape the DAG view renders.
///
/// Extracted from `visualize_dag` so the *empty view* is testable on its own, because it is a
/// legitimate request rather than an error: a DAG with no blocks yet, and a `start_block_number`
/// above the top of the chain, both filter to nothing here — and the renderer must render that empty
/// slice rather than index it (H2a, `graph_generator::dag_as_cluster`).
/// The block a **no-hash** `explore-deploy` reads (AUDIT C129).
///
/// It is the last **finalized** block, and the choice is deliberate rather than incidental.
///
/// The arm used to read the chain *tip* — the highest height, then an arbitrary hash from that
/// height's set (`height_map.iter().next_back()…hashes.iter().next()`) — on the argument that a
/// wallet checking a balance after its own transfer should not see the finalized fringe lag. Two
/// things are wrong with that as a default, and the second is why this is a decision rather than a
/// preference. An **arbitrary** hash from a height's set is not a block anyone chose: two honest
/// nodes can answer the same unauthenticated request from different blocks, so the read is not an
/// oracle. And the anchor is a block a **byzantine proposer can pick** — the route takes no caller
/// identity, and its answer is what a wallet believes about its own balance.
///
/// The finalized block is the one block in the DAG that is agreed by construction, which is what
/// `bond_status` and `last_finalized_block` already read. A caller that wants the tip asks for it by
/// hash (`explore-deploy-by-block-hash`), which is an explicit choice rather than this route's
/// silent one.
///
/// Extracted from the handler so the choice is testable without a node: the fixture builds the two
/// heights the decision is between.
fn exploration_anchor(dag: &DagRepresentation) -> Result<BlockHash, String> {
    dag.last_finalized_block_unsafe()
}

fn view_blocks_above(dag: &DagRepresentation, lowest_height: i64) -> Vec<ValidatorBlock> {
    let to_hash_str = |bytes: &[u8]| base16::encode(bytes).chars().take(5).collect::<String>();
    dag.dag_message_state
        .msg_map
        .values()
        .filter(|m| i64::from(m.height) >= lowest_height)
        .map(|m| ValidatorBlock {
            id: to_hash_str(m.id.as_bytes()),
            sender: to_hash_str(m.sender.as_bytes()),
            height: i64::from(m.height),
            justifications: m
                .parents
                .iter()
                .map(|h| to_hash_str(h.as_bytes()))
                .collect(),
            fringe: m.fringe.iter().map(|h| to_hash_str(h.as_bytes())).collect(),
        })
        .collect()
}

/// The heights a chain-data read may serve, newest first, cut to `depth` **finalised** heights.
///
/// **Nothing above the last finalised block is readable** (W-N3-04). `height_map` is the whole DAG,
/// including blocks this node has accepted and the network has *not* finalised, and this read used to
/// walk it from the top with no floor — so a caller could read the state of a block that may yet be
/// rolled back, which is reading a chain that does not exist yet.
///
/// The floor is a *height*, which is the cheap form of "is this finalised": finality is a prefix of
/// the chain, so a fringe cannot finalise out of order, and every block at or below the floor is
/// finalised while every block above it is not. The `depth` counts finalised heights only, because a
/// depth of "the last two blocks" is what a caller reading the chain is asking for.
///
/// `floor == None` is a DAG that has finalised nothing, and that is **no floor** rather than an empty
/// set: a node whose finality has not started — and every fixture that never builds a fringe — still
/// answers from what it holds, as it did before this existed. A `Some` floor that cannot be read is an
/// error rather than a `None` here, which is what keeps the two apart.
fn readable_heights(
    height_map: &BTreeMap<rchain_shared::refined::BlockHeight, BTreeSet<BlockHash>>,
    floor: Option<rchain_shared::refined::BlockHeight>,
    depth: usize,
) -> Vec<&BTreeSet<BlockHash>> {
    height_map
        .iter()
        .rev()
        .filter(|(height, _)| floor.is_none_or(|floor| **height <= floor))
        .take(depth)
        .map(|(_, hashes)| hashes)
        .collect()
}

/// Whether this node may sign a finality claim with `key`, and why not if it may not (W-N5-07).
///
/// **The propose path has had this gate since #70; the attestation path did not.** An attestation is
/// a statement the rest of the network counts toward quorum — the fringe rule requires a full
/// partition of attestations — so a signature from outside the active set is a claim with no standing
/// made under this node's name, and `finality_attestation` signed one for any caller that asked.
///
/// The three answers stay three, deliberately: `Ok(false)` from the gate is "not in the active set",
/// and an `Err` is "this node cannot read the DAG well enough to say". Both refuse, but only one of
/// them is about this node's bonds, and the propose path makes the same distinction (AUDIT C67) —
/// flattening them is what made a broken DAG read look like an unbonded validator.
pub async fn attestation_is_admissible(
    dag: &Arc<dyn BlockDagStorage>,
    key: &[u8],
) -> Result<(), String> {
    let sender = Validator::try_from(key).map_err(|e| e.to_string())?;
    match crate::blocks::proposer::proposer::is_active_validator(dag, &sender).await {
        Ok(true) => Ok(()),
        Ok(false) => {
            Err("this node is not in the active validator set; refusing to attest".to_string())
        }
        Err(e) => Err(format!("cannot decide whether this node is bonded: {e}")),
    }
}

impl BlockApiImpl {
    /// The name-data read, with or without the finality floor.
    ///
    /// One body rather than two, because the only difference is the floor and a second copy is a
    /// second place for the depth limit and the sort to drift. `finalised_only` is the public route;
    /// the node's own reply read is the other (W-N3-04).
    async fn name_data_response(
        &self,
        depth: i32,
        listening_name: &Par,
        finalised_only: bool,
    ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)> {
        if depth < 0 || depth > self.max_depth_limit {
            return Err(format!(
                "Your request on getListeningName depth {depth} exceed the max limit {}",
                self.max_depth_limit
            ));
        }
        let dag = self.dag.get_representation().await;
        let floor = match finalised_only {
            true => self.last_finalized_height().await?,
            false => None,
        };
        let depth_with_limit = depth.min(self.max_depth_limit) as usize;
        let sorted = sort_par_term(listening_name);
        let mut out: Vec<DataWithBlockInfo> = Vec::new();
        for hashes in readable_heights(&dag.height_map, floor, depth_with_limit) {
            for h in hashes {
                let block = self.get_block_unsafe(h).await?;
                if let Some(d) = self.get_data_with_block_info(&sorted, &block).await? {
                    out.push(d);
                }
            }
        }
        let len = out.len() as i32;
        Ok((out, len))
    }

    /// The height of the last finalised block, or `None` on a DAG that has finalised nothing.
    ///
    /// **The floor a read of chain data has to have** (W-N3-04). `height_map` is the whole DAG,
    /// including blocks this node has accepted and the network has *not* finalised, and it is
    /// height-ordered — so a height floor is the cheap form of "is this finalised": finality is a
    /// prefix of the chain (a fringe cannot finalise out of order), so every block at or below the
    /// floor is finalised and every block above it is not.
    ///
    /// A finalised hash with no metadata is an error rather than `None`: treating it as "no floor"
    /// would silently serve exactly what this exists to withhold, and an unreadable finalised block
    /// is a node in a state it should say so about.
    async fn last_finalized_height(&self) -> ApiErr<Option<rchain_shared::refined::BlockHeight>> {
        let dag = self.dag.get_representation().await;
        let Some(hash) = dag.last_finalized_block_hash() else {
            return Ok(None);
        };
        self.dag
            .lookup(&hash)
            .await?
            .map(|meta| meta.block_num)
            .ok_or_else(|| {
                format!(
                    "the DAG names {} as finalised but holds no metadata for it",
                    hash.to_hex()
                )
            })
            .map(Some)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dag: Arc<dyn BlockDagStorage>,
        block_store: BlockStore,
        runtime: Arc<RuntimeManager>,
        validator_opt: Option<ValidatorIdentity>,
        network_id: String,
        shard_id: String,
        min_phlo_price: i64,
        version: String,
        network_status: NetworkStatusFn,
        is_node_read_only: bool,
        max_depth_limit: i32,
        dev_mode: bool,
        trigger_propose: Option<ProposeFunction>,
        proposer_state: Option<Arc<tokio::sync::Mutex<ProposerState>>>,
        auto_propose: bool,
        propose_on_deploy: bool,
        admin_http: bool,
        system_public_keys: BTreeSet<Vec<u8>>,
        propose_health: ProposeHealth,
        shard_health: ShardHealth,
        phase: NodePhaseCell,
        relay: Option<RelayFunction>,
    ) -> Self {
        BlockApiImpl {
            dag,
            block_store,
            runtime,
            validator_opt,
            network_id,
            shard_id,
            min_phlo_price,
            version,
            network_status,
            is_node_read_only,
            max_depth_limit,
            dev_mode,
            trigger_propose,
            proposer_state,
            auto_propose,
            propose_on_deploy,
            admin_http,
            system_public_keys,
            propose_health,
            shard_health,
            phase,
            relay,
        }
    }

    /// Resolve a caller-supplied hash string to a block this node has **and has added**.
    ///
    /// Extracted from `get_block` when `get_raw_block` arrived, so the two read paths cannot drift on
    /// what counts as readable: the length floor and hex decode, the full-hash versus prefix lookup,
    /// and the `contains` check (a block the store holds but the DAG never took is answered as an
    /// error rather than served, which is the difference between "on this node" and "on this chain").
    async fn find_block(&self, hash: &str) -> Result<BlockMessage, String> {
        if hash.len() < 6 {
            return Err(format!(
                "Input hash value must be at least 6 characters: {hash}"
            ));
        }
        let hash_bytes = base16::decode(hash)
            .ok_or_else(|| format!("Input hash value is not valid hex string: {hash}"))?;
        let block_opt = if hash.len() == 64 {
            self.block_store
                .get(&[BlockHash::try_from(hash_bytes.as_slice()).map_err(|e| e.to_string())?])
                .await?
                .pop()
                .flatten()
        } else {
            let dag = self.dag.get_representation().await;
            match dag.find(hash) {
                Some(h) => self.block_store.get(&[h]).await?.pop().flatten(),
                None => None,
            }
        };
        let block =
            block_opt.ok_or_else(|| format!("Error: Failure to find block with hash: {hash}"))?;
        let dag = self.dag.get_representation().await;
        if dag.contains(&block.block_hash) {
            Ok(block)
        } else {
            Err(format!(
                "Error: Block with hash {hash} received but not added yet"
            ))
        }
    }

    async fn get_block_unsafe(&self, hash: &BlockHash) -> Result<BlockMessage, String> {
        self.block_store
            .get(&[*hash])
            .await?
            .pop()
            .flatten()
            .ok_or_else(|| format!("block {} was not found on this node", hash.to_hex()))
    }

    async fn get_data_at_par_raw(
        &self,
        par: &Par,
        block_hash: &BlockHash,
        use_pre_state_hash: bool,
    ) -> Result<(Vec<Par>, LightBlockInfo), String> {
        let block = self.get_block_unsafe(block_hash).await?;
        let sorted_par = sort_par_term(par);
        let state_hash = if use_pre_state_hash {
            block.pre_state_hash
        } else {
            block.post_state_hash
        };
        let data = self.runtime.get_data(&state_hash, &sorted_par).await?;
        let lbi = get_light_block_info(&block);
        Ok((data, lbi))
    }

    async fn get_data_with_block_info(
        &self,
        sorted_name: &Par,
        block: &BlockMessage,
    ) -> Result<Option<DataWithBlockInfo>, String> {
        if self.is_listening_name_reduced(block, &[sorted_name.clone()]) {
            let state_hash = block.post_state_hash;
            let data = self.runtime.get_data(&state_hash, sorted_name).await?;
            let block_info = get_light_block_info(block);
            Ok(Some(DataWithBlockInfo {
                post_block_data: data,
                block: block_info,
            }))
        } else {
            Ok(None)
        }
    }

    async fn get_continuations_with_block_info(
        &self,
        sorted_names: &[Par],
        block: &BlockMessage,
    ) -> Result<Option<ContinuationsWithBlockInfo>, String> {
        if self.is_listening_name_reduced(block, sorted_names) {
            let state_hash = block.post_state_hash;
            let continuations = self
                .runtime
                .get_continuation(&state_hash, sorted_names)
                .await?;
            let continuation_infos = continuations
                .into_iter()
                .map(|(patterns, cont)| WaitingContinuationInfo {
                    post_block_patterns: patterns,
                    post_block_continuation: cont,
                })
                .collect();
            let block_info = get_light_block_info(block);
            Ok(Some(ContinuationsWithBlockInfo {
                post_block_continuations: continuation_infos,
                block: block_info,
            }))
        } else {
            Ok(None)
        }
    }

    fn is_listening_name_reduced(&self, block: &BlockMessage, sorted_names: &[Par]) -> bool {
        let event_log: Vec<REvent> = block
            .state
            .deploys
            .iter()
            .flat_map(|d| d.deploy_log.iter())
            .map(to_rspace_event)
            .collect();
        event_log.iter().any(|ev| match ev {
            REvent::Produce(p) => {
                // A produce has exactly one channel; a query name with a different arity simply
                // does not match (must not panic on a user-supplied `Par`).
                sorted_names.len() == 1 && p.channels_hash == hash_channel(&sorted_names[0])
            }
            REvent::Consume(c) => {
                let mut c_hashes = c.channels_hashes.clone();
                c_hashes.sort();
                c_hashes == hash_seq(sorted_names)
            }
            REvent::Comm(comm) => {
                let mut c_hashes = comm.consume.channels_hashes.clone();
                c_hashes.sort();
                c_hashes == hash_seq(sorted_names)
                    || comm
                        .produces
                        .iter()
                        .any(|p| p.channels_hash == hash_channels(sorted_names))
            }
        })
    }
}

/// **The capability answer, split from the node so the one identity field can be tested without a
/// DAG, a store and a runtime.**
///
/// `proposer` is the only field here that is *identity* rather than *mode*: the other three are
/// configuration flags, and `manual_propose` is exactly `!autopropose && !proposeOnDeploy` — so an
/// unkeyed observer reports it `true`, the same as a manual-mode validator. Anything that needs to
/// know whether a node can include a deploy reads `proposer`; the test below is what keeps that
/// distinction from drifting back into the mode flags.
fn capability_answer(
    auto_propose: bool,
    propose_on_deploy: bool,
    admin_http: bool,
    dev_mode: bool,
    holds_validator_key: bool,
) -> Capabilities {
    Capabilities {
        autopropose: auto_propose,
        propose_on_deploy,
        manual_propose: !auto_propose && !propose_on_deploy,
        admin_http,
        dev_mode,
        proposer: holds_validator_key,
    }
}

#[async_trait]
impl BlockApi for BlockApiImpl {
    async fn status(&self) -> Status {
        let net = (self.network_status)().await;
        // Latest block number from the DAG (height map), not a hardcoded 0 — the devnet's autopropose
        // (dummy deploys) advances it continuously.
        let latest_block_number = self.dag.get_representation().await.latest_block_number();
        Status {
            version: VersionInfo {
                api: 1.to_string(),
                node: self.version.clone(),
            },
            address: net.address,
            network_id: self.network_id.clone(),
            shard_id: self.shard_id.clone(),
            peers: net.peers,
            nodes: net.nodes,
            min_phlo_price: self.min_phlo_price,
            latest_block_number,
        }
    }

    async fn pooled_deploys(&self) -> ApiErr<Vec<SignedDeployData>> {
        let pooled = self.dag.pooled_deploys().await?;
        Ok(pooled.into_values().collect())
    }

    async fn deploy_pool_stats(&self) -> ApiErr<DeployPoolStats> {
        let (deploys, bytes) = self.dag.deploy_pool_stats().await?;
        Ok(DeployPoolStats {
            deploys,
            max_deploys: crate::dag::MAX_POOLED_DEPLOYS,
            bytes,
            max_bytes: crate::dag::MAX_POOLED_DEPLOY_BYTES,
        })
    }

    async fn capabilities(&self) -> Capabilities {
        capability_answer(
            self.auto_propose,
            self.propose_on_deploy,
            self.admin_http,
            self.dev_mode,
            self.validator_opt.is_some(),
        )
    }

    async fn proposer_health(&self) -> ProposerHealth {
        self.propose_health.snapshot()
    }

    /// **This shard's own finality-and-merge observation** — read from the cell the validation path
    /// writes, never from a copy taken at setup (W-N10-02, W-N10-03).
    async fn finality_health(&self) -> FinalityHealth {
        let h = &self.shard_health;
        FinalityHealth {
            stall_reason: h.finality_stall_reason(),
            stall_sustained: h.finality_stall_sustained(),
            stall_episodes: h.stall_episodes(),
            non_quiet_merge_reports: h.non_quiet_merge_reports(),
            unrestorable_records: h.unrestorable_records(),
            unrestorable_block: h.unrestorable_block(),
            counters_durable: h.counters_durable(),
        }
    }

    /// **One entry, keyed by this shard** (W-N10-04). `BlockApiImpl` *is* one shard, so the map is
    /// total over everything it can answer for; the router merges the members' maps.
    async fn health_by_shard(&self) -> BTreeMap<String, ShardHealthEntry> {
        BTreeMap::from([(
            self.shard_id.clone(),
            ShardHealthEntry {
                proposer: self.propose_health.snapshot(),
                finality: self.finality_health().await,
            },
        )])
    }

    async fn deploy(&self, deploy: &SignedDeployData) -> ApiErr<String> {
        if self.is_node_read_only {
            return Err(
                "Deploy was rejected because node is running in read-only mode.".to_string(),
            );
        }
        // **A shard outside its serving phases cannot include work**, even when its pool has room.
        // `acceptsDeploys` remains the pool's Law 86a answer; readiness is this explicit refusal.
        // Applied here so HTTP, gRPC, the faucet and every other caller of the one deploy path
        // agree, and read from **this shard's** cell so one syncing shard does not refuse the
        // others. The global `sync_state` is not consulted: the shard's `Syncing` phase is the same
        // fact at the right scope.
        if !self.phase.admits_work() {
            return Err(format!(
                "Deploy refused: shard {} is in phase {} and is not ready to include work.",
                self.shard_id,
                self.phase.phase().label()
            ));
        }
        if crate::blocks::block_processor::pipeline_faults() > 0 {
            return Err(
                "Deploy refused: the block pipeline is blocked after a fault and is not ready to \
                 include work."
                    .to_string(),
            );
        }
        if crate::blocks::proposer::proposer::proposal_isolated() {
            return Err(
                "Deploy refused: this validator is isolated from finality and is not proposing; \
                 an operator repair/restart is required."
                    .to_string(),
            );
        }
        if !deploy.verify_signature() {
            return Err("Deploy signature is invalid.".to_string());
        }
        if deploy.data.phlo_limit < 0 {
            return Err("Deploy phlo limit must be non-negative.".to_string());
        }
        if deploy.data.shard_id != self.shard_id {
            return Err(format!(
                "Deploy shardId '{}' is not as expected network shard '{}'.",
                deploy.data.shard_id, self.shard_id
            ));
        }
        if self.system_public_keys.contains(&deploy.deployer) {
            return Err(
                "Deploy refused because it's signed with forbidden private key.".to_string(),
            );
        }
        if deploy.data.phlo_price < self.min_phlo_price {
            return Err(format!(
                "Phlo price {} is less than minimum price {}.",
                deploy.data.phlo_price, self.min_phlo_price
            ));
        }
        // Reject deploys anchored too far in the future (R28): a deploy with
        // `valid_after_block_number` far ahead of the tip is neither selected (it stays "future")
        // nor expired, so it would permanently occupy a deploy-pool slot.
        let latest_block = self.dag.get_representation().await.latest_block_number();
        if deploy.data.valid_after_block_number
            > latest_block + crate::multi_parent_casper::DEPLOY_LIFESPAN
        {
            return Err(format!(
                "Deploy validAfterBlockNumber {} is too far in the future (latest block {}).",
                deploy.data.valid_after_block_number, latest_block
            ));
        }
        // **And the other end of the same window** (issue #356). A deploy anchored *behind* the tip is
        // already expired: `select_deploys` drops it as `expired`, the pool slot is held, and the
        // submitter is answered `Success!` and told nothing — a deploy that can never be included.
        //
        // The asymmetry with the arm above is the reason this one exists: a **future** anchor becomes
        // selectable as the chain advances, so accepting it is accepting work the node will get to;
        // an **expired** one can only get worse, because the height only grows. Accepting it is
        // accepting a deploy that will never be a block.
        //
        // **This departs from the Scala oracle deliberately** — the Scala deploy API pools such a
        // deploy and its proposer silently skips it at selection, which is the behaviour the port had
        // and the one this issue is about. The deviation is registered in `spec/audit/passes.md` §6.
        if crate::validate::deploy_expired_at_block(
            deploy.data.valid_after_block_number,
            latest_block + 1,
            crate::multi_parent_casper::DEPLOY_LIFESPAN,
        ) {
            return Err(format!(
                "Deploy validAfterBlockNumber {} is already expired at block {} (the deploy lifespan \
                 is {} blocks): it can never be included. Anchor it at the current height — \
                 `rnode deploy` does that when `--valid-after-block-number` is omitted — or omit the \
                 anchor and let the node choose.",
                deploy.data.valid_after_block_number,
                latest_block + 1,
                crate::multi_parent_casper::DEPLOY_LIFESPAN
            ));
        }

        let r = match crate::multi_parent_casper::deploy(self.dag.as_ref(), deploy).await {
            Ok(id) => Ok(format!("Success!\nDeployId is: {}", base16::encode(&id))),
            Err(e) => Err(e.0),
        };

        // **Offer admitted work to peers before proposing** (Law 86e). The deploy is already in
        // this pool; the relay copies it to peers, so a constrained node that cannot execute it
        // still lets a properly equipped validator include it. The callback spawns its own task.
        if r.is_ok() {
            if let Some(relay) = &self.relay {
                relay(deploy.clone());
            }
        }

        // Auto-propose if enabled and a trigger is available.
        if self.propose_on_deploy || self.auto_propose {
            if let Some(trigger) = &self.trigger_propose {
                let _ = trigger(true).await;
            }
        }
        r
    }

    async fn deploy_status(&self, deploy_id: &DeployId) -> ApiErr<DeployExecStatus> {
        if let Some(block_hash) = self.dag.lookup_by_deploy_id(deploy_id).await? {
            // The deploy is indexed as belonging to this block, but the block body may not be in
            // the store yet (the proposer persists the body separately from the DAG index). Treat a
            // missing body as "still processing" so a polling caller keeps waiting instead of
            // surfacing a raw internal "missing block" error.
            let block = match self.block_store.get(&[block_hash]).await?.pop().flatten() {
                Some(block) => block,
                None => {
                    return Ok(DeployExecStatus::NotProcessed {
                        status: "Block not yet available".to_string(),
                    });
                }
            };
            let deploy_opt = block
                .state
                .deploys
                .iter()
                .find(|d| d.deploy.sig.as_slice() == deploy_id.as_slice());
            let deploy = deploy_opt.ok_or_else(|| {
                format!(
                    "Deploy not found in the block, blockHash: {}, deploy sig: {}",
                    block_hash.to_hex(),
                    base16::encode(deploy_id)
                )
            })?;
            if !deploy.is_failed {
                let deploy_id_ch = RhoDeployId::apply(deploy_id.clone());
                let (par, light_block) = self
                    .get_data_at_par_raw(&deploy_id_ch, &block_hash, false)
                    .await?;
                Ok(DeployExecStatus::ProcessedWithSuccess {
                    deploy_result: par,
                    block: light_block,
                })
            } else {
                // The failure reason the reducer recorded when the deploy ran (issue #15), read back
                // from the block rather than reported as unavailable.
                let light_block = get_light_block_info(&block);
                Ok(DeployExecStatus::ProcessedWithError {
                    deploy_error: deploy_error_text(deploy),
                    block: light_block,
                })
            }
        } else if self.dag.contains_deploy_in_pool(deploy_id).await? {
            Ok(DeployExecStatus::NotProcessed {
                status: "Pooled".to_string(),
            })
        } else {
            // The Scala's fourth `NotProcessed` reason, `"Running"`, is absent by construction: it came
            // from a per-node `BlockExecutionTracker` this port does not have, and the nearest
            // derivation ("in neither the DAG nor the pool") is also true of a deploy whose block was
            // just proposed. Registered in `spec/audit/passes.md` §6.
            Ok(DeployExecStatus::NotProcessed {
                status: "Unknown".to_string(),
            })
        }
    }

    async fn create_block(&self, is_async: bool) -> ApiErr<String> {
        let trigger = self
            .trigger_propose
            .as_ref()
            .ok_or_else(|| "Propose error: read-only node.".to_string())?;
        let r = trigger(is_async).await;
        Ok(match r {
            ProposerResult::Empty => {
                return Err("Failure: another propose is in progress".to_string());
            }
            ProposerResult::Failure {
                status, seq_number, ..
            } => {
                return Err(format!("Failure: {status} (seqNum {seq_number})"));
            }
            ProposerResult::Started { seq_number } => {
                format!("Propose started (seqNum {seq_number})")
            }
            ProposerResult::Success { block, .. } => {
                format!(
                    "Success! Block {} created and added.",
                    block.block_hash.to_hex()
                )
            }
        })
    }

    async fn get_propose_result(&self) -> ApiErr<String> {
        let state = self
            .proposer_state
            .as_ref()
            .ok_or_else(|| "Error: read-only node.".to_string())?;
        let mut s = state.lock().await;
        match s.curr_propose_result.take() {
            None => {
                let result = s.latest_propose_result.clone().unwrap_or((
                    ProposeResult {
                        propose_status: ProposeStatus::NotEnoughNewBlocks,
                    },
                    None,
                ));
                Ok(propose_result_message(result))
            }
            Some(rx) => {
                drop(s);
                let result = rx.await.map_err(|e| e.to_string())?;
                Ok(propose_result_message(result))
            }
        }
    }

    async fn get_listening_name_data_response(
        &self,
        depth: i32,
        listening_name: &Par,
    ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)> {
        self.name_data_response(depth, listening_name, true).await
    }

    /// **The one way out of a durable halt** — see the trait's doc.
    ///
    /// The store is cleared **first**, and its failure is propagated: a timer woken before the record
    /// is gone would resume, halt again on the next failure, and find the old record still standing —
    /// so a clear that did not reach the store cannot be allowed to read as a resume.
    async fn clear_autopropose_halt(&self) -> Result<(), String> {
        self.dag.clear_halt_record().await?;
        self.propose_health.clear_halt();
        Ok(())
    }

    /// The same read with no finality floor, for the node's own reply channel — see the trait's doc
    /// for why the bridge is not the caller the floor is written for.
    async fn get_unfinalised_name_data_response(
        &self,
        depth: i32,
        listening_name: &Par,
    ) -> ApiErr<(Vec<DataWithBlockInfo>, i32)> {
        self.name_data_response(depth, listening_name, false).await
    }

    async fn get_listening_name_continuation_response(
        &self,
        depth: i32,
        listening_names: &[Par],
    ) -> ApiErr<(Vec<ContinuationsWithBlockInfo>, i32)> {
        if depth < 0 || depth > self.max_depth_limit {
            return Err(format!(
                "Your request on getListeningNameContinuation depth {depth} exceed the max limit {}",
                self.max_depth_limit
            ));
        }
        let dag = self.dag.get_representation().await;
        let floor = self.last_finalized_height().await?;
        let depth_with_limit = depth.min(self.max_depth_limit) as usize;
        let sorted: Vec<Par> = listening_names.iter().map(sort_par_term).collect();
        let mut out: Vec<ContinuationsWithBlockInfo> = Vec::new();
        for hashes in readable_heights(&dag.height_map, floor, depth_with_limit) {
            for h in hashes {
                let block = self.get_block_unsafe(h).await?;
                if let Some(c) = self
                    .get_continuations_with_block_info(&sorted, &block)
                    .await?
                {
                    out.push(c);
                }
            }
        }
        let len = out.len() as i32;
        Ok((out, len))
    }

    async fn get_blocks_by_heights(
        &self,
        start_block_number: i64,
        end_block_number: i64,
    ) -> ApiErr<Vec<LightBlockInfo>> {
        let range = end_block_number.checked_sub(start_block_number).ok_or_else(|| {
            format!(
                "Your request startBlockNumber {start_block_number} and endBlockNumber {end_block_number} exceed the max limit {}",
                self.max_depth_limit
            )
        })?;
        if range > self.max_depth_limit as i64 {
            return Err(format!(
                "Your request startBlockNumber {start_block_number} and endBlockNumber {end_block_number} exceed the max limit {}",
                self.max_depth_limit
            ));
        }
        let dag = self.dag.get_representation().await;
        let topo = dag
            .topo_sort_unsafe(start_block_number, Some(end_block_number))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for hashes in &topo {
            for h in hashes {
                let block = self.get_block_unsafe(h).await?;
                out.push(get_light_block_info(&block));
            }
        }
        Ok(out)
    }

    async fn visualize_dag(
        &self,
        depth: i32,
        start_block_number: i32,
        _show_justification_lines: bool,
    ) -> ApiErr<Vec<String>> {
        let dag = self.dag.get_representation().await;
        let start = start_block_number.max(0);
        let start_block_num = if start == 0 {
            dag.latest_block_number()
        } else {
            start as i64
        };
        let depth_limited = if depth <= 0 || depth > self.max_depth_limit {
            self.max_depth_limit
        } else {
            depth
        };
        let lowest_height = start_block_num - depth_limited as i64;
        let blocks = view_blocks_above(&dag, lowest_height);

        let mut ser = ListSerializer::default();
        dag_as_cluster(&blocks, &mut ser);
        Ok(ser.buf)
    }

    async fn machine_verifiable_dag(&self, depth: i32) -> ApiErr<String> {
        if depth > self.max_depth_limit {
            return Err(format!(
                "Your request depth {depth} exceed the max limit {}",
                self.max_depth_limit
            ));
        }
        let dag = self.dag.get_representation().await;
        let start = dag.latest_block_number() - depth as i64;
        let topo = dag
            .topo_sort_unsafe(start, None)
            .map_err(|e| e.to_string())?;
        let edges = machine_verifiable_dag(&topo, |h| async move {
            let block = self.get_block_unsafe(&h).await?;
            Ok::<Vec<BlockHash>, String>(block.justifications)
        })
        .await?;
        Ok(edges
            .iter()
            .map(|e| e.show())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn get_blocks(&self, depth: i32) -> ApiErr<Vec<LightBlockInfo>> {
        if depth > self.max_depth_limit {
            return Err(format!(
                "Your request depth {depth} exceed the max limit {}",
                self.max_depth_limit
            ));
        }
        let dag = self.dag.get_representation().await;
        let start = dag.latest_block_number() - depth as i64;
        let topo = dag
            .topo_sort_unsafe(start, None)
            .map_err(|e| e.to_string())?;
        let mut acc: Vec<LightBlockInfo> = Vec::new();
        for hashes in &topo {
            let mut infos = Vec::new();
            for h in hashes {
                let block = self.get_block_unsafe(h).await?;
                infos.push(get_light_block_info(&block));
            }
            acc.extend(infos);
        }
        acc.reverse();
        Ok(acc)
    }

    async fn find_deploy(&self, id: &DeployId) -> ApiErr<LightBlockInfo> {
        match self.dag.lookup_by_deploy_id(id).await? {
            Some(h) => {
                let block = self.get_block_unsafe(&h).await?;
                Ok(get_light_block_info(&block))
            }
            None => Err(format!(
                "Couldn't find block containing deploy with id: {}",
                base16::encode(id)
            )),
        }
    }

    async fn find_deployer(&self, deployer_hash: &[u8]) -> ApiErr<DeployerInfo> {
        let lookup = self.dag.lookup_by_deployer(deployer_hash).await?;
        let block = match lookup.block {
            Some(h) => Some(get_light_block_info(&self.get_block_unsafe(&h).await?)),
            None => None,
        };
        Ok(DeployerInfo {
            block,
            indexed_from_height: lookup.indexed_from,
        })
    }

    async fn get_block(&self, hash: &str) -> ApiErr<BlockInfo> {
        let block = self.find_block(hash).await?;
        Ok(get_full_block_info(&block))
    }

    async fn get_raw_block(&self, hash: &str) -> ApiErr<RawBlock> {
        let block = self.find_block(hash).await?;
        // `encoded` is `to_bytes_cleared()`, which is *exactly* the preimage `hash_block` hashes —
        // so a client that hashes it and gets `block_hash` back has verified every other field in it
        // (the heights, the seq_num, the justifications and the bond map all live inside these
        // bytes). `sig` travels beside the preimage because the cleared form empties it by
        // construction, and it is over the 32 bytes of `block_hash`, not over `encoded`.
        Ok(RawBlock {
            block_hash: base16::encode(block.block_hash.as_bytes()),
            encoded: base16::encode(&block.to_bytes_cleared()),
            sender: base16::encode(block.sender.as_bytes()),
            sig_algorithm: block.sig_algorithm.clone(),
            sig: base16::encode(&block.sig),
        })
    }

    async fn finality_attestation(&self, nonce: &str) -> ApiErr<FinalityAttestation> {
        let Some(identity) = self.validator_opt.as_ref() else {
            return Err("this node has no validator identity to attest with".to_string());
        };
        // The nonce is the caller's, and it is signed rather than echoed: an attestation is good for
        // the conversation that asked for it and no other (see `attestation_prehash`). Refusing an
        // empty one is the freshness rule stated where it can be checked — a statement with no
        // caller-supplied part is a statement anyone can replay.
        let nonce_bytes =
            base16::decode(nonce).ok_or_else(|| format!("nonce is not valid hex: {nonce}"))?;
        if nonce_bytes.len() < 16 {
            return Err(format!(
                "nonce must be at least 16 bytes of entropy (hex), got {}",
                nonce_bytes.len()
            ));
        }
        // **Before the signature, not after** (W-N5-07): a claim this node is not entitled to make
        // must not exist even briefly, because the caller can publish it.
        attestation_is_admissible(&self.dag, identity.public_key.bytes()).await?;
        let dag = self.dag.get_representation().await;
        let hash = dag.last_finalized_block_unsafe()?;
        let sig = identity
            .attest_finality(&nonce_bytes, &hash)
            .map_err(|e| e.to_string())?;
        Ok(FinalityAttestation {
            block_hash: base16::encode(hash.as_bytes()),
            nonce: nonce.to_string(),
            sender: base16::encode(identity.public_key.bytes()),
            sig_algorithm: identity.sig_algorithm.clone(),
            sig: base16::encode(&sig),
        })
    }

    async fn bond_status(&self, public_key: &[u8]) -> ApiErr<bool> {
        let dag = self.dag.get_representation().await;
        let hash = dag.last_finalized_block_unsafe()?;
        let block = self.get_block_unsafe(&hash).await?;
        let state_hash = block.post_state_hash;
        let bonds = self.runtime.compute_bonds(&state_hash).await?;
        Ok(bonds.contains_key(&Validator::try_from(public_key).map_err(|e| e.to_string())?))
    }

    async fn exploratory_deploy(
        &self,
        term: &str,
        block_hash: Option<&str>,
        use_pre_state_hash: bool,
    ) -> ApiErr<(CapturedReply, LightBlockInfo)> {
        if !(self.is_node_read_only || self.dev_mode) {
            return Err("Exploratory deploy can only be executed on read-only RNode.".to_string());
        }
        let dag = self.dag.get_representation().await;
        let target: Option<BlockMessage> = match block_hash {
            None => {
                let hash = exploration_anchor(&dag)?;
                self.block_store.get(&[hash]).await?.pop().flatten()
            }
            Some(h) => {
                let bytes = base16::decode(h)
                    .ok_or_else(|| format!("Input hash value is not valid hex string: {h}"))?;
                self.block_store
                    .get(&[BlockHash::try_from(bytes.as_slice()).map_err(|e| e.to_string())?])
                    .await?
                    .pop()
                    .flatten()
            }
        };
        match target {
            None => Err(format!("Can not find block {block_hash:?}")),
            Some(b) => {
                let state_hash = if use_pre_state_hash {
                    b.pre_state_hash
                } else {
                    b.post_state_hash
                };
                let res = self
                    .runtime
                    .play_exploratory_deploy(term, &state_hash)
                    .await?;
                let lbi = get_light_block_info(&b);
                Ok((res, lbi))
            }
        }
    }

    async fn get_data_at_par(
        &self,
        par: &Par,
        block_hash: &str,
        use_pre_state_hash: bool,
    ) -> ApiErr<(Vec<Par>, LightBlockInfo)> {
        let bytes = base16::decode(block_hash)
            .ok_or_else(|| format!("Invalid block hash base 16 encoding, {block_hash}"))?;
        let hash = BlockHash::try_from(bytes.as_slice()).map_err(|e| e.to_string())?;
        self.get_data_at_par_raw(par, &hash, use_pre_state_hash)
            .await
    }

    async fn last_finalized_block(&self) -> ApiErr<BlockInfo> {
        let dag = self.dag.get_representation().await;
        let hash = dag.last_finalized_block_unsafe()?;
        let block = self.get_block_unsafe(&hash).await?;
        Ok(get_full_block_info(&block))
    }

    async fn is_finalized(&self, hash: &str) -> ApiErr<bool> {
        let dag = self.dag.get_representation().await;
        let given =
            BlockHash::try_from(base16::try_decode(hash)?.as_slice()).map_err(|e| e.to_string())?;
        Ok(dag.is_finalized(&given))
    }

    async fn get_latest_message(&self) -> ApiErr<BlockMetadata> {
        let validator = self
            .validator_opt
            .as_ref()
            .ok_or_else(|| "Validator not available (read-only node).".to_string())?;
        let validator_bytes = validator.public_key.bytes();
        let dag = self.dag.get_representation().await;
        let latest_id = dag
            .dag_message_state
            .latest_msgs
            .get(&Validator::from_slice(validator_bytes))
            .map(|m| m.id);
        match latest_id {
            Some(id) => self
                .dag
                .lookup(&id)
                .await?
                .ok_or_else(|| "No block message for latest message.".to_string()),
            None => Err("No block message for validator.".to_string()),
        }
    }
}

fn propose_result_message(result: (ProposeResult, Option<BlockMessage>)) -> String {
    match result.1 {
        Some(block) => format!(
            "Success! Block {} created and added.",
            block.block_hash.to_hex()
        ),
        None => result.0.propose_status.to_string(),
    }
}

/// The text for a failed deploy's `ProcessedWithError`.
///
/// The Scala reads it out of the node's **execution tracker** — a per-node cache of the deploys this
/// node's block creator ran (`BlockApiImpl.scala:200-207`,
/// `executionTracker.findDeploy(deployId).collect { case DeployStatusError(msg) => msg }`) — and falls
/// back to `"<deploy error message not available in cache or deploy executed on another node>"` when
/// the tracker has nothing, which is exactly what a node that did *not* run the deploy returns. This
/// port keeps the reason in the block instead: `ProcessedDeploy.system_deploy_error` is where the
/// reducer's failure was recorded when the deploy ran (issue #15, `runtime_manager.rs`), it is
/// replayed and compared like the rest of the deploy's record, and every node holding the block has
/// it. So the answer is the same message wherever the Scala has one, and *better* than the Scala's
/// placeholder where it does not — with the placeholder kept for a deploy whose record carries no
/// message (a block from a node that predates issue #15, or a failure recorded without a reason).
fn deploy_error_text(
    deploy: &rchain_models::casper::protocol::casper_message::ProcessedDeploy,
) -> String {
    deploy
        .system_deploy_error
        .as_deref()
        .filter(|message| !message.is_empty())
        .unwrap_or(
            "<deploy error message not available in cache or deploy executed on another node>",
        )
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_models::casper::protocol::casper_message::RholangState;

    /// **The discriminator is identity, not mode.**
    ///
    /// Both arms are the same configuration (`--autopropose` off), so `manualPropose` is `true` for
    /// both — which is the whole reason it cannot be the field a hostlist filters on. Only the
    /// validator key separates a manual-mode validator from an observer. Falsified by computing
    /// `proposer` from any of the mode flags: one of the two assertions then flips.
    #[test]
    fn proposer_is_the_validator_identity_not_the_mode() {
        let validator = capability_answer(false, false, false, false, true);
        let observer = capability_answer(false, false, false, false, false);
        assert_eq!(
            (validator.manual_propose, observer.manual_propose),
            (true, true),
            "the mode flag cannot tell them apart — that is the trap this field exists to remove"
        );
        assert!(validator.proposer, "a keyed manual validator can propose");
        assert!(
            !observer.proposer,
            "an unkeyed observer cannot propose, whatever its mode"
        );

        // Autopropose does not change the identity answer either.
        assert!(capability_answer(true, true, false, false, true).proposer);
        assert!(!capability_answer(true, true, false, false, false).proposer);
    }

    fn block() -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([1u8; 32]),
            block_number: 0.try_into().unwrap(),
            sender: Validator::new([0u8; 65]),
            seq_num: 0.try_into().unwrap(),
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            justifications: vec![],
            bonds: std::collections::BTreeMap::new(),
            rejected_deploys: std::collections::BTreeSet::new(),
            rejected_blocks: std::collections::BTreeSet::new(),
            rejected_senders: std::collections::BTreeSet::new(),
            state: RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![],
            timestamp: 0,
        }
    }

    /// **AUDIT C129.** The no-hash anchor is the last **finalized** block, not the chain tip.
    ///
    /// The fixture is exactly the case that distinguishes them: height 1 holds a block the fringe
    /// has not finalized, so the old tip read answered with it — and picked it *arbitrarily* out of
    /// **Nothing above the last finalised block is read** (W-N3-04).
    ///
    /// `data-at-name` and its continuation sibling walked `height_map` — the *whole* DAG, including
    /// blocks this node has accepted and the network has not finalised — from the top with no floor,
    /// so a caller could read the state of a block that may yet be rolled back.
    ///
    /// Falsified by dropping the floor filter: the third and fourth assertions then see height 5,
    /// which is exactly the block the row is about.
    #[test]
    fn only_finalised_heights_are_readable() {
        use rchain_shared::refined::BlockHeight;

        let at = |h: i64| BlockHeight::try_from(h).expect("a height");
        let hash = |b: u8| BlockHash::new([b; 32]);
        let map: BTreeMap<BlockHeight, BTreeSet<BlockHash>> = (1..=5)
            .map(|h| (at(h), BTreeSet::from([hash(h as u8)])))
            .collect();

        // Newest first, and cut to the requested depth.
        let newest = readable_heights(&map, None, 3);
        assert_eq!(newest.len(), 3);
        assert!(
            newest[0].contains(&hash(5)),
            "the newest height comes first"
        );

        // **The floor.** Heights 3, 2 and 1 are readable; 5 and 4 are not.
        let finalised = readable_heights(&map, Some(at(3)), 10);
        assert_eq!(finalised.len(), 3, "heights 3, 2 and 1");
        assert!(
            !finalised
                .iter()
                .any(|set| set.contains(&hash(5)) || set.contains(&hash(4))),
            "no block above the floor may be read: {finalised:?}"
        );
        assert!(
            finalised[0].contains(&hash(3)),
            "and the floor itself is finalised, so it is read"
        );

        // **The depth counts finalised heights.** With the floor at 3 and a depth of 2 the read is
        // heights 3 and 2 — not 5 and 4, which is what counting from the top would give.
        let cut = readable_heights(&map, Some(at(3)), 2);
        assert_eq!(cut.len(), 2);
        assert!(cut[0].contains(&hash(3)) && cut[1].contains(&hash(2)));
    }

    /// that height's set. The finalized block at height 0 is the only block two honest nodes must
    /// agree on, which is the whole reason the read is anchored there.
    #[test]
    fn the_no_hash_exploration_read_is_anchored_to_the_finalized_block() {
        use std::collections::BTreeMap;

        use rchain_block_storage::dag::finalizer::Message;
        use rchain_block_storage::dag::message_state::DagMessageState;
        use rchain_shared::refined::{BlockHeight, SeqNum};

        let finalized = BlockHash::new([0x0a; 32]);
        let tip = BlockHash::new([0x0b; 32]);
        let sender = Validator::new([0u8; 65]);
        // The finalized block is a *message in the message map*, and the latest justification carries
        // it in its fringe — which is the construction `latest_fringe` reads
        // (`message_map.rs`: the max-height justification's fringe, mapped through `msg_at`). A
        // fixture with an empty fringe resolves to no finalized block at all and would test the
        // error arm instead of the choice.
        let finalized_msg = Message {
            id: finalized,
            height: BlockHeight::try_from(0).unwrap(),
            sender: sender.clone(),
            sender_seq: SeqNum::try_from(0).unwrap(),
            bonds_map: BTreeMap::new(),
            parents: BTreeSet::new(),
            fringe: BTreeSet::from([finalized]),
            seen: Arc::new(BTreeSet::from([finalized])),
        };
        let mut latest_msgs = BTreeMap::new();
        latest_msgs.insert(sender.clone(), finalized_msg.clone());
        let mut msg_map = BTreeMap::new();
        msg_map.insert(finalized, finalized_msg);
        let dag = DagRepresentation {
            dag_set: Arc::new(BTreeSet::from([finalized, tip])),
            child_map: Arc::new(BTreeMap::new()),
            height_map: Arc::new(BTreeMap::from([
                (
                    BlockHeight::try_from(0).unwrap(),
                    BTreeSet::from([finalized]),
                ),
                (BlockHeight::try_from(1).unwrap(), BTreeSet::from([tip])),
            ])),
            dag_message_state: DagMessageState::from_parts(latest_msgs, msg_map),
        };

        assert_eq!(
            exploration_anchor(&dag).expect("the fringe is available"),
            finalized,
            "the anchor must be the finalized block, not the height-1 tip the old read picked"
        );
        assert_ne!(
            exploration_anchor(&dag).unwrap(),
            tip,
            "the tip is reachable by hash, but it is not this route's default"
        );
    }

    /// **A failed deploy reports the reason the reducer gave it**, not a placeholder. The three arms
    /// are the three real cases: a message (the common one — `runtime_manager.rs` records the
    /// reducer's first error), no message (a block from a node that predates issue #15), and an empty
    /// message (a failure recorded without a reason, which the API must not render as nothing at all).
    #[test]
    fn a_failed_deploy_reports_the_recorded_reason() {
        use rchain_models::casper::protocol::casper_message::{
            DeployData, PCost, ProcessedDeploy, SignedDeployData,
        };

        fn failed_with(error: Option<&str>) -> ProcessedDeploy {
            ProcessedDeploy {
                deploy: SignedDeployData {
                    data: DeployData {
                        attachments: Vec::new(),
                        term: "Nil".to_string(),
                        timestamp: 0,
                        phlo_price: 1,
                        phlo_limit: 10,
                        valid_after_block_number: 0,
                        shard_id: "root".to_string(),
                    },
                    deployer: vec![0u8; 65],
                    sig: vec![1, 2, 3],
                    sig_algorithm: "secp256k1".to_string(),
                },
                cost: PCost { cost: 0 },
                deploy_log: Vec::new(),
                is_failed: true,
                system_deploy_error: error.map(str::to_string),
            }
        }

        assert_eq!(
            deploy_error_text(&failed_with(Some("Out of phlogistons after 7 steps"))),
            "Out of phlogistons after 7 steps"
        );
        assert!(
            deploy_error_text(&failed_with(None))
                .starts_with("<deploy error message not available"),
            "a deploy whose record carries no message keeps the Scala's placeholder"
        );
        assert!(
            deploy_error_text(&failed_with(Some("")))
                .starts_with("<deploy error message not available"),
            "an empty message is no message"
        );
    }

    #[test]
    fn propose_result_message_renders_success_and_failure() {
        let ok = propose_result_message((
            ProposeResult {
                propose_status: ProposeStatus::ProposeSuccess,
            },
            Some(block()),
        ));
        assert!(ok.starts_with("Success! Block "));

        let err = propose_result_message((
            ProposeResult {
                propose_status: ProposeStatus::NoNewDeploys,
            },
            None,
        ));
        assert_eq!(err, "Proposal failed: NoNewDeploys");
    }

    /// A representation holding one message per `(height, id byte)` pair.
    fn repr_with(blocks: &[(i64, u8)]) -> DagRepresentation {
        use rchain_block_storage::dag::finalizer::Message;
        use rchain_block_storage::dag::message_state::DagMessageState;
        use rchain_shared::refined::{BlockHeight, SeqNum};
        use std::collections::BTreeMap;

        let mut state = DagMessageState::<BlockHash, Validator>::empty();
        for (height, byte) in blocks {
            let id = BlockHash::new([*byte; 32]);
            state.insert_msg_mut(&Message {
                id,
                height: BlockHeight::try_from(*height).unwrap(),
                sender: Validator::new([0u8; 65]),
                sender_seq: SeqNum::try_from(0).unwrap(),
                bonds_map: BTreeMap::new(),
                parents: BTreeSet::new(),
                fringe: BTreeSet::new(),
                seen: Arc::new([id].into_iter().collect::<BTreeSet<_>>()),
            });
        }
        DagRepresentation {
            dag_set: Arc::new(BTreeSet::new()),
            child_map: Arc::new(BTreeMap::new()),
            height_map: Arc::new(BTreeMap::new()),
            dag_message_state: state,
        }
    }

    /// **H2a's reachable inputs, and the filter that produces them.** `visualize_dag` renders what is
    /// at or above `start_block_number - depth`, so both an empty DAG and a `start_block_number` above
    /// the top of the chain filter to nothing — the second being the case a caller reaches with an
    /// ordinary request for a window above the chain. The renderer handles the empty slice
    /// (`graph_generator::tests::an_empty_view_renders_an_empty_graph`); this pins that the filter
    /// really produces it, with a control showing the filter does keep what is at the bound.
    #[test]
    fn a_view_of_nothing_is_reachable_from_the_filter() {
        assert!(
            view_blocks_above(&repr_with(&[]), 0).is_empty(),
            "a DAG with no blocks renders nothing"
        );

        let one_at_five = repr_with(&[(5, 7)]);
        assert!(
            view_blocks_above(&one_at_five, 999).is_empty(),
            "every block below the window's lowest height renders nothing"
        );
        assert_eq!(
            view_blocks_above(&one_at_five, 5).len(),
            1,
            "the control: the block at the bound is kept, so this test can fail"
        );
    }
}
