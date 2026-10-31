//! Casper merge index data structures (Law 17: merge determinism).
//!
//! Ports the pure data types, conflict/dependency relations, and the effectful constructors
//! (`DeployChainIndex.apply`, `BlockIndex.apply`, `MergeScope.merge`) from `casper/.../merging/`.

use rchain_shared::lock::Unpoison;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_block_storage::dag::finalizer::Message;
use rchain_block_storage::dag::finalizer::NoAdvance;
use rchain_block_storage::dag::message_map;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::public_key::PublicKey;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::casper::protocol::casper_message::{
    BlockMessage, Event, ProcessedDeploy, ProcessedSystemDeploy, SystemDeployData,
};
use rchain_models::runtime::{BindPattern, ListParWithRandom, TaggedContinuation};
use rchain_models::sorted::SortedProc;
use rchain_models::validator::Validator;
use rchain_rholang::merging::SidecarRecord;
use rchain_rholang::merging::{calculate_number_channel_merge, read_mergeable_values};
use rchain_rholang::native_state::{
    pos_vault_key, pos_vault_put_action, vault_key, vault_put_action, NativeSystemState,
};
use rchain_rholang::storage::RhoHistoryRepository;
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_rspace::history::history_repository::HistoryRepository;
use rchain_rspace::hot_store_trie_action::HotStoreTrieAction;
use rchain_rspace::merger::event_log_index::{EventLogIndex, NumberChannelsDiff};
use rchain_rspace::merger::event_log_merging_logic::{are_conflicting, depends};
use rchain_rspace::merger::state_change::StateChange;
use rchain_rspace::merger::state_change_merger::compute_trie_actions;
use rchain_rspace::native_store::{BlockNativeEffects, InMemNativeStore, NativeStoreAction};
use rchain_rspace::trace::event::{Event as REvent, Produce};
use rchain_sdk::dag::merging::{
    compute_dependency_map, compute_greedy_non_intersecting_branches,
    compute_relation_map_for_merge_set, resolve_conflict_set_with_census, SearchBudget,
    SearchCensus,
};
use rchain_shared::refined::{BlockHeight, NonNegI64};
use rchain_shared::serialize::Serialize;

use crate::block_random_seed::BlockRandomSeed;
use crate::event_converter::to_rspace_event;
use crate::interpreter_util::is_genesis_pre_state;
use crate::runtime_manager::RuntimeManager;

/// **The merge search's real input, which nothing observed (#117).**
///
/// `compute_rejection_options` (`sdk/src/dag/merging.rs`) expands one state per nonempty subset of the
/// conflict set that induces an acyclic subgraph of the conflict relation, and its cost is exponential
/// in how many chains can be accepted together. That much is now a pinned, deterministic fact. What was
/// *not* pinned — and what the defect was argued about with instead — is the shape the running node
/// actually hands it: how wide a merge scope gets and how dense its conflicts are. These accumulate
/// that, so the answer is read off a running node rather than assumed.
///
/// Atomics rather than a return value because `MergeScope::merge` is async and has no log handle; its
/// callers do, and they read this. Every value is a monotone maximum or a count, so a lost race can
/// only under-report, never invent.
///
/// **An instrument for #117: remove it with the fix.**
pub mod search_census {
    use super::SearchCensus;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    /// Merges that ran a conflict search.
    pub static MERGES: AtomicU64 = AtomicU64::new(0);
    /// Widest conflict set seen (chains in one merge scope's conflict search).
    pub static MAX_KEYS: AtomicUsize = AtomicUsize::new(0);
    /// Densest conflict map seen (`(key, conflict)` pairs).
    pub static MAX_CONFLICTS: AtomicUsize = AtomicUsize::new(0);
    /// Most asymmetric pairs seen in one conflict map — `0` everywhere means the exact
    /// output-sensitive rewrite is the symmetric (maximal-independent-set) case.
    pub static MAX_ASYMMETRIC: AtomicUsize = AtomicUsize::new(0);
    /// Most states the search ever expanded on one merge.
    pub static MAX_EXPANDED: AtomicUsize = AtomicUsize::new(0);

    /// **The distributions, which the maxima above cannot give.** A maximum answers "how bad was the
    /// worst merge"; a bound on this class needs "how *often* is a scope wide, and how often is a merge
    /// expensive", because that is what a threshold is chosen from and what a price is set against
    /// (C182, #127's change record, Stage 1).
    ///
    /// **Two of them, because the first run showed width is not the cost.** The quiet devnet reached 43
    /// chains and 899,236 states while the census run's storm reached 1,663,395 at 35 chains — so the
    /// conflict *density* moves the cost more than the width does, and a threshold keyed on width alone
    /// would be keyed on the wrong quantity. `states_expanded` is the cost itself, and its distribution
    /// is what a threshold should be read off.
    pub const WIDTH_EDGES: [usize; 4] = [16, 32, 64, 128];
    /// The cost's edges, logarithmic: below 10³ nothing is worth gating, and 10⁶ is the order that
    /// reached gigabytes in the heap profile.
    ///
    /// **These edges are calibrated against the *old* step unit and the histogram is not comparable
    /// across C178.** Every count ever published under these buckets — the 899,236 and 1,663,395 and
    /// 2,026,511 figures C182 cites — was a count of **accepted-set** states expanded. The directed path
    /// now counts **distinct rejected sets** instead (`SearchCensus::expanded`,
    /// `sdk/src/dag/merging.rs`), which is a strictly smaller number for the same merge and a different
    /// quantity in the same units: on a dependency chain of `n` chains the old unit gave `2^n - 1` and
    /// the new one gives `n`. So a sample that moves bucket after the quotient moved for two reasons at
    /// once, and reading a bucket drop as "the merge got cheaper" is wrong until the distribution is
    /// re-measured on its own arm. That re-measurement is owed and is what C182 closes on.
    pub const EXPANDED_EDGES: [usize; 4] = [1_000, 10_000, 100_000, 1_000_000];

    /// Cumulative counts per bucket: `COUNTS[i]` is the number of samples `<= EDGES[i]`, and the last
    /// entry counts everything past the last edge (the shape Prometheus histograms use).
    pub type Buckets<const N: usize> = [AtomicUsize; N];
    pub static WIDTH_COUNTS: Buckets<5> = [const { AtomicUsize::new(0) }; 5];
    pub static EXPANDED_COUNTS: Buckets<5> = [const { AtomicUsize::new(0) }; 5];
    /// What has already been handed to the registry, per bucket, for each distribution.
    static WIDTH_PUBLISHED: Buckets<5> = [const { AtomicUsize::new(0) }; 5];
    static EXPANDED_PUBLISHED: Buckets<5> = [const { AtomicUsize::new(0) }; 5];

    /// The exact sums: `TOTAL_WIDTH / MERGES` and `TOTAL_EXPANDED / MERGES` are the **means**, where a
    /// histogram valued at bucket edges can only offer a mean *of edges* (C182's second defect).
    pub static TOTAL_WIDTH: AtomicU64 = AtomicU64::new(0);
    pub static TOTAL_EXPANDED: AtomicU64 = AtomicU64::new(0);

    /// Which bucket a value falls in: the first edge it is `<=`, or the open end past the last. Pure, so
    /// the boundaries can be tested without touching the process-wide counters — a test asserting bucket
    /// counts would race every other test in the binary that runs a merge, which is the fixture-isolation
    /// rule this repository already records.
    pub fn bucket_index(edges: &[usize], value: usize) -> usize {
        edges
            .iter()
            .position(|edge| value <= *edge)
            .unwrap_or(edges.len())
    }

    /// What is new since the last call, per bucket, as `(edge, samples)`; the last entry is the
    /// open-ended bucket. `record` accumulates, so a running total would double-count, and the publish
    /// rides the DAG's write path under the guard that path holds — nothing races this read-and-set.
    pub fn take_deltas(
        edges: &[usize],
        counts: &Buckets<5>,
        published: &Buckets<5>,
    ) -> [(usize, usize); 5] {
        let mut out = [(0usize, 0usize); 5];
        for i in 0..5 {
            let total = counts[i].load(Ordering::Relaxed);
            let seen = published[i].swap(total, Ordering::Relaxed);
            out[i] = (
                edges.get(i).copied().unwrap_or(usize::MAX),
                total.saturating_sub(seen),
            );
        }
        out
    }

    /// The width distribution's deltas. See `take_deltas`.
    pub fn take_width_deltas() -> [(usize, usize); 5] {
        take_deltas(&WIDTH_EDGES, &WIDTH_COUNTS, &WIDTH_PUBLISHED)
    }

    /// The cost distribution's deltas. See `take_deltas`.
    pub fn take_expanded_deltas() -> [(usize, usize); 5] {
        take_deltas(&EXPANDED_EDGES, &EXPANDED_COUNTS, &EXPANDED_PUBLISHED)
    }

    pub fn record(census: &SearchCensus) {
        MERGES.fetch_add(1, Ordering::Relaxed);
        TOTAL_WIDTH.fetch_add(census.keys as u64, Ordering::Relaxed);
        TOTAL_EXPANDED.fetch_add(census.expanded as u64, Ordering::Relaxed);
        MAX_KEYS.fetch_max(census.keys, Ordering::Relaxed);
        MAX_CONFLICTS.fetch_max(census.conflicts, Ordering::Relaxed);
        MAX_ASYMMETRIC.fetch_max(census.asymmetric, Ordering::Relaxed);
        MAX_EXPANDED.fetch_max(census.expanded, Ordering::Relaxed);
        let width = bucket_index(&WIDTH_EDGES, census.keys);
        WIDTH_COUNTS[width].fetch_add(1, Ordering::Relaxed);
        let cost = bucket_index(&EXPANDED_EDGES, census.expanded);
        EXPANDED_COUNTS[cost].fetch_add(1, Ordering::Relaxed);
    }

    /// One line, for a caller that has a `Log`.
    pub fn summary() -> String {
        format!(
            "merge search: {} merges · widest scope {} chains / {} conflict pairs / {} asymmetric · \
             most states expanded on one merge {} · width buckets {:?} · cost buckets {:?}",
            MERGES.load(Ordering::Relaxed),
            MAX_KEYS.load(Ordering::Relaxed),
            MAX_CONFLICTS.load(Ordering::Relaxed),
            MAX_ASYMMETRIC.load(Ordering::Relaxed),
            MAX_EXPANDED.load(Ordering::Relaxed),
            WIDTH_COUNTS.each_ref().map(|c| c.load(Ordering::Relaxed)),
            EXPANDED_COUNTS.each_ref().map(|c| c.load(Ordering::Relaxed)),
        )
    }
}

/// A deploy id paired with its execution cost (port of `DeployIdWithCost`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeployIdWithCost {
    pub id: Vec<u8>,
    pub cost: i64,
}

/// **One deploy's cost-accounting effect on the vaults, as deltas** (AUDIT C207).
///
/// Cost accounting writes `pos:vault` from **every** user deploy, so a block's native sidecar used to
/// carry that write and therefore overlapped every concurrent sibling on one slot — and the merge
/// resolves an overlap by rejecting a whole block, which silently took the deploy's *own* writes (a
/// delegation, a bond, a trust) with it. These three numbers are what the merge re-applies instead,
/// per accepted deploy, so the shared slot stops being carried by anyone.
///
/// All three are pure functions of the deploy and its recorded cost, computed where the chain index
/// is built (`BlockIndex::apply`), which is what makes carrying them cheaper than carrying values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CostMoves {
    /// The deployer's REV address — charged, and refunded.
    pub deployer: String,
    /// Debited from the deployer's vault and credited to the staking vault.
    pub charge: i64,
    /// Credited back to the deployer — the phlo it did not burn.
    pub refund: i64,
    /// The phlo it burned. The producer's share is a fraction of this, taken at merge time from the
    /// chain's `executor_share`.
    pub burned: i64,
}

/// The index of a single deploy (port of `DeployIndex`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployIndex {
    pub deploy_id: Vec<u8>,
    pub cost: i64,
    pub event_log_index: EventLogIndex,
}

impl Ord for DeployIndex {
    fn cmp(&self, other: &Self) -> Ordering {
        self.deploy_id.cmp(&other.deploy_id)
    }
}

impl PartialOrd for DeployIndex {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl DeployIndex {
    pub const SYS_SLASH_DEPLOY_COST: i64 = 0;
    pub const SYS_CLOSE_BLOCK_DEPLOY_COST: i64 = 0;
    pub const SYS_EMPTY_DEPLOY_COST: i64 = 0;

    pub fn sys_slash_deploy_id() -> Vec<u8> {
        vec![1]
    }
    pub fn sys_close_block_deploy_id() -> Vec<u8> {
        vec![2]
    }
    pub fn sys_empty_deploy_id() -> Vec<u8> {
        vec![3]
    }
}

/// **What a merge did, in the terms an operator needs** (#280).
///
/// `MergeScope::merge` has no log handle — the `search_census` module says why — so it *returns* what
/// it did and the caller that holds a logger reports it. The precedent is
/// [`ParentsMergedState::finality_stall`], which exists for exactly this reason.
///
/// **Why the incident needed it.** A merge on the live net rejected a boundary chain and, with it, a
/// user deploy that had ridden in on that block. The rejected deploy's id went into the block's
/// `rejectedDeploys`, the deploy's own status stayed `ProcessedWithSuccess`, and **no counter and no
/// line anywhere moved** — `reject_whole_blocks` had no log handle, and `search_census::record` ran
/// before it, so even the merge-work gauges never saw it. This is that missing ledger.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    /// Chains in the **conflict** scope, and how they resolved. **This qualifier is load-bearing**
    /// (C260): the final scope's own rejections are the `final_scope_rejected_chains` field below, and
    /// until it existed nothing distinguished a rejection of one from a rejection of the other.
    pub conflict_chains: usize,
    pub kept_chains: usize,
    pub rejected_chains: usize,
    /// **The final scope's rejections — the ledger's missing row** (C260). A chain of a *finalised*
    /// block whose derived rejection set names its first deploy. This records a historical
    /// rejection decision, which is expected when finalizing deliberately conflicting branches.
    /// It is not by itself evidence of final-state disagreement or an unexplained rejection.
    ///
    /// **It was visible nowhere.** Not in `rejected_chains` (conflict scope), not in the merge's
    /// `rejected_deploys` (`rejected_ids` is built from the conflict scope), not in `describe()`, and
    /// not in `nonQuietMergeReports` — `is_quiet()` could not see it, so the counter that exists to
    /// make a non-quiet merge visible never moved. I1 could not catch it either: its own filter
    /// exempts final-scope chains by construction (`conflict_set.contains`).
    ///
    /// **What it does not mean.** The finalised chain is not dropped and no write is lost: a
    /// final-scope rejection cascades only to its *conflict-scope dependents* (`incompatible_with_final`
    /// in `sdk/src/dag/merging.rs`), and the finalised chain's own effects are already in the base
    /// state. This is the ledger's missing row, not a lost write.
    pub final_scope_rejected_chains: usize,
    /// The native slots the **rejected** chains would have written, with the host that wrote each:
    /// the effects of a rejection, named. They had no name before this.
    pub dropped_native_slots: Vec<(Blake2b256Hash, (u8, Blake2b256Hash))>,
    /// Rejected chains that carried cost-accounting moves — **the #280 class**, where a deploy's own
    /// economics die with a contention it had no part in. A count, because the ids are in the merge's
    /// `rejected_deploys` beside it.
    pub rejected_cost_accounted_chains: usize,
    /// **I1 — every rejected chain conflicts with a kept chain, or depends on a rejected one.** A
    /// rejection with neither is a chain dropped for no reason of its own, which is precisely what
    /// the block-level rule did; this is the checkable form of that complaint, and it must be empty.
    pub rejections_without_a_conflict: Vec<Vec<u8>>,
    /// **I2 — every slot a kept chain wrote has its action in the merged batch.** A violation is a
    /// kept chain's write silently disappearing: the other half of the same injury. Must be empty.
    pub unapplied_kept_writes: Vec<(u8, Blake2b256Hash)>,
    /// Who won each slot in the merged state. The map the incident had no way to read.
    pub native_writer_of_slot: BTreeMap<(u8, Blake2b256Hash), Blake2b256Hash>,
}

impl MergeReport {
    /// Whether there is anything here an operator should see.
    ///
    /// Final-scope rejection decisions remain visible (C260), including expected decisions
    /// from a conflicting-branch campaign. The I1/I2 fields distinguish unexplained
    /// rejections and lost writes from this historical rejection ledger.
    pub fn is_quiet(&self) -> bool {
        self.rejected_chains == 0
            && self.final_scope_rejected_chains == 0
            && self.rejections_without_a_conflict.is_empty()
            && self.unapplied_kept_writes.is_empty()
    }

    /// One line for the caller's log. The lists are capped: this is a per-merge `warn`, and a
    /// peer-sized list of ids on one line is a line nobody reads.
    pub fn describe(&self) -> String {
        let mut line = format!(
            "merge: {} chains in scope ({} kept, {} rejected; {} rejected a cost-accounted chain)",
            self.conflict_chains,
            self.kept_chains,
            self.rejected_chains,
            self.rejected_cost_accounted_chains
        );
        // Named where a reader of the line meets it, and named as what it is: not a lost write — the
        // finalized state's decisions are retained in the base. This ledger remains visible
        // without claiming that a recorded rejection alone proves divergent finality (C260).
        if self.final_scope_rejected_chains > 0 {
            line.push_str(&format!(
                "; **{} chain(s) of the final scope rejected**: recorded final-scope rejection \
                 decisions retained in the merge base (C260)",
                self.final_scope_rejected_chains
            ));
        }
        if !self.dropped_native_slots.is_empty() {
            line.push_str(&format!(
                "; {} native slot(s) went unwritten ({})",
                self.dropped_native_slots.len(),
                describe_slots(self.dropped_native_slots.iter().map(|(h, s)| (*h, *s)))
            ));
        }
        if !self.rejections_without_a_conflict.is_empty() {
            line.push_str(&format!(
                "; **I1 VIOLATED**: {} chain(s) rejected without a conflict or a rejected dependency \
                 ({})",
                self.rejections_without_a_conflict.len(),
                describe_ids(
                    &self
                        .rejections_without_a_conflict
                        .iter()
                        .map(|id| rchain_shared::base16::encode(id))
                        .collect::<Vec<_>>()
                )
            ));
        }
        if !self.unapplied_kept_writes.is_empty() {
            line.push_str(&format!(
                "; **I2 VIOLATED**: {} kept chain write(s) absent from the merged batch ({})",
                self.unapplied_kept_writes.len(),
                describe_plain_slots(&self.unapplied_kept_writes)
            ));
        }
        line
    }
}

/// **What a merge produced**: the state, the deploys it rejected, and what it did.
///
/// A struct rather than the `(state, rejected)` tuple it used to be, because the third element is the
/// point: the report is how a merge that drops something becomes visible, and a tuple has nowhere to
/// put it (#280).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeOutcome {
    pub state: Blake2b256Hash,
    pub rejected_deploys: BTreeSet<Vec<u8>>,
    pub report: MergeReport,
}

/// A list of `(prefix, key)` slots, first four then a count.
fn rejection_has_a_reason<D: Ord>(
    chain: &D,
    kept: &BTreeSet<D>,
    rejected: &BTreeSet<D>,
    conflicts_map: &BTreeMap<D, BTreeSet<D>>,
    dependency_map: &BTreeMap<D, BTreeSet<D>>,
    accepted_finally: &BTreeSet<D>,
    rejected_finally: &BTreeSet<D>,
) -> bool {
    let conflicts = |a: &D, b: &D| {
        conflicts_map.get(a).is_some_and(|s| s.contains(b))
            || conflicts_map.get(b).is_some_and(|s| s.contains(a))
    };

    // Direct conflict with a chain that survives is a reason to reject this chain.
    if kept.union(accepted_finally).any(|k| conflicts(chain, k)) {
        return true;
    }

    // `dependency_map` is keyed by the dependency and points to its dependents.  The resolver's
    // `with_dependencies` walks exactly this edge when a rejection cascades, so a rejected chain is
    // justified when some other rejected chain has this chain among its dependents.
    rejected.union(rejected_finally).any(|r| {
        r != chain
            && dependency_map
                .get(r)
                .is_some_and(|dependents| dependents.contains(chain))
    })
}

fn describe_plain_slots(slots: &[(u8, Blake2b256Hash)]) -> String {
    let rendered: Vec<String> = slots
        .iter()
        .map(|(prefix, key)| format!("{prefix:02x}/{}", key.to_hex()))
        .collect();
    describe_ids(&rendered)
}

/// The first four slots of a list, then a count — the id lists on this path are peer-sized.
fn describe_slots(items: impl Iterator<Item = (Blake2b256Hash, (u8, Blake2b256Hash))>) -> String {
    let all: Vec<String> = items
        .map(|(host, (prefix, key))| format!("{}:{:02x}/{}", host.to_hex(), prefix, key.to_hex()))
        .collect();
    describe_ids(&all)
}

/// The first four entries of a list of rendered ids, then a count of the rest.
fn describe_ids(items: &[String]) -> String {
    if items.len() <= 4 {
        items.join(", ")
    } else {
        format!("{}, +{} more", items[..4].join(", "), items.len() - 4)
    }
}

/// The merged state seen by a block's parents (port of `ParentsMergedState`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParentsMergedState {
    /// Why the fringe derivation did not advance, when it did not — the gate's own reason
    /// (`NoAdvance`), carried here so the caller that holds a logger can report it. `None` means the
    /// fringe advanced or the walk had nothing to publish from the start.
    pub finality_stall: Option<NoAdvance<Validator>>,
    /// **What the merge that produced this pre-state did** (#280), carried for the same reason
    /// `finality_stall` is: `MergeScope::merge` has no log handle, and the caller does.
    pub merge_report: MergeReport,
    pub justifications: Vec<BlockMetadata>,
    pub max_block_num: i64,
    pub max_seq_nums: BTreeMap<Validator, i64>,
    pub fringe: BTreeSet<BlockHash>,
    pub fringe_state: Blake2b256Hash,
    /// **The fringe this computation started from, and the cache key it looked up** (#139).
    ///
    /// `fringe_state` above is where the merge *ended*; these two are where it began, and they are
    /// what makes a state disagreement legible: the replay's `close_block` anchors the next epoch's
    /// seed to the fringe state, so two nodes that began from different fringes replay the same block
    /// to different post-states. `prev_fringe` empty with `prev_fringe_lookup == hash(∅)` is the
    /// signature of a node whose restored blocks carry no fringe at all
    /// (`node_syncing.rs`'s `populate_dag`), and that is a fact about this node rather than about the
    /// block — which is why it is reported and not inferred.
    pub prev_fringe: BTreeSet<BlockHash>,
    pub prev_fringe_lookup: Blake2b256Hash,
    pub fringe_bonds_map: BTreeMap<Validator, NonNegI64>,
    /// **The participation the epoch reward's absence rule reads** (B4, #150): each validator's latest
    /// message height *in the fringe this merge computed*, from `liveness::latest_heights` over the
    /// fringe's own messages.
    ///
    /// Derived here, where that `Message` set is still alive, and deliberately from the **same**
    /// `prev_fringe` the `fringe_state` above is looked up under — re-deriving it from the DAG
    /// representation instead is the one way play and replay could disagree about *which* fringe was
    /// meant, which is what the `prev_fringe`/`prev_fringe_lookup` pair below exists to make legible.
    pub participation: BTreeMap<Validator, BlockHeight>,
    pub fringe_rejected_deploys: BTreeSet<Vec<u8>>,
    pub pre_state_hash: Blake2b256Hash,
    pub rejected_deploys: BTreeSet<Vec<u8>>,
}

/// The index of deploys depending on each other within a single block (port of `DeployChainIndex`).
#[derive(Clone, Debug)]
pub struct DeployChainIndex {
    pub host_block: Blake2b256Hash,
    pub deploys_with_cost: BTreeSet<DeployIdWithCost>,
    pub pre_state_hash: Blake2b256Hash,
    pub post_state_hash: Blake2b256Hash,
    pub event_log_index: EventLogIndex,
    pub state_changes: StateChange,
    /// **The cost-accounting moves this chain's deploys made** (AUDIT C207), by deploy id — what the
    /// merge re-applies per accepted chain. Deliberately **outside the chain's identity**: the one key
    /// below is `(host_block, post_state_hash, deploys_with_cost)`, so this field cannot move a chain's
    /// equality, its hash, or the rejection-option computation.
    pub cost_moves: BTreeMap<Vec<u8>, CostMoves>,
    /// The address the producer's share of this chain's deploys is paid to — the **host block's own
    /// signed sender**, which is what `pay_executor` reads on both play and replay. Outside the
    /// chain's identity, for the same reason as `cost_moves`.
    pub executor: String,
    /// **The native writes of this chain's own deploys, and of no other chain's** (#280).
    ///
    /// This is the field the fix is made of. A block's native effects used to live on the block
    /// alone, which left the merge no relation to key a conflict on but the *host* — so a chain that
    /// wrote nothing contended was still a conflict partner, and `reject_whole_blocks` dropped it
    /// with its host. Here the relation reads the chain's own slots, and a chain with none cannot
    /// conflict. Outside the chain's identity, for the same reason as `cost_moves`: the one key is
    /// `(host_block, post_state_hash, deploys_with_cost)`, so this field cannot move law 17a's
    /// rejection-option key.
    pub native_effects: Vec<NativeStoreAction>,
    /// **Where this chain sits in its block's deploy order** — the smallest ordinal among the deploys
    /// it carries, where the ordinal is the position in `state.deploys` followed by
    /// `state.system_deploys`.
    ///
    /// Load-bearing in exactly one place, and it is new with the chain-level relation: two chains of
    /// one block that wrote a common slot are not concurrent, so the native relation does not order
    /// them — but the merge applies absolute values, and the later chain's value was computed on top
    /// of the earlier one's. Without this, resolution could keep the later and drop the earlier and
    /// apply a value built on a write the state does not hold.
    pub first_deploy_ordinal: u32,
}

// **Ordering, equality and hashing are one key**, and this is a correction rather than a port.
//
// The Scala kept the two apart — `Ordering.by((hostBlock, postStateHash))` beside an `equals` over
// `deploysWithCost` — and that is legal there, because a Scala `TreeMap` reads only the `Ordering`.
// Rust is not: `Ord` **must** agree with `Eq` (`a == b` iff `a.cmp(b) == Equal`), because `BTreeMap`,
// `BTreeSet` and `sort` all read the order as the identity. Kept apart they did exactly what the
// std docs say they must not: two chains of one block — same host, same post-state, different deploys
// — compared `Ordering::Equal` while `==` said they differed, so a `BTreeSet` of chains silently held
// **one** of them and a `BTreeMap` keyed on one answered `get` for the other.
//
// On the chain-level native relation (#280) that is not a lost lookup but a **hang**: the dependency
// map held one such key whose value was the other chain, so `get` returned that value for *either*
// chain, and `traverse_tree` — which has no visited set — walked the one-element cycle for ever.
// `merging::native_merge_tests::a_blocks_own_chains_are_ordered_and_dependent` is the reproduction,
// and it hung CI on #281 rather than failing it.
//
// The Scala ordering's primary pair is kept, so the relative order of chains from *different* blocks
// is unchanged; the equality key is the tie-breaker, which is what makes the order total.
impl PartialEq for DeployChainIndex {
    fn eq(&self, other: &Self) -> bool {
        self.order_key() == other.order_key()
    }
}

impl Eq for DeployChainIndex {}

impl Hash for DeployChainIndex {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.order_key().hash(state);
    }
}

impl PartialOrd for DeployChainIndex {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DeployChainIndex {
    fn cmp(&self, other: &Self) -> Ordering {
        self.order_key().cmp(&other.order_key())
    }
}

impl DeployChainIndex {
    /// The one key that ordering, equality and hashing share — see the `Ord` impl above for why Rust
    /// cannot let those three be three different things, and for the hang that proved it.
    ///
    /// Borrowed, not cloned: `cmp` is called from the merge search's inner loop.
    fn order_key(&self) -> (Blake2b256Hash, Blake2b256Hash, &BTreeSet<DeployIdWithCost>) {
        (
            self.host_block,
            self.post_state_hash,
            &self.deploys_with_cost,
        )
    }

    /// The native slots this chain wrote — the domain of the native relation (#280).
    ///
    /// Computed rather than stored: the chain's `native_effects` are the authority, and a second
    /// field holding the same thing in another shape is a second thing to keep in step.
    pub fn native_slots(&self) -> BTreeSet<(u8, Blake2b256Hash)> {
        self.native_effects
            .iter()
            .map(NativeStoreAction::slot)
            .collect()
    }

    /// The total cost of the deploy chain (port of `deployChainCost`).
    pub fn deploy_chain_cost(r: &DeployChainIndex) -> i64 {
        r.deploys_with_cost.iter().map(|d| d.cost).sum()
    }

    /// Whether `a` depends on `b` (port of `depends`).
    pub fn depends(a: &DeployChainIndex, b: &DeployChainIndex) -> bool {
        depends(&a.event_log_index, &b.event_log_index)
    }

    /// Whether two sets of deploy chains conflict (port of `branchesAreConflicting`).
    ///
    /// Fallible because combining a set's indices can overflow the numeric-channel accumulation
    /// (AUDIT C41): the alternative was to answer `true` — "conflicting" — on an un-computable sum,
    /// which would be a silent semantic choice where the merge's own arithmetic returns an error.
    pub fn branches_are_conflicting(
        a: &BTreeSet<Arc<DeployChainIndex>>,
        b: &BTreeSet<Arc<DeployChainIndex>>,
    ) -> Result<bool, String> {
        let a_ids: BTreeSet<&Vec<u8>> = a
            .iter()
            .flat_map(|d| d.deploys_with_cost.iter().map(|x| &x.id))
            .collect();
        let b_ids: BTreeSet<&Vec<u8>> = b
            .iter()
            .flat_map(|d| d.deploys_with_cost.iter().map(|x| &x.id))
            .collect();
        let mut a_event = EventLogIndex::empty();
        for d in a {
            a_event = EventLogIndex::combine(&a_event, &d.event_log_index)?;
        }
        let mut b_event = EventLogIndex::empty();
        for d in b {
            b_event = EventLogIndex::combine(&b_event, &d.event_log_index)?;
        }
        Ok(!a_ids.is_disjoint(&b_ids) || are_conflicting(&a_event, &b_event))
    }

    /// Whether two deploy chains conflict (port of `deploysAreConflicting`).
    pub fn deploys_are_conflicting(a: &DeployChainIndex, b: &DeployChainIndex) -> bool {
        let a_ids: BTreeSet<&Vec<u8>> = a.deploys_with_cost.iter().map(|x| &x.id).collect();
        let b_ids: BTreeSet<&Vec<u8>> = b.deploys_with_cost.iter().map(|x| &x.id).collect();
        !a_ids.is_disjoint(&b_ids) || are_conflicting(&a.event_log_index, &b.event_log_index)
    }

    /// Build a deploy chain from its member deploys + pre/post state (port of
    /// `DeployChainIndex.apply`).
    pub async fn apply<C, P, A, K>(
        host_block: Blake2b256Hash,
        deploys: &BTreeSet<DeployIndex>,
        cost_moves: BTreeMap<Vec<u8>, CostMoves>,
        executor: String,
        pre_state_hash: Blake2b256Hash,
        post_state_hash: Blake2b256Hash,
        history_repository: &HistoryRepository<C, P, A, K>,
        native_effects: Vec<NativeStoreAction>,
        first_deploy_ordinal: u32,
    ) -> Result<DeployChainIndex, String>
    where
        C: Serialize<C> + Send + Sync + 'static,
        P: Serialize<P> + Send + Sync + 'static,
        A: Serialize<A> + Send + Sync + 'static,
        K: Serialize<K> + Send + Sync + 'static,
    {
        let deploys_with_cost: BTreeSet<DeployIdWithCost> = deploys
            .iter()
            .map(|d| DeployIdWithCost {
                id: d.deploy_id.clone(),
                cost: d.cost,
            })
            .collect();
        let mut event_log_index = EventLogIndex::empty();
        for d in deploys {
            event_log_index = EventLogIndex::combine(&event_log_index, &d.event_log_index)?;
        }

        let pre_reader = history_repository.get_history_reader(pre_state_hash).await;
        let pre_binary = pre_reader.reader_binary();
        let post_reader = history_repository.get_history_reader(post_state_hash).await;
        let post_binary = post_reader.reader_binary();

        let state_changes =
            StateChange::apply(pre_binary.as_ref(), post_binary.as_ref(), &event_log_index).await?;

        Ok(DeployChainIndex {
            host_block,
            deploys_with_cost,
            pre_state_hash,
            post_state_hash,
            event_log_index,
            state_changes,
            cost_moves,
            executor,
            native_effects,
            first_deploy_ordinal,
        })
    }
}

/// The index of a block: its deploy chains (port of `BlockIndex`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockIndex {
    pub block_hash: BlockHash,
    /// The block's deploy chains, `Arc`-shared.
    ///
    /// A `DeployChainIndex` carries an `EventLogIndex` (sets of produces and consumes) and a
    /// `StateChange`, so it is a large value — and the merge path does not merely read chains, it
    /// copies them into *sets keyed by chain*: the conflict and final scope sets, the
    /// accepted/rejected-against-the-finally sets, the mergeable-diff map, and every set and map the
    /// SDK's `resolve_conflict_set`, `compute_rejection_options` and relation maps build on top of
    /// those. Sharing makes each of those copies a refcount bump. Measured under a devnet fork storm
    /// (#117), that copy traffic was the dominant allocation site: a heap profile put
    /// `resolve_conflict_set`'s `BTreeMap::clone` at 99,982 allocations against a 35 MiB peak.
    ///
    /// Safe because a chain index is immutable once built, and every comparison the port relies on is
    /// preserved: `Ord` delegates to the inner `(host_block, post_state_hash)`, and `Eq`/`Hash` to
    /// the inner `deploys_with_cost`, exactly as they do for the owned value.
    pub deploy_chains: Vec<Arc<DeployChainIndex>>,
}

impl BlockIndex {
    /// Build an `EventLogIndex` from a deploy's events against the pre-state (port of
    /// `BlockIndex.createEventLogIndex`).
    pub async fn create_event_log_index<C, P, A, K>(
        events: &[Event],
        history_repository: &HistoryRepository<C, P, A, K>,
        pre_state_hash: Blake2b256Hash,
        mergeable_chs: NumberChannelsDiff,
    ) -> Result<EventLogIndex, String>
    where
        C: Serialize<C> + Send + Sync + 'static,
        P: Serialize<P> + Send + Sync + 'static,
        A: Serialize<A> + Send + Sync + 'static,
        K: Serialize<K> + Send + Sync + 'static,
    {
        let pre_reader = history_repository.get_history_reader(pre_state_hash).await;
        let rspace_events: Vec<REvent> = events.iter().map(to_rspace_event).collect();

        // Collect the distinct produces referenced by the trace to resolve the two pre-state
        // predicates (`produceExistsInPreState` and `produceTouchesPreStateJoin`).
        let mut produces: BTreeSet<Produce> = BTreeSet::new();
        for e in &rspace_events {
            match e {
                REvent::Produce(p) => {
                    produces.insert(p.clone());
                }
                REvent::Comm(c) => {
                    for p in &c.produces {
                        produces.insert(p.clone());
                    }
                }
                REvent::Consume(_) => {}
            }
        }

        let mut exists_in_pre_state: BTreeSet<Produce> = BTreeSet::new();
        let mut touches_pre_state_join: BTreeSet<Produce> = BTreeSet::new();
        for p in &produces {
            let data = pre_reader
                .get_data(p.channels_hash)
                .await
                .map_err(|e| e.to_string())?;
            if data.iter().any(|d| d.source == *p) {
                exists_in_pre_state.insert(p.clone());
            }
            let joins = pre_reader
                .get_joins(p.channels_hash)
                .await
                .map_err(|e| e.to_string())?;
            if joins.iter().any(|j| j.len() > 1) {
                touches_pre_state_join.insert(p.clone());
            }
        }

        Ok(EventLogIndex::apply(
            &rspace_events,
            |p| exists_in_pre_state.contains(p),
            |p| touches_pre_state_join.contains(p),
            mergeable_chs,
        ))
    }

    /// Build a `BlockIndex` from the processed deploys + mergeable-channel data (port of
    /// `BlockIndex.apply`).
    #[allow(clippy::too_many_arguments)]
    pub async fn apply<C, P, A, K>(
        block_hash: BlockHash,
        usr_processed_deploys: &[ProcessedDeploy],
        sys_processed_deploys: &[ProcessedSystemDeploy],
        pre_state_hash: Blake2b256Hash,
        post_state_hash: Blake2b256Hash,
        history_repository: &HistoryRepository<C, P, A, K>,
        mergeable_chan_data: &[NumberChannelsDiff],
        effects: BlockNativeEffects,
        executor: String,
    ) -> Result<BlockIndex, String>
    where
        C: Serialize<C> + Send + Sync + 'static,
        P: Serialize<P> + Send + Sync + 'static,
        A: Serialize<A> + Send + Sync + 'static,
        K: Serialize<K> + Send + Sync + 'static,
    {
        let usr_count = usr_processed_deploys.len();
        let deploy_count = usr_count + sys_processed_deploys.len();
        let mrg_count = mergeable_chan_data.len();
        if deploy_count != mrg_count {
            return Err(format!(
                "Cache of mergeable channels ({mrg_count}) doesn't match deploys count ({deploy_count})."
            ));
        }

        let (usr_mergeable, sys_mergeable) = mergeable_chan_data.split_at(usr_count);

        let mut deploy_indices: BTreeSet<DeployIndex> = BTreeSet::new();
        // **Where each indexed deploy sits in the block's list** (#280): its position in
        // `state.deploys` followed by `state.system_deploys`. The native sidecar is keyed by it, so
        // this is what lets a slot's write be placed on the chain that carries its deploy.
        let mut ordinal_of: BTreeMap<Vec<u8>, u32> = BTreeMap::new();

        // User deploy indices (failed deploys are skipped).
        for (i, (d, merge_chs)) in usr_processed_deploys
            .iter()
            .zip(usr_mergeable.iter())
            .enumerate()
        {
            if d.is_failed {
                continue;
            }
            let ordinal = u32::try_from(i)
                .map_err(|_| "more deploys than a u32 ordinal can name".to_string())?;
            ordinal_of.insert(d.deploy.sig.clone(), ordinal);
            let event_log_index = Self::create_event_log_index(
                &d.deploy_log,
                history_repository,
                pre_state_hash,
                merge_chs.clone(),
            )
            .await?;
            deploy_indices.insert(DeployIndex {
                deploy_id: d.deploy.sig.clone(),
                cost: i64::try_from(d.cost.cost).map_err(|e| e.to_string())?,
                event_log_index,
            });
        }

        // **The cost-accounting moves, per deploy** (AUDIT C207). Three pure functions of the deploy
        // and the cost the block recorded for it — which is exactly why a merge can re-apply them
        // instead of a block's native sidecar carrying the values, and why only these numbers travel
        // on the chain.
        //
        // **A failed deploy is not skipped, and must not be.** `is_failed` does not say "the charge
        // never happened": a deploy that ran out of phlo *was* charged, refunded nothing and burned
        // it all, and its moves have to be composed like any other's — dropping them would destroy
        // the REV it burned. The case where the charge really did not happen — `pre_charge` found the
        // vault short — is the one that needs no special case: that deploy is recorded with
        // `cost: 0`, so its refund is the whole charge and all three moves below are exactly zero.
        let mut cost_moves: BTreeMap<Vec<u8>, CostMoves> = BTreeMap::new();
        for d in usr_processed_deploys {
            let Some(charge) = d.deploy.data.total_phlo_charge() else {
                continue;
            };
            // The vault the charge comes from and the refund goes to is the deployer's own address —
            // the same derivation `pre_charge` and `refund` use, so the merge and the block agree
            // about which leaf moves without either carrying the address.
            let Some(deployer) =
                RevAddress::from_public_key(&PublicKey::new(d.deploy.deployer.clone()))
                    .map(|a| a.to_base58())
            else {
                continue;
            };
            cost_moves.insert(
                d.deploy.sig.clone(),
                CostMoves {
                    deployer,
                    charge: i64::from(charge),
                    refund: i64::from(d.refund_amount()),
                    burned: i64::from(d.burned_amount()),
                },
            );
        }

        // System deploy indices (only `Succeeded` blocks contribute).
        for (i, (sd, merge_chs)) in sys_processed_deploys
            .iter()
            .zip(sys_mergeable.iter())
            .enumerate()
        {
            let sys_ordinal = u32::try_from(usr_count + i)
                .map_err(|_| "more deploys than a u32 ordinal can name".to_string())?;
            let (id, log) = match sd {
                ProcessedSystemDeploy::Succeeded {
                    event_list,
                    system_deploy: SystemDeployData::Slash { .. },
                } => (sys_deploy_id(&block_hash, 1), event_list),
                ProcessedSystemDeploy::Succeeded {
                    event_list,
                    system_deploy: SystemDeployData::CloseBlock,
                } => (sys_deploy_id(&block_hash, 2), event_list),
                ProcessedSystemDeploy::Succeeded {
                    event_list,
                    system_deploy: SystemDeployData::Empty,
                } => (sys_deploy_id(&block_hash, 3), event_list),
                // **Index 4 was `SystemDeployData::RecordSpoke`** (B4, #150), retired with the variant
                // it named. It is not reused: the index is a fold position, and a retired one staying
                // vacant is cheaper to read than a kind that silently inherited a number. A block from
                // before the retirement decodes its field 3 as `Empty` and takes the arm above, which
                // is why that block is refused at replay rather than merged under a stale id.
                ProcessedSystemDeploy::Failed { .. } => continue,
            };
            let event_log_index = Self::create_event_log_index(
                log,
                history_repository,
                pre_state_hash,
                merge_chs.clone(),
            )
            .await?;
            ordinal_of.insert(id.clone(), sys_ordinal);
            deploy_indices.insert(DeployIndex {
                deploy_id: id,
                cost: 0,
                event_log_index,
            });
        }

        // Deploys in a block execute sequentially, so there are only dependencies (no conflicts).
        let dependency_map = compute_dependency_map(&deploy_indices, &deploy_indices, |l, r| {
            depends(&l.event_log_index, &r.event_log_index)
        });
        let deploy_chains =
            compute_greedy_non_intersecting_branches(&deploy_indices, &dependency_map);

        let host_block = Blake2b256Hash::from_bytes(*block_hash.as_bytes());
        let mut chains = Vec::new();
        let mut placed: BTreeSet<u32> = BTreeSet::new();
        for chain in &deploy_chains {
            // Each chain carries the moves of **its own** deploys, so a rejected chain's cost
            // accounting is dropped with it rather than applied by another chain's acceptance.
            let chain_moves: BTreeMap<Vec<u8>, CostMoves> = chain
                .iter()
                .filter_map(|d| {
                    cost_moves
                        .get(&d.deploy_id)
                        .map(|m| (d.deploy_id.clone(), m.clone()))
                })
                .collect();
            // **And its own native writes** (#280). Every deploy of this chain contributes the
            // actions recorded under its ordinal, in that order, so a chain's native set is exactly
            // what its deploys wrote and nothing else.
            let mut native_effects: Vec<NativeStoreAction> = Vec::new();
            for d in chain {
                let Some(ordinal) = ordinal_of.get(&d.deploy_id) else {
                    continue;
                };
                placed.insert(*ordinal);
                native_effects.extend(effects.of_deploy(*ordinal).iter().cloned());
            }
            let first_deploy_ordinal = chain
                .iter()
                .filter_map(|d| ordinal_of.get(&d.deploy_id).copied())
                .min()
                .ok_or_else(|| "a deploy chain with no indexed deploy".to_string())?;
            chains.push(Arc::new(
                DeployChainIndex::apply(
                    host_block,
                    chain,
                    chain_moves,
                    executor.clone(),
                    pre_state_hash,
                    post_state_hash,
                    history_repository,
                    native_effects,
                    first_deploy_ordinal,
                )
                .await?,
            ));
        }

        // **Every attributed effect is placed, or the index is refused** (#280). "Attributed to a
        // deploy that has no chain" is not a state this build can carry: the alternative — filing it
        // under the block — is the representation the defect lived in. The case is expected
        // unreachable (a failed deploy's effects are reverted before the drain, and a failed deploy
        // gets no index), and it is an `Err` rather than a silent omission precisely because
        // "expected unreachable" is a claim and not a proof.
        let unplaced: Vec<u32> = effects.ordinals().filter(|o| !placed.contains(o)).collect();
        if !unplaced.is_empty() {
            return Err(format!(
                "the sidecar of block {} attributes native effects to deploy ordinal(s) {unplaced:?}, \
                 which carry no chain — they cannot be placed, and filing them under the block is the \
                 representation #280 removed",
                block_hash.to_hex()
            ));
        }

        Ok(BlockIndex {
            block_hash,
            deploy_chains: chains,
        })
    }
}

/// Build the system-deploy id as `blockHash ++ prefix` (port of `blockHash.concat(SYS_*_DEPLOY_ID)`).
fn sys_deploy_id(block_hash: &BlockHash, prefix: u8) -> Vec<u8> {
    let mut id = block_hash.as_bytes().to_vec();
    id.push(prefix);
    id
}

/// The in-memory block-index cache (port of `BlockIndex.cache`).
///
/// Entries are `Arc`-shared rather than owned. A `BlockIndex` holds a `Vec<DeployChainIndex>` and each
/// of those holds an `EventLogIndex` — sets of produces and consumes — so an entry is kilobytes to
/// megabytes, and this cache is read on *every* block validated and every block proposed: the merge
/// asks for each block of its conflict and final scopes, and a hit used to deep-copy the whole index.
/// Measured under a devnet fork storm (#117): ~750 requests over one stall with 711 of them hits, and
/// a heap profile put `DeployChainIndex::clone`/`Vec::clone` under this frame among the largest
/// allocating sites. Sharing is safe because an index is immutable once built.
/// Provisional entry-retention limit independent of finality. The 2026-09-29 restart logged 250
/// cached indices 30 seconds before a global OOM kill. That correlation does not establish the cache
/// as the cause, and the 43-chain merge scope in #117 does not measure the number of block indices.
/// Retaining at most 64 entries bounds this cache's references, not process RSS: individual indices
/// can be large and in-flight users hold their own `Arc`. Evicted indices are recomputed; the miss
/// cost and memory envelope still require measurement under the same controlled campaign.
const BLOCK_INDEX_CACHE_MAX_ENTRIES: usize = 64;

#[derive(Default)]
struct BlockIndexCache {
    entries: BTreeMap<BlockHash, Arc<BlockIndex>>,
    insertion_order: VecDeque<BlockHash>,
}

impl BlockIndexCache {
    fn get(&mut self, hash: &BlockHash) -> Option<Arc<BlockIndex>> {
        let index = self.entries.get(hash).map(Arc::clone)?;
        // Tiny (<=64) LRU queue: keep the blocks merges are actually reusing resident instead of
        // evicting a hot old entry merely because it was inserted before a cold one.
        self.insertion_order.retain(|queued| queued != hash);
        self.insertion_order.push_back(*hash);
        Some(index)
    }

    /// Insert one immutable index and evict the least-recently-used cached entries until the hard
    /// bound holds. Cache order is not consensus state, and unlike a finality-derived prune this
    /// bound keeps working while finality is stalled.
    fn insert(&mut self, hash: BlockHash, index: Arc<BlockIndex>) -> usize {
        self.entries.insert(hash, index);
        self.insertion_order.retain(|queued| queued != &hash);
        self.insertion_order.push_back(hash);

        let mut evicted = 0usize;
        while self.entries.len() > BLOCK_INDEX_CACHE_MAX_ENTRIES {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            if self.entries.remove(&oldest).is_some() {
                evicted += 1;
            }
        }
        evicted
    }

    fn remove_many(&mut self, hashes: &[BlockHash]) -> usize {
        let before = self.entries.len();
        for hash in hashes {
            self.entries.remove(hash);
        }
        if self.entries.len() != before {
            let entries = &self.entries;
            self.insertion_order
                .retain(|hash| entries.contains_key(hash));
        }
        before - self.entries.len()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

static BLOCK_INDEX_CACHE: OnceLock<Mutex<BlockIndexCache>> = OnceLock::new();

async fn get_block_unsafe(
    block_store: &BlockStore,
    hash: &BlockHash,
) -> Result<BlockMessage, String> {
    let mut vals = block_store.get(&[*hash]).await?;
    vals.pop()
        .flatten()
        .ok_or_else(|| format!("missing block {}", hash.to_hex()))
}

impl BlockIndex {
    /// Load (or compute + cache) a block index (port of `BlockIndex.getBlockIndex`).
    ///
    /// `fringe_state_hash` is the state hash of the last finalised fringe as of `block_hash`, and only
    /// the sidecar-regeneration path below uses it: that path *replays* the block, and a replay of a
    /// block that closes an epoch needs the value its close deploy anchored the next seed to. It is a
    /// parameter rather than a DAG read because the callers differ in what they have — validation has
    /// the value it just computed from the merge, and the node's indexing loops have the DAG.
    pub async fn get_block_index(
        runtime: &RuntimeManager,
        dag: &dyn BlockDagStorage,
        block_store: &BlockStore,
        block_hash: BlockHash,
        fringe_state_hash: Blake2b256Hash,
    ) -> Result<Arc<BlockIndex>, String> {
        INDEX_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cache = BLOCK_INDEX_CACHE.get_or_init(|| Mutex::new(BlockIndexCache::default()));
        if let Some(idx) = cache.lock().unpoison().get(&block_hash) {
            // A hit is a refcount bump, not a copy: see the cache's own comment.
            return Ok(idx);
        }

        let block = get_block_unsafe(block_store, &block_hash).await?;
        let sender = block.sender.as_bytes().to_vec();
        let pre_state_hash = Blake2b256Hash::from_byte_array(block.pre_state_hash.as_bytes());
        let post_state_hash = Blake2b256Hash::from_byte_array(block.post_state_hash.as_bytes());
        let mergeable_result = runtime
            .load_mergeable_channels(
                post_state_hash.as_bytes(),
                &sender,
                i64::from(block.seq_num),
            )
            .await;
        let native_recorded = runtime
            .load_native_changes(
                post_state_hash.as_bytes(),
                &sender,
                i64::from(block.seq_num),
            )
            .await?;
        // Both sidecars are needed: mergeable channels to rebuild the tuple-space effects, native
        // changes to re-apply the effects the merge cannot see (issue #74). If either is missing -
        // an LFS-restored or deep-replayed block, or a block indexed before the native sidecar
        // existed - replay the block once to reproduce both, and persist them so the next lookup is
        // a read. A store error on the mergeable read is still a store error.
        let (mergeable_chs, native_record) = match mergeable_result {
            Ok(channels) => match native_recorded {
                Some(native) => (channels, native),
                None => {
                    regenerate_sidecars(
                        runtime,
                        dag,
                        &block,
                        &sender,
                        pre_state_hash,
                        post_state_hash,
                        fringe_state_hash,
                    )
                    .await?
                }
            },
            Err(err) if err.starts_with("Mergeable store invalid state hash") => {
                regenerate_sidecars(
                    runtime,
                    dag,
                    &block,
                    &sender,
                    pre_state_hash,
                    post_state_hash,
                    fringe_state_hash,
                )
                .await?
            }
            Err(err) => return Err(err),
        };
        // **A record written before attribution existed is a re-index, not a value** (#280). Its
        // effects belong to the block as a whole, so attaching them to the deploys that made them is
        // impossible — reading it as though they could be attached is the representation this fix
        // removes. A *nonempty* legacy record is therefore regenerated (the replay writes an attributed
        // one) and counted, so the migration is visible rather than silent; an *empty* one is accepted,
        // because there is no effect to mis-attribute.
        let (mergeable_chs, native_effects) = match native_record {
            SidecarRecord::Attributed(effects) => (mergeable_chs, effects),
            SidecarRecord::LegacyUnattributed(actions) if actions.is_empty() => {
                (mergeable_chs, BlockNativeEffects::empty())
            }
            SidecarRecord::LegacyUnattributed(actions) => {
                LEGACY_SIDECARS_REGENERATED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let (channels, record) = regenerate_sidecars(
                    runtime,
                    dag,
                    &block,
                    &sender,
                    pre_state_hash,
                    post_state_hash,
                    fringe_state_hash,
                )
                .await?;
                match record {
                    SidecarRecord::Attributed(effects) => (channels, effects),
                    other => {
                        return Err(format!(
                            "the sidecar regenerated for block {} is still not attributed ({other:?}), \
                             and its {} action(s) cannot be placed",
                            block.block_hash.to_hex(),
                            actions.len()
                        ))
                    }
                }
            }
        };

        // The producer's share is paid to the **block's own signed sender** — `pay_executor` reads it
        // off the runtime's block data on both paths, so the index carries the same address rather
        // than letting a merge re-derive it from anything else.
        let executor =
            RevAddress::from_public_key(&PublicKey::new(block.sender.as_bytes().to_vec()))
                .map(|a| a.to_base58())
                .ok_or_else(|| {
                    format!(
                        "block {} has no derivable REV address for its sender",
                        block.block_hash.to_hex()
                    )
                })?;
        // **The attributed set goes to the index as it is** (#280): `BlockIndex::apply` places every
        // deploy's actions on the chain that carries it and refuses anything it cannot place, so the
        // block-level set the index used to hold is not a value that reaches the merge any more.
        let index = BlockIndex::apply(
            block.block_hash,
            &block.state.deploys,
            &block.state.system_deploys,
            pre_state_hash,
            post_state_hash,
            runtime.get_history_repo(),
            &mergeable_chs,
            native_effects,
            executor,
        )
        .await?;

        let shared = Arc::new(index);
        let (cache_len, capacity_evicted) = {
            let mut guard = cache.lock().unpoison();
            let capacity_evicted = guard.insert(block_hash, Arc::clone(&shared));
            (guard.len() as u64, capacity_evicted as u64)
        };
        if capacity_evicted > 0 {
            INDEX_CACHE_CAPACITY_EVICTED
                .fetch_add(capacity_evicted, std::sync::atomic::Ordering::Relaxed);
        }
        INDEX_CACHE_LEN.store(cache_len, std::sync::atomic::Ordering::Relaxed);
        Ok(shared)
    }

    /// Remove cached block indices for `hashes` (port of `BlockIndex.cache.remove` in `pruneDiff`).
    ///
    /// The cache is advisory: a pruned entry is simply recomputed on the next `get_block_index`, so
    /// pruning is always safe (it never affects correctness, only recomputation cost). This bounds
    /// the otherwise-unbounded in-memory cache as the fringe finalizes.
    pub fn prune_cache(hashes: &[BlockHash]) {
        let Some(cache) = BLOCK_INDEX_CACHE.get() else {
            return;
        };
        let mut guard = cache.lock().unpoison();
        let pruned = guard.remove_many(hashes);
        // The oracle logs this (`Pruned N merging indices, new size: M`) and the port did not, which is
        // part of why the cache's growth was invisible (#60).
        if pruned > 0 {
            INDEX_CACHE_PRUNED.fetch_add(pruned as u64, std::sync::atomic::Ordering::Relaxed);
        }
        INDEX_CACHE_LEN.store(guard.len() as u64, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Reproduce and persist a block's mergeable-channel **and** native-changes sidecars by replaying the
/// block from its own pre-state.
///
/// Called when either sidecar is missing: an LFS-restored or deep-replayed block, or one indexed
/// before the native sidecar existed. The replay is the same one validation runs, so it reproduces
/// both effects exactly; `replay_compute_state_with` persists them, and the native changes are read
/// back here for the index. The genesis (an empty block) has no deploys to replay, so both sidecars
/// are recorded empty - its native state is installed by `compute_genesis` outside the block, and the
/// genesis is never a merge parent.
async fn regenerate_sidecars(
    runtime: &RuntimeManager,
    dag: &dyn BlockDagStorage,
    block: &BlockMessage,
    sender: &[u8],
    pre_state_hash: Blake2b256Hash,
    post_state_hash: Blake2b256Hash,
    fringe_state_hash: Blake2b256Hash,
) -> Result<(Vec<NumberChannelsDiff>, SidecarRecord), String> {
    let seq_num = i64::from(block.seq_num);
    if block.justifications.is_empty()
        && block.state.deploys.is_empty()
        && block.state.system_deploys.is_empty()
    {
        // A failed *cache* write must not fail indexing: the caller only needs the (empty) channels,
        // and the worst case is recomputing this block's index next start.
        if runtime
            .save_mergeable_channels(post_state_hash, sender, seq_num, &[], pre_state_hash)
            .await
            .is_err()
            || runtime
                .save_native_changes(
                    post_state_hash,
                    sender,
                    seq_num,
                    &BlockNativeEffects::empty(),
                )
                .await
                .is_err()
        {
            INDEX_REPLAY_SAVE_FAILURES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        return Ok((
            Vec::new(),
            SidecarRecord::Attributed(BlockNativeEffects::empty()),
        ));
    }
    // The expensive path in #60: a full replay of the block from its own pre-state, taken whenever a
    // sidecar is absent (a block that arrived by LFS restore or was deep-replayed, rather than
    // proposed locally). Count and time it - a start-up that does this for every block used to
    // advertise nothing at all.
    INDEX_REPLAY_FALLBACKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let replay_started = std::time::Instant::now();
    // **The participation the epoch's absence rule reads** (B4, #150), derived on this arm only —
    // deriving it on every index lookup would put a fringe walk on the restart's hot path for a value
    // only a fallback replay consumes. It is the *same* derivation the merge that validated this block
    // performed (`participation_for_block`), and it has to be: the post-state comparison below refuses
    // a replay that computes anything else.
    let participation = crate::multi_parent_casper::participation_for_block(dag, block).await?;
    let forked = runtime.fork_replay_runtime(pre_state_hash).await?;
    let rand = BlockRandomSeed::random_generator_from_block(block).map_err(|e| e.to_string())?;
    let with_cost_accounting = !block.justifications.is_empty();
    let (computed, channels) = runtime
        .replay_compute_state_with(
            &forked,
            &pre_state_hash,
            &block.state.deploys,
            &block.state.system_deploys,
            &rand,
            BlockData::from_block(block),
            &fringe_state_hash,
            &participation,
            with_cost_accounting,
            // Genesis PoS descriptors; consumed only on the genesis replay path
            // (`is_genesis_pre_state`). See `interpreter_util.rs`.
            runtime.genesis_pos(),
            // The genesis vault balances, and only for the genesis. The claim that stood here —
            // "this is always non-genesis block replay" — is what AUDIT C46 falsified on a
            // 3-validator devnet: the genesis is what the finalized fringe points at when a validator
            // joins, and that validator is the one whose indexing takes this branch. Its replay
            // computes a different post-state without the balances (they are installed natively,
            // outside the block's deploys) and then refuses block #0 forever.
            if is_genesis_pre_state(&pre_state_hash) {
                runtime.genesis_vaults()
            } else {
                &[]
            },
        )
        .await
        .map_err(|e| {
            format!(
                "failed to regenerate mergeable channels for block {}: {e:?}",
                block.block_hash.to_hex()
            )
        })?;
    if computed != post_state_hash {
        return Err(format!(
            "regenerated mergeable channels for block {} but replay computed {} instead of {}",
            block.block_hash.to_hex(),
            computed.to_hex(),
            post_state_hash.to_hex()
        ));
    }
    // `replay_compute_state_with` already persisted both sidecars; read the native changes back for
    // the index the caller is building.
    // The sidecar's half — the block's own writes; cost accounting is re-derived by the merge (AUDIT
    // C207), so a regenerated sidecar must omit it exactly as a played one does, or the two would
    // disagree about what a block's effects are.
    // **The sidecar's half, now attributed** (#280): `from_drain` refuses a write with no deploy to
    // key it by, so a replay that produced one reports it rather than filing it under the block.
    let native_effects = BlockNativeEffects::from_drain(&forked.last_native_drain())?;
    INDEX_REPLAY_MILLIS.fetch_add(
        replay_started.elapsed().as_millis() as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
    Ok((channels, SidecarRecord::Attributed(native_effects)))
}

/// **The cost-accounting moves of the accepted deploys, composed into absolute vault writes** (AUDIT
/// C207).
///
/// `pre_charge`, `refund` and `pay_executor` are pure balance movements — a debit from one vault, a
/// credit to another, summing to zero per deploy — so unlike a block's other native writes they
/// *compose*: two accepted deploys' charges add, where two blocks' absolute snapshots of one slot
/// could only be resolved by rejecting one of them. That is what lets the merge apply them after the
/// fact rather than a block's sidecar carrying `pos:vault` — which is what made every user-deploy
/// block overlap every concurrent sibling on that one slot, and, by the whole-block rejection rule,
/// silently discard the deploy's own native writes with it.
///
/// **Where a chain wrote one of these slots itself** — a `bond` debits the deployer's own vault, a
/// boundary credits `pos:vault` — the fold already holds that chain's absolute value, and that value
/// was computed *after* the chain's own cost accounting. Its own moves are therefore already inside
/// it, and so are the moves of every block it had seen; only the moves of blocks the fold's winner
/// did **not** see are added here. That is exactly the composition a sequential execution reaches,
/// and it is why the winner is needed and not only the winning value: a deeper block's snapshot
/// contains its ancestors' charges, a concurrent one's does not.
///
/// `changes` is the fold's absolute writes and is **edited in place**: a slot this composes onto is
/// replaced by the composed value, so the batch still holds one action per slot, which
/// `RadixHistory::process` requires.
async fn compose_cost_accounting(
    history_repository: &RhoHistoryRepository,
    base_state: Blake2b256Hash,
    accepted: &BTreeSet<Arc<DeployChainIndex>>,
    seen: impl Fn(&Blake2b256Hash, &Blake2b256Hash) -> bool,
    winners: &BTreeMap<(u8, Blake2b256Hash), Blake2b256Hash>,
    changes: &mut BTreeMap<(u8, Blake2b256Hash), NativeStoreAction>,
) -> Result<(), String> {
    let reader = history_repository.get_native_reader(base_state).await;
    let store: Arc<InMemNativeStore> = Arc::new(InMemNativeStore::new(reader));
    // The producer's share travels in consensus state (`PosParams`), so the merge reads it where
    // `pay_executor` reads it rather than carrying a copy on the chain.
    let share = i64::from(
        NativeSystemState::new(Arc::clone(&store))
            .params()
            .await?
            .executor_share,
    );

    // The moves are accumulated per slot first, because the *skip* decision is per (chain, slot):
    // two chains can contribute to one slot with only one of them seen by the fold's winner.
    let mut deltas: BTreeMap<(u8, Blake2b256Hash), (Option<String>, i64)> = BTreeMap::new();
    for chain in accepted {
        let host = chain.host_block;
        for moves in chain.cost_moves.values() {
            // `pay_executor`'s own arithmetic, kept identical to it: the product of two `i64`s does
            // not fit an `i64`, and a wrapped product would pay a different amount than the share
            // names, silently, on a consensus path.
            let payment = i128::from(moves.burned) * i128::from(share) / 10000;
            let Some(payment) = i64::try_from(payment).ok() else {
                return Err(format!("executor payment {} overflows i64", moves.burned));
            };
            let entries = [
                // The deployer pays the charge and is refunded what it did not burn.
                (
                    (
                        rchain_rspace::native_store::PREFIX_VAULT,
                        vault_key(&moves.deployer),
                    ),
                    Some(moves.deployer.clone()),
                    -moves.charge + moves.refund,
                ),
                // The staking vault keeps what was burned, minus the producer's share of it.
                (
                    (rchain_rspace::native_store::PREFIX_POS, pos_vault_key()),
                    None,
                    moves.charge - moves.refund - payment,
                ),
                // The block's own signed sender is paid for producing it.
                (
                    (
                        rchain_rspace::native_store::PREFIX_VAULT,
                        vault_key(&chain.executor),
                    ),
                    Some(chain.executor.clone()),
                    payment,
                ),
            ];
            for (slot, address, delta) in entries {
                if delta == 0 {
                    continue;
                }
                if let Some(winner) = winners.get(&slot) {
                    if *winner == host || seen(winner, &host) {
                        continue;
                    }
                }
                let entry = deltas.entry(slot).or_insert_with(|| (address, 0));
                entry.1 = entry
                    .1
                    .checked_add(delta)
                    .ok_or_else(|| format!("cost-accounting delta overflow on {slot:?}"))?;
            }
        }
    }

    for (slot, (address, delta)) in deltas {
        // What this slot's value is composed onto: the fold's absolute write when the batch already
        // holds one, otherwise the balance the base state holds.
        let current = match changes.get(&slot) {
            Some(NativeStoreAction::Put { value, .. }) => decode_balance(value)?,
            Some(NativeStoreAction::Delete { .. }) => 0,
            None => match &address {
                Some(a) => match store
                    .get(rchain_rspace::native_store::PREFIX_VAULT, &vault_key(a))
                    .await?
                {
                    Some(bytes) => decode_balance(&bytes)?,
                    None => 0,
                },
                None => match store
                    .get(rchain_rspace::native_store::PREFIX_POS, &pos_vault_key())
                    .await?
                {
                    Some(bytes) => decode_balance(&bytes)?,
                    None => 0,
                },
            },
        };
        // **`pre_charge`'s guard, applied to the state the merge is building.** A deploy's charge
        // was affordable where that deploy ran, but a *concurrent* deploy by the same payer is
        // invisible to it, and two of them can together exceed the vault. A balance cannot go
        // negative, so the merge refuses rather than inventing one: this is the fail-closed branch,
        // and it is reachable only when two accepted blocks each spent the same payer's REV without
        // seeing each other.
        let composed = current
            .checked_add(delta)
            .ok_or_else(|| format!("cost-accounting composition overflow on {slot:?}"))?;
        if composed < 0 {
            return Err(format!(
                "cost accounting would take a vault to {composed}: the accepted deploys spend more \
                 than the merged state holds, and no sequential execution reaches this"
            ));
        }
        let value = NonNegI64::try_from(composed)
            .map_err(|e| format!("cost-accounting result is not a balance: {e}"))?;
        let action = match &address {
            Some(a) => vault_put_action(a, value),
            None => pos_vault_put_action(value),
        };
        changes.insert(slot, action);
    }
    Ok(())
}

/// A native balance leaf's payload as the `i64` it was written from (`write_balance`'s format).
fn decode_balance(bytes: &[u8]) -> Result<i64, String> {
    let arr: [u8; 8] = bytes.try_into().map_err(|_| {
        format!(
            "native balance leaf is {} bytes, expected the 8 of an i64",
            bytes.len()
        )
    })?;
    Ok(i64::from_le_bytes(arr))
}

/// **Which final-scope blocks were already rejected, by block — the merge's rejection lookup.**
///
/// The value for a final-scope block `h` is the union of the `rejected_deploys` of the blocks that
/// **finalised the fringe `h` belongs to**. That is what the retired `fringe-data` record held: the
/// record was keyed by `fringe_hash_of(fringe)` and its `rejected_deploys` was the join — a union —
/// of the `rejected_deploys` of the blocks that wrote it, i.e. the blocks `X` with `X.fringe` equal
/// to that key's fringe. A block absent from the map is "not rejected" (`merge`'s own reading of an
/// absent entry), and the map is keyed by `block_hash` alone, so the lookup is one `BTreeMap` probe.
///
/// **`rejections_for` used to build this map by walking the whole `fringe_states` map** — every
/// fringe in the DAG, cloning each `rejected_deploys` set into the answer, on every merge, for a
/// lookup that then touched at most the final scope's blocks (AUDIT C56's owed paragraph). It cannot
/// any more: the store retired (Law 66/68; C250's residue, C270) and the per-block derivation needs
/// the DAG's `child_map`, each block's `fringe`, and each block's `rejected_deploys` — none of which
/// `merge` holds. So the derivation is done by the **caller**, which has the DAG
/// (`multi_parent_casper.rs::final_scope_rejections`), and this function's job — indexing the answer
/// for the final scope, borrowed, with no clones — is subsumed by the caller's map builder. What
/// remains here is the *shape* of the map and the one probe `merge` makes:
///
/// ```text
/// // how the caller derives it, per final-scope block h:
/// //   F  = the fringe h belongs to      (h.member_of_fringe names it)
/// //   Xs = the blocks that finalised F   (children of F's members whose own fringe == F)
/// //   h  -> union over X in Xs of X.rejected_deploys
/// ```
///
/// This is simpler than the record: the record held a *derived value* keyed by the fringe, and the
/// ambiguity Law 66 describes was one key with many bases. Here every value is a per-block fact and
/// the key is the block, so a disagreement is two visible claims rather than one rewritten value —
/// and `merge` no longer needs to know which fringe a block was in.
pub type RejectionsMap = BTreeMap<BlockHash, BTreeSet<Vec<u8>>>;

/// **The per-block map for one fringe, folded from the finalisers' own declared rejects.**
///
/// `members` are the fringe's blocks (the merge's final scope); `finalisers` are the
/// `rejected_deploys` sets of the blocks whose own `fringe` is this fringe — the blocks that
/// *finalised* it. Every member carries the **union**, which is the record the retired store held
/// (its value was joined across writers by union) and the answer the merge reads.
///
/// **Why a union and not a choice.** The old store's value was decided by *arrival order* when two
/// finalisers disagreed (last write, then a `min` tie-break) — the ambiguity Law 66 names, and the
/// C215/C270 stall. A union is commutative, associative and idempotent, so every node holding these
/// blocks derives the same value regardless of the order it saw them in, and the direction is
/// fail-closed: a merge can only ever reject *more* of what some finaliser rejected. The finalisers'
/// own claims stay distinct on the blocks themselves (Law 68: a block is written once), so a
/// disagreement is still *visible* to the reader that wants it
/// (`multi_parent_casper.rs::get_pre_state_for_parents` logs it), rather than being folded away.
pub fn rejections_of_fringe(
    members: &BTreeSet<BlockHash>,
    finalisers: impl IntoIterator<Item = BTreeSet<Vec<u8>>>,
) -> RejectionsMap {
    let union: BTreeSet<Vec<u8>> = finalisers.into_iter().flatten().collect();
    members.iter().map(|m| (*m, union.clone())).collect()
}

/// **How native writes relate the deploy chains of a merge — on the chain, not on the block** (#280).
///
/// A native write is an **absolute value** computed from its block's own pre-state, not an effect
/// that composes: an epoch boundary's `close_block` writes the whole bond pool, a phlo charge writes
/// the staking vault's new balance. So two chains writing one slot can be merged only when one has
/// seen the other, and then the descendant's value is the merged one. Two *concurrent* writers of a
/// slot cannot both be kept — concatenating them hands radix history the same key twice — and keeping
/// either value silently discards the other's transition, which is why equal values are not a licence
/// to merge: the relation is on slots, not on values.
///
/// What changed, and why it had to. The relation used to read the **host block's** slot set — the
/// union of every chain that rode in on it — so at an epoch boundary, where every block runs
/// `close_block` and every block's union holds the epoch's slots, **every** pair of concurrent blocks
/// conflicted, including the pairs whose user chain wrote no contended slot at all. The rejection was
/// then closed over the whole block (`reject_whole_blocks`), so those chains died with their host: the
/// live capture is `spec/audit/evidence/n280-merge-loses-a-write-results.md`, where a user deploy's
/// write is present at heights 100, 101 and 102 and gone from 103 on every node at once.
///
/// Now the relation reads the chain's **own** slots, and a chain that wrote nothing contended is not
/// a conflict partner at all. `reject_whole_blocks` is deleted rather than narrowed: there is nothing
/// left for it to close over, because the operation it performed — reject every chain of a block
/// because of a write the block made — is no longer expressible.
///
/// - **conflict**: two chains of different blocks that wrote a common slot, neither having seen the
///   other. Resolution then rejects one side, as for a tuple-space conflict.
/// - **dependency**: a chain whose slot a block it has *seen* also wrote depends on that block's
///   chain, so rejecting the ancestor rejects the value built on it — **and a chain of one block
///   depends on its own block's earlier chains**, because a later chain's absolute value was computed
///   on top of theirs. That second arm is new with the chain-level relation: the whole-block rule
///   never applied two chains of one block separately, so it never needed it.
struct NativeRelations<'a> {
    ancestry: &'a BTreeMap<BlockHash, BTreeSet<BlockHash>>,
}

impl NativeRelations<'_> {
    fn sees(&self, a: &Blake2b256Hash, b: &Blake2b256Hash) -> bool {
        self.ancestry
            .get(&BlockHash::new(*a.as_bytes()))
            .is_some_and(|seen| seen.contains(&BlockHash::new(*b.as_bytes())))
    }

    /// Whether two chains wrote a common slot — the whole of the native overlap, and per chain.
    fn overlap(&self, a: &DeployChainIndex, b: &DeployChainIndex) -> bool {
        !a.native_slots().is_disjoint(&b.native_slots())
    }

    fn conflicting(&self, a: &DeployChainIndex, b: &DeployChainIndex) -> bool {
        let (ha, hb) = (&a.host_block, &b.host_block);
        ha != hb && self.overlap(a, b) && !self.sees(ha, hb) && !self.sees(hb, ha)
    }

    /// Whether `a` depends on `b` — see the type's comment for both arms.
    fn depends(&self, a: &DeployChainIndex, b: &DeployChainIndex) -> bool {
        if a.host_block == b.host_block {
            // One block's own chains: the deploy order decided it, and a later chain's value already
            // includes every earlier chain's write to a shared slot.
            return a.first_deploy_ordinal > b.first_deploy_ordinal && self.overlap(a, b);
        }
        self.overlap(a, b) && self.sees(&a.host_block, &b.host_block)
    }

    /// The accepted chains' native writes as one action per slot.
    ///
    /// Writers are applied in ancestry order — a descendant's value replaces its ancestor's — and a
    /// block's own chains in **deploy order**, for the reason the dependency arm gives. The
    /// `seen_count` key puts every ancestor before its descendants (a block has seen strictly more
    /// in-scope blocks than any ancestor of it); the hash and the ordinal only make the order total.
    ///
    /// Two accepted writers of a slot that are neither in that order nor seen would mean resolution
    /// kept a conflict, and that is refused rather than decided by iteration order. After the
    /// chain-level relation that refusal is unreachable on any state resolution can produce —
    /// `the_resolution_leaves_the_fold_nothing_to_refuse` in `spec/Rchain/Merging.lean` — so it is a
    /// fail-closed backstop rather than a judgement call.
    ///
    /// **The writer is returned with its action** (AUDIT C207), because the merge's cost-accounting
    /// pass needs to know *which* block's absolute value landed on a slot: a value computed after that
    /// block's own cost accounting already contains it, and the moves of every block that block had
    /// seen are in it too. The map's key is the slot; the tuple is `(host, action)` and the host is
    /// the **last writer in that order**, i.e. the deepest one — the value that survives.
    #[allow(clippy::type_complexity)]
    fn fold(
        &self,
        accepted: &BTreeSet<Arc<DeployChainIndex>>,
    ) -> Result<BTreeMap<(u8, Blake2b256Hash), (Blake2b256Hash, NativeStoreAction)>, String> {
        let seen_count = |h: &Blake2b256Hash| {
            self.ancestry
                .get(&BlockHash::new(*h.as_bytes()))
                .map_or(0, BTreeSet::len)
        };
        let mut writers: Vec<&Arc<DeployChainIndex>> = accepted
            .iter()
            .filter(|c| !c.native_effects.is_empty())
            .collect();
        writers.sort_by_key(|c| {
            (
                seen_count(&c.host_block),
                c.host_block,
                c.first_deploy_ordinal,
            )
        });

        let mut by_slot: BTreeMap<(u8, Blake2b256Hash), (Blake2b256Hash, NativeStoreAction)> =
            BTreeMap::new();
        for chain in writers {
            for action in &chain.native_effects {
                let slot = action.slot();
                if let Some((previous, _)) = by_slot.get(&slot) {
                    // A writer of the *same* block is ordered by the sort above (deploy order); a
                    // writer of another block must have been seen, or resolution kept a conflict.
                    if *previous != chain.host_block && !self.sees(&chain.host_block, previous) {
                        return Err(format!(
                            "blocks {} and {} both write native slot {:02x}/{} and neither has seen \
                             the other; conflict resolution must reject one",
                            previous.to_hex(),
                            chain.host_block.to_hex(),
                            slot.0,
                            slot.1.to_hex()
                        ));
                    }
                }
                by_slot.insert(slot, (chain.host_block, action.clone()));
            }
        }
        Ok(by_slot)
    }
}

/// The scope of a merge: final (immutable) and conflict (alterable) blocks (port of `MergeScope`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeScope {
    pub final_scope: BTreeSet<BlockHash>,
    pub conflict_scope: BTreeSet<BlockHash>,
    /// For each block of either scope, the blocks of either scope it has *seen* (its in-scope
    /// ancestors, itself excluded). Not in the Scala: native writes are absolute values rather than
    /// event logs, so whether two blocks writing one native key are a conflict or a dependency is a
    /// question about the DAG, not about their effects (issue #83). A block absent from the map has
    /// seen nothing in scope, i.e. every pair is read as concurrent.
    pub ancestry: BTreeMap<BlockHash, BTreeSet<BlockHash>>,
}

impl MergeScope {
    /// Create a merge scope from the DAG (port of `MergeScope.fromDag`).
    pub fn from_dag(
        merge_fringe: &BTreeSet<BlockHash>,
        final_fringe: &BTreeSet<BlockHash>,
        child_map: &BTreeMap<BlockHash, BTreeSet<BlockHash>>,
        dag_data: &BTreeMap<BlockHash, Message<BlockHash, Validator>>,
    ) -> Result<(MergeScope, Option<BlockHash>), String> {
        // A block's merged state is a function of the ancestry of its declared
        // parents, not of later siblings that happened to arrive at this node.
        // prune_fringe considers children of the finalized frontier, including
        // those outside this merge's causal past if given the global child map.
        // A post-partition node may hold more of these children than the author,
        // changing the final scope and recomputed pre-state for fixed parents.
        //
        // Restrict only the children used for pruning; keep the full message map
        // for computing scope ancestry. Membership checks avoid cloning every
        // parent's accumulated seen set.
        let mut causal_children = BTreeMap::new();
        for member in final_fringe {
            if let Some(children) = child_map.get(member) {
                let in_past: BTreeSet<BlockHash> = children
                    .iter()
                    .filter(|child| {
                        merge_fringe.iter().any(|tip| {
                            tip == *child
                                || dag_data
                                    .get(tip)
                                    .is_some_and(|message| message.seen.contains(*child))
                        })
                    })
                    .copied()
                    .collect();
                causal_children.insert(*member, in_past);
            }
        }
        let prune_fringe: BTreeSet<BlockHash> =
            message_map::prune_fringe(dag_data, final_fringe, &causal_children)
                .iter()
                .map(|m| m.id)
                .collect();
        MergeScope::from_fringes(merge_fringe, final_fringe, &prune_fringe, dag_data)
    }

    /// Create a merge scope from explicit fringes (port of `MergeScope.fromFringes`).
    pub fn from_fringes(
        merge_fringe: &BTreeSet<BlockHash>,
        final_fringe: &BTreeSet<BlockHash>,
        prune_fringe: &BTreeSet<BlockHash>,
        dag_data: &BTreeMap<BlockHash, Message<BlockHash, Validator>>,
    ) -> Result<(MergeScope, Option<BlockHash>), String> {
        // Every fringe is checked against the DAG before use, as before — but the scope itself is
        // computed over ids, so no message is copied. A `Message` carries its `seen` set (Θ(N) for
        // a chain of N blocks), and cloning the operands and the result per block was Θ(N²) in
        // copies (AUDIT C56).
        let require_in_dag = |fringe: &BTreeSet<BlockHash>, what: &str| -> Result<(), String> {
            match fringe.iter().find(|h| !dag_data.contains_key(h)) {
                Some(h) => Err(format!("{what} not in dag: {}", h.to_hex())),
                None => Ok(()),
            }
        };
        require_in_dag(merge_fringe, "merge fringe")?;
        require_in_dag(final_fringe, "final fringe")?;
        require_in_dag(prune_fringe, "prune fringe")?;

        let c_scope_ids = message_map::between(dag_data, merge_fringe, final_fringe);
        let f_scope_ids = message_map::between(dag_data, final_fringe, prune_fringe);

        let base_msg = if f_scope_ids.is_empty() {
            let genesis = message_map::find_with_empty_parents(dag_data)
                .ok_or_else(|| "Final scope is empty but no genesis found.".to_string())?;
            Some(genesis.id)
        } else {
            None
        };
        let conflict_scope: BTreeSet<BlockHash> = c_scope_ids
            .difference(&base_msg.into_iter().collect())
            .copied()
            .collect();

        // Restricted to the scope, so the map is as small as the merge rather than the chain.
        let in_scope: BTreeSet<BlockHash> = conflict_scope.union(&f_scope_ids).copied().collect();
        let ancestry: BTreeMap<BlockHash, BTreeSet<BlockHash>> = in_scope
            .iter()
            .filter_map(|id| {
                let seen: BTreeSet<BlockHash> = dag_data
                    .get(id)?
                    .seen
                    .iter()
                    .filter(|s| *s != id && in_scope.contains(s))
                    .copied()
                    .collect();
                (!seen.is_empty()).then_some((*id, seen))
            })
            .collect();

        Ok((
            MergeScope {
                final_scope: f_scope_ids,
                conflict_scope,
                ancestry,
            },
            base_msg,
        ))
    }

    /// Merge the conflict scope into the base state, returning the new state hash + rejected
    /// deploy ids (port of `MergeScope.merge`).
    pub async fn merge<F, Fut>(
        merge_scope: &MergeScope,
        base_state: Blake2b256Hash,
        rejections: &RejectionsMap,
        history_repository: &RhoHistoryRepository,
        block_index: &F,
        rejection_cost: impl Fn(&DeployChainIndex) -> i64,
    ) -> Result<MergeOutcome, String>
    where
        F: Fn(BlockHash) -> Fut,
        Fut: std::future::Future<Output = Result<Arc<BlockIndex>, String>>,
    {
        let conflict_indices: Vec<Arc<BlockIndex>> = {
            let mut v = Vec::new();
            for h in &merge_scope.conflict_scope {
                v.push(block_index(*h).await?);
            }
            v
        };
        let final_indices: Vec<Arc<BlockIndex>> = {
            let mut v = Vec::new();
            for h in &merge_scope.final_scope {
                v.push(block_index(*h).await?);
            }
            v
        };

        // **No block-level native index any more** (#280). The relation reads each chain's own slots
        // (`DeployChainIndex::native_effects`), so there is nothing here to key by host: a chain that
        // wrote nothing contended cannot be a conflict partner, and the per-block maps that used to
        // make it one — and to make its rejection total — are gone with the rule they served. The
        // final scope's chains still take part in the relation, through `final_set` below, which is
        // what keeps a conflict-scope writer of a finalised writer's slot incompatible.
        let conflict_set: BTreeSet<Arc<DeployChainIndex>> = conflict_indices
            .iter()
            .flat_map(|b| b.deploy_chains.iter().cloned())
            .collect();
        let final_set: BTreeSet<Arc<DeployChainIndex>> = final_indices
            .iter()
            .flat_map(|b| b.deploy_chains.iter().cloned())
            .collect();

        // Finalization decisions made in the final set — see [`RejectionsMap`]. The caller derived
        // this from per-block facts (`multi_parent_casper.rs::final_scope_rejections`); `merge` only
        // reads it, and `rejections.get` treats absence as "not rejected", so a block the caller did
        // not derive has no rejections rather than an error.
        let mut rejected_finally: BTreeSet<Arc<DeployChainIndex>> = BTreeSet::new();
        let mut accepted_finally: BTreeSet<Arc<DeployChainIndex>> = BTreeSet::new();
        for b in &final_indices {
            let rejected = rejections.get(&b.block_hash);
            for chain in &b.deploy_chains {
                // Borrowed rather than cloned: only used for a set lookup below.
                let first_id = chain.deploys_with_cost.iter().next().map(|d| &d.id);
                let is_rejected = match (rejected, first_id) {
                    (Some(rej), Some(id)) => rej.contains(id),
                    _ => false,
                };
                if is_rejected {
                    rejected_finally.insert(chain.clone());
                } else {
                    accepted_finally.insert(chain.clone());
                }
            }
        }

        // Mergeable channels.
        // A view, not a copy: each diff is owned by the chain it describes, and those live in
        // `conflict_set` for the whole merge, so the map only needs to point at them (#117 — this
        // copied one diff map per chain per merge).
        let mergeable_diffs_map: BTreeMap<Arc<DeployChainIndex>, &NumberChannelsDiff> =
            conflict_set
                .iter()
                .map(|b| (Arc::clone(b), &b.event_log_index.number_channels_data))
                .collect();
        let mut all_channel_hashes: BTreeSet<Blake2b256Hash> = BTreeSet::new();
        for diffs in mergeable_diffs_map.values() {
            all_channel_hashes.extend(diffs.keys().copied());
        }
        let init_mergeable_values =
            read_mergeable_values(history_repository, base_state, &all_channel_hashes).await?;

        // A chain relation is its event-log relation, widened by the chain's **own** native writes
        // (issue #83, #280): see `NativeRelations`.
        let native = NativeRelations {
            ancestry: &merge_scope.ancestry,
        };
        let (conflicts_map, dependency_map) = compute_relation_map_for_merge_set(
            &conflict_set,
            &final_set,
            |a, b| DeployChainIndex::deploys_are_conflicting(a, b) || native.conflicting(a, b),
            |a, b| DeployChainIndex::depends(a, b) || native.depends(a, b),
        );

        let ((to_merge, rejected), search) = resolve_conflict_set_with_census(
            &conflict_set,
            &accepted_finally,
            &rejected_finally,
            // The scope sets hold `Arc<DeployChainIndex>`, so the cost closure that the SDK's generic
            // `D` sees takes the shared handle; `rejection_cost` stays a plain `Fn(&DeployChainIndex)`
            // for every caller, and the deref coercion bridges the two here rather than at each call
            // site.
            |c: &Arc<DeployChainIndex>| rejection_cost(c),
            &conflicts_map,
            &dependency_map,
            &mergeable_diffs_map,
            &init_mergeable_values,
            // **The bound, and where its refusal goes.** `SearchBudget::NODE` is node-local policy, not
            // a protocol constant: an exceeded search returns **no answer**, this refuses the merge, and
            // the `Err` travels as `String` to `ValidateError::Internal` — a *drop*, which is the same
            // class as a missing dependency. It must never become a `ValidationFailed`: a budget is this
            // node's own resource policy, and marking the *block* for it would estrange the node from a
            // proposer for a local problem (C173). The restoring rule C173's fix added is keyed on a
            // view-dependent `Divergence` and does not cover a local budget — so the drop stays a drop,
            // and every node that does run the search inside its budget gets the identical answer.
            SearchBudget::NODE,
        )
        .map_err(|e| {
            format!(
                "merge search exceeded its budget ({} steps, {} options, budget {} steps / {} options):                  refused locally rather than answered from a partial enumeration",
                e.steps, e.options, e.budget.max_steps, e.budget.max_options
            )
        })?;
        // #117's instrument: what this merge handed the search. See `search_census`.
        search_census::record(&search);
        // **`reject_whole_blocks` used to run here**, closing resolution over whole native-writing
        // blocks. It is deleted rather than narrowed (#280): the relation is on the chain, so a chain
        // is rejected for a slot it wrote or not at all, and "reject every chain of a block because of
        // a write the block made" is no longer an expressible operation. Its second loop — the cascade
        // to blocks that depend on a rejected one — is already implied by the resolution's own
        // `with_dependencies` closure.

        // The native effects of the chains that survive resolution, one action per slot. A chain that
        // wrote nothing contributes nothing, and — the point of the fix — cannot be dropped for a
        // write it did not make. Two accepted writers of one slot are ancestor and descendant, or
        // chains of one block in deploy order, and the last write of a slot wins; see
        // `NativeRelations::fold`.
        //
        // This replaced the first #83 fix (b5e024d), which kept one write per slot by taking the last
        // accepted host in hash order: that restored liveness but decided a disagreement by hash, and
        // two concurrent blocks' absolute values are two transitions, so keeping either discards the
        // other's.
        let folded = native.fold(&to_merge)?;
        let winners: BTreeMap<(u8, Blake2b256Hash), Blake2b256Hash> = folded
            .iter()
            .map(|(slot, (host, _))| (*slot, *host))
            .collect();
        let mut native_changes: BTreeMap<(u8, Blake2b256Hash), NativeStoreAction> = folded
            .into_values()
            .map(|(_, action)| (action.slot(), action))
            .collect();
        // **And cost accounting, which no longer travels in a block's sidecar.** Applied per
        // accepted chain rather than per block, which is the granularity the tuple space already
        // merges its effects at — and the reason a user deploy's own native writes no longer die
        // with a whole-block rejection (AUDIT C207).
        compose_cost_accounting(
            history_repository,
            base_state,
            &to_merge,
            |a, b| native.sees(a, b),
            &winners,
            &mut native_changes,
        )
        .await?;
        let native_changes: Vec<NativeStoreAction> = native_changes.into_values().collect();

        let new_state = MergeScope::compute_merged_state(
            &to_merge,
            base_state,
            history_repository,
            &native_changes,
        )
        .await?;
        let rejected_ids: BTreeSet<Vec<u8>> = rejected
            .iter()
            .flat_map(|d| d.deploys_with_cost.iter().map(|x| x.id.clone()))
            .collect();
        // **What this merge did, for the caller that holds a logger** (#280). Computed here rather
        // than logged here: `merge` has no log handle by design, and the two invariants are worth
        // stating where the maps that decide them are still in hand.
        let report = MergeReport {
            conflict_chains: conflict_set.len(),
            kept_chains: to_merge.len(),
            rejected_chains: rejected.len(),
            // The other scope's ledger (C260): chains of *finalised* blocks whose derived rejection set
            // names them. Computed in the loop over `final_indices` above, and until this field existed
            // the set was handed to `resolve_conflict_set_with_census` and reported by nothing.
            final_scope_rejected_chains: rejected_finally.len(),
            dropped_native_slots: rejected
                .iter()
                .flat_map(|c| {
                    c.native_effects
                        .iter()
                        .map(|a| (c.host_block, a.slot()))
                        .collect::<Vec<_>>()
                })
                .collect(),
            rejected_cost_accounted_chains: rejected
                .iter()
                .filter(|c| !c.cost_moves.is_empty())
                .count(),
            // I1: a rejected chain must have a reason — a conflict with a kept chain, or a dependency
            // on another rejected one. Include finalized decisions as reasons: a conflict-scope
            // chain forced out by those decisions is not an unexplained rejection.
            rejections_without_a_conflict: rejected
                .iter()
                .filter(|c| conflict_set.contains(*c))
                .filter(|c| {
                    !rejection_has_a_reason(
                        *c,
                        &to_merge,
                        &rejected,
                        &conflicts_map,
                        &dependency_map,
                        &accepted_finally,
                        &rejected_finally,
                    )
                })
                .flat_map(|c| c.deploys_with_cost.iter().map(|d| d.id.clone()))
                .collect(),
            // I2: every slot a kept chain wrote is in the batch. `fold` builds the batch from exactly
            // those chains' actions, so a violation means the fold dropped one — which is the injury
            // this whole change is about, and why it is checked rather than assumed.
            unapplied_kept_writes: to_merge
                .iter()
                .flat_map(|c| c.native_slots())
                .filter(|slot| !native_changes.iter().any(|a| a.slot() == *slot))
                .collect(),
            native_writer_of_slot: winners.clone(),
        };
        Ok(MergeOutcome {
            state: new_state,
            rejected_deploys: rejected_ids,
            report,
        })
    }

    /// Merge a set of deploy chains into the base state and produce the new state hash (port of
    /// `MergeScope.computeMergedState`).
    ///
    /// `native_changes` are the native state mutations of the blocks contributing to `to_merge`.
    /// They are folded into the same checkpoint as the tuple-space changes: the `StateChange`s
    /// reconstruct only tuple space, so without them a merge would revert every native write the
    /// merged branches made (issue #74).
    pub async fn compute_merged_state(
        to_merge: &BTreeSet<Arc<DeployChainIndex>>,
        base_state: Blake2b256Hash,
        history_repository: &RhoHistoryRepository,
        native_changes: &[NativeStoreAction],
    ) -> Result<Blake2b256Hash, String> {
        let history_reader = history_repository.get_history_reader(base_state).await;
        let base_reader = history_reader.reader_binary();

        // Combine all state changes + mergeable diffs.
        let all_changes = to_merge.iter().fold(StateChange::empty(), |acc, b| {
            StateChange::combine(&acc, &b.state_changes)
        });
        let mut mergeable_diffs: NumberChannelsDiff = BTreeMap::new();
        for b in to_merge {
            for (k, v) in &b.event_log_index.number_channels_data {
                let entry = mergeable_diffs.entry(*k).or_insert(0);
                let sum = entry.checked_add(*v).ok_or_else(|| {
                    format!(
                        "number channel diff accumulation overflow: {entry} + {v} does not fit i64"
                    )
                })?;
                *entry = sum;
            }
        }

        // Pre-compute the mergeable-channel override actions.
        let mut overrides: BTreeMap<
            Blake2b256Hash,
            HotStoreTrieAction<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>,
        > = BTreeMap::new();
        for (hash, diff) in &mergeable_diffs {
            let changes = all_changes
                .datums_changes
                .get(hash)
                .cloned()
                .unwrap_or_default();
            let action =
                calculate_number_channel_merge(*hash, *diff, &changes, history_reader.as_ref())
                    .await?;
            overrides.insert(*hash, action);
        }

        let trie_actions = compute_trie_actions(
            &all_changes,
            base_reader.as_ref(),
            mergeable_diffs.clone(),
            |hash, _changes, _chs| overrides.get(hash).cloned(),
        )
        .await?;

        // **`reset_volatile`, not `reset`** (the programme's L2). `reset` publishes `base_state` as
        // `CURRENT_ROOT` before the merged trie exists, so a crash between it and the checkpoint below
        // left the node naming an *ancestor* on restart — neither the pre-merge nor the post-merge
        // state. The base is still validated; only the publication moves to the one place where the
        // merged state is real.
        let reset_repo = history_repository
            .reset_volatile(base_state)
            .await
            .map_err(|e| e.to_string())?;
        let new_repo = reset_repo
            .do_checkpoint_with_native(&trie_actions, native_changes)
            .await
            .map_err(|e| e.to_string())?;
        Ok(new_repo.root())
    }
}

// --- observability for a start-up replay ------------------------------------
//
// Indexing every stored block is the expensive half of a restart, and the port reported nothing about
// it: no progress, no count, and no hint that an "unresponsive" node was in fact working through its
// DAG (#60). These counters make that visible; the node logs them from its block-index closure while a
// replay runs (`wire_block_processing` in `node_runtime.rs`).
static INDEX_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static INDEX_REPLAY_FALLBACKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static INDEX_REPLAY_MILLIS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// How many blocks had to be **re-indexed** because their sidecar was written before native effects
/// carried the deploy that made them (#280).
///
/// A migration that happens silently is a migration nobody can tell from "nothing happened", which is
/// this repository's recurring failure: the counter is what makes the transition observable, and it must
/// fall to zero and stay there once every block in the store has been replayed.
static LEGACY_SIDECARS_REGENERATED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static INDEX_REPLAY_SAVE_FAILURES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static INDEX_CACHE_PRUNED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static INDEX_CACHE_CAPACITY_EVICTED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static INDEX_CACHE_LEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A snapshot of the block-index counters, for a node's progress line.
#[derive(Clone, Copy, Debug, Default)]
pub struct IndexStats {
    pub calls: u64,
    pub replay_fallbacks: u64,
    pub replay_millis: u64,
    pub replay_save_failures: u64,
    pub cache_len: u64,
    pub cache_pruned: u64,
    pub cache_capacity_evicted: u64,
}

impl IndexStats {
    /// Read the counters. Relaxed ordering throughout: this is a progress report, not a barrier.
    pub fn read() -> Self {
        use std::sync::atomic::Ordering::Relaxed;
        IndexStats {
            calls: INDEX_CALLS.load(Relaxed),
            replay_fallbacks: INDEX_REPLAY_FALLBACKS.load(Relaxed),
            replay_millis: INDEX_REPLAY_MILLIS.load(Relaxed),
            replay_save_failures: INDEX_REPLAY_SAVE_FAILURES.load(Relaxed),
            cache_len: INDEX_CACHE_LEN.load(Relaxed),
            cache_pruned: INDEX_CACHE_PRUNED.load(Relaxed),
            cache_capacity_evicted: INDEX_CACHE_CAPACITY_EVICTED.load(Relaxed),
        }
    }

    /// One line, for a log: what indexing has cost so far.
    pub fn summary(&self) -> String {
        format!(
            "{} blocks indexed, {} replay fallbacks ({} ms, {} not persisted), index cache {} entries, {} pruned, cap {}, {} capacity evicted",
            self.calls,
            self.replay_fallbacks,
            self.replay_millis,
            self.replay_save_failures,
            self.cache_len,
            self.cache_pruned,
            BLOCK_INDEX_CACHE_MAX_ENTRIES,
            self.cache_capacity_evicted
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn chain(id: u8, cost: i64) -> DeployChainIndex {
        DeployChainIndex {
            native_effects: Vec::new(),
            first_deploy_ordinal: 0,
            host_block: Blake2b256Hash::from_bytes([id; 32]),
            deploys_with_cost: BTreeSet::from([DeployIdWithCost { id: vec![id], cost }]),
            pre_state_hash: Blake2b256Hash::from_bytes([0u8; 32]),
            post_state_hash: Blake2b256Hash::from_bytes([0u8; 32]),
            event_log_index: EventLogIndex::empty(),
            state_changes: StateChange::empty(),
            cost_moves: BTreeMap::new(),
            executor: String::new(),
        }
    }

    #[test]
    fn deploy_chain_cost_sums() {
        let a = chain(1, 5);
        let b = chain(2, 7);
        assert_eq!(DeployChainIndex::deploy_chain_cost(&a), 5);
        assert_eq!(DeployChainIndex::deploy_chain_cost(&b), 7);
    }

    /// **Equality is the same key as ordering and hashing** — `(host_block, post_state_hash,
    /// deploys_with_cost)`; see the `Ord` impl above for why, and for the hang that proved it.
    ///
    /// Until #281 this test asserted the opposite: equality was over `deploysWithCost` alone (the Scala
    /// override), so two chains of *different blocks* with the same deploy set were equal while the
    /// order called them apart. That is legal in Scala, where a `TreeMap` reads only the `Ordering`, and
    /// it is not legal in Rust, where `BTreeMap`/`BTreeSet`/`sort` read the order as the identity. Its
    /// cost was not a slower search but a **lost chain** — a `BTreeSet` of chains held one of two
    /// distinct members. Inverted rather than deleted, so the correction stands where the claim stood.
    #[test]
    fn equality_is_the_same_key_as_ordering_and_hashing() {
        let a = chain(1, 5);
        let mut b = chain(1, 5);
        // A different host block is a different chain.
        b.host_block = Blake2b256Hash::from_bytes([9; 32]);
        assert_ne!(a, b, "chains of different blocks are different chains");
        assert_ne!(
            a.cmp(&b),
            Ordering::Equal,
            "and the order agrees with that, which is the requirement Rust imposes on the pair"
        );
        // Different cost — not equal, and ordered apart for the same reason.
        let c = chain(1, 6);
        assert_ne!(a, c);
        assert_ne!(a.cmp(&c), Ordering::Equal);
        // A chain is equal to itself, both ways round.
        assert_eq!(a, chain(1, 5));
        assert_eq!(a.cmp(&chain(1, 5)), Ordering::Equal);
    }

    #[test]
    fn deploys_are_conflicting_by_shared_id() {
        let a = chain(1, 5);
        let mut b = chain(1, 5);
        // Same deploy id -> conflicting (shared id), regardless of event logs.
        b.host_block = Blake2b256Hash::from_bytes([9; 32]);
        assert!(DeployChainIndex::deploys_are_conflicting(&a, &b));
        // Different ids, empty event logs -> not conflicting.
        assert!(!DeployChainIndex::deploys_are_conflicting(
            &chain(1, 5),
            &chain(2, 5)
        ));
    }

    fn msg(id: u8, parents: &[u8], seen: &[u8]) -> Message<BlockHash, Validator> {
        let hash = |b: u8| BlockHash::new([b; 32]);
        Message {
            id: hash(id),
            height: rchain_shared::refined::BlockHeight::zero(),
            sender: Validator::new([id; 65]),
            sender_seq: rchain_shared::refined::SeqNum::zero(),
            bonds_map: BTreeMap::new(),
            parents: parents.iter().map(|&b| hash(b)).collect(),
            fringe: BTreeSet::new(),
            seen: Arc::new(seen.iter().map(|&b| hash(b)).collect()),
        }
    }

    #[test]
    fn merge_scope_genesis_returns_genesis_as_base() {
        let g = msg(1, &[], &[1]);
        let c = msg(2, &[1], &[1, 2]);
        let dag = BTreeMap::from([(g.id, g.clone()), (c.id, c.clone())]);

        let (scope, base) = MergeScope::from_fringes(
            &[c.id].into_iter().collect(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &dag,
        )
        .unwrap();
        // Final scope empty -> genesis is the base; conflict scope excludes it.
        assert!(scope.final_scope.is_empty());
        assert_eq!(scope.conflict_scope, [c.id].into_iter().collect());
        assert_eq!(base, Some(g.id));
    }

    /// A non-empty final fringe: the final scope is what the final fringe has *seen* (a block's
    /// `seen` set includes itself, so the fringe block is in it), there is no base block to return,
    /// and the conflict scope is the merge fringe's own ancestors minus the final scope — here
    /// empty, because the merge fringe is the final fringe.
    #[test]
    fn merge_scope_with_a_final_fringe_has_no_base() {
        let g = msg(1, &[], &[1]);
        let c = msg(2, &[1], &[1, 2]);
        let dag = BTreeMap::from([(g.id, g.clone()), (c.id, c.clone())]);

        let (scope, base) = MergeScope::from_fringes(
            &[c.id].into_iter().collect(),
            &[c.id].into_iter().collect(),
            &BTreeSet::new(),
            &dag,
        )
        .expect("a well-formed dag");

        assert_eq!(
            scope.final_scope,
            [g.id, c.id].into_iter().collect::<BTreeSet<_>>(),
            "the final scope is what the final fringe has seen"
        );
        assert!(
            scope.conflict_scope.is_empty(),
            "the merge fringe is the final fringe, so nothing is left to merge"
        );
        assert_eq!(base, None, "a non-empty final scope has no base block");
    }

    /// A merge fringe that has *seen* a block the final fringe has not: that block is the conflict
    /// scope, because it is what merging has to reconcile.
    #[test]
    fn merge_scope_conflicts_are_the_fringe_blocks_the_final_fringe_has_not_seen() {
        // g <- a <- b, with `a` seen only by `b`; the final fringe is `a`, the merge fringe is `b`.
        let g = msg(1, &[], &[1]);
        let a = msg(2, &[1], &[1, 2]);
        let b = msg(3, &[2], &[1, 2, 3]);
        let dag = BTreeMap::from([(g.id, g.clone()), (a.id, a.clone()), (b.id, b.clone())]);

        let (scope, base) = MergeScope::from_fringes(
            &[b.id].into_iter().collect(),
            &[a.id].into_iter().collect(),
            &BTreeSet::new(),
            &dag,
        )
        .expect("a well-formed dag");

        assert_eq!(
            scope.final_scope,
            [g.id, a.id].into_iter().collect::<BTreeSet<_>>()
        );
        assert_eq!(
            scope.conflict_scope,
            [b.id].into_iter().collect::<BTreeSet<_>>(),
            "b is seen by the merge fringe only, so it is what must be merged"
        );
        assert_eq!(base, None);
    }

    /// **A pinned fringe still grows the conflict scope when a parent advances (C201).**
    ///
    /// `from_dag`'s base half is derived from the fringes alone — `final_scope`, the base and the base
    /// state — so a fringe that cannot advance cannot advance the base either. The **conflict scope** is
    /// the other half: it is `merge_fringe.seen \ final_fringe.seen`, derived from the *parents*.
    ///
    /// That is the fact that bounds C201's diagnosis, and it was written because the A2 rig measured a
    /// pre-state that never changed across 133 parent sets while one parent advanced to height 122
    /// (`spec/audit/evidence/c201/`). With the fringe held fixed, a parent further along contributes
    /// strictly more to the scope — so **the scope is not where that constancy comes from**, and a pinned
    /// fringe alone cannot explain it. What is left is that those blocks' chains were rejected in the
    /// merge, or that the caller never handed the merge an advancing parent at all.
    ///
    /// A `from_dag` that derived the conflict scope from the fringe rather than the parents would fail
    /// this, which is the mutation it exists to catch.
    #[test]
    fn a_pinned_fringe_still_grows_the_conflict_scope_with_a_parent_that_advances() {
        // g <- x2 <- x3 <- x4, with y a sibling off g that never advances.
        let g = msg(1, &[], &[1]);
        let y = msg(2, &[1], &[1, 2]);
        let x2 = msg(3, &[1], &[1, 3]);
        let x3 = msg(4, &[3], &[1, 3, 4]);
        let x4 = msg(5, &[4], &[1, 3, 4, 5]);
        let dag = BTreeMap::from([
            (g.id, g.clone()),
            (y.id, y.clone()),
            (x2.id, x2.clone()),
            (x3.id, x3.clone()),
            (x4.id, x4.clone()),
        ]);
        // The fringe neither parent can advance past — `g` alone, which is the stalled-network shape.
        let pinned: BTreeSet<BlockHash> = [g.id].into_iter().collect();
        let no_children: BTreeMap<BlockHash, BTreeSet<BlockHash>> = BTreeMap::new();

        let scope_of = |x: &Message<BlockHash, Validator>| {
            MergeScope::from_dag(
                &[x.id, y.id].into_iter().collect(),
                &pinned,
                &no_children,
                &dag,
            )
            .expect("a well-formed dag")
        };
        let (early, base_early) = scope_of(&x2);
        let (late, base_late) = scope_of(&x4);

        assert_eq!(
            early.final_scope, late.final_scope,
            "the fringe-derived half is the pinned one, which is the premise"
        );
        assert_eq!(base_early, base_late, "and so is the base it implies");
        assert!(
            early.conflict_scope.len() < late.conflict_scope.len(),
            "the parent-derived half grows with the parent: {} then {}",
            early.conflict_scope.len(),
            late.conflict_scope.len()
        );
        assert!(
            late.conflict_scope.contains(&x4.id),
            "and the block the advancing parent added is in it"
        );
    }

    /// A fringe hash that is **not in the DAG** is an error naming which fringe and which hash —
    /// never a silent omission, which would merge a scope that is quietly missing a branch.
    #[test]
    fn a_fringe_hash_absent_from_the_dag_is_an_error_naming_the_fringe() {
        let g = msg(1, &[], &[1]);
        let c = msg(2, &[1], &[1, 2]);
        let dag = BTreeMap::from([(g.id, g.clone()), (c.id, c.clone())]);
        let missing = BlockHash::new([9u8; 32]);

        let err = MergeScope::from_fringes(
            &[missing].into_iter().collect(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &dag,
        )
        .expect_err("the merge fringe is not in the dag");
        assert!(
            err.starts_with("merge fringe not in dag: "),
            "the error names the fringe: {err}"
        );
        assert!(err.contains(&missing.to_hex()), "{err}");

        let err = MergeScope::from_fringes(
            &BTreeSet::new(),
            &[missing].into_iter().collect(),
            &BTreeSet::new(),
            &dag,
        )
        .expect_err("the final fringe is not in the dag");
        assert!(err.starts_with("final fringe not in dag: "), "{err}");

        let err = MergeScope::from_fringes(
            &BTreeSet::new(),
            &BTreeSet::new(),
            &[missing].into_iter().collect(),
            &dag,
        )
        .expect_err("the prune fringe is not in the dag");
        assert!(err.starts_with("prune fringe not in dag: "), "{err}");
    }

    /// `prune_cache` is advisory: with nothing cached it returns without touching anything, and the
    /// hashes it is given are simply not there. The interesting property is that it never panics on
    /// a hashes-not-present input — it runs on the finalization path, where a panic would stop the
    /// node.
    #[test]
    fn pruning_a_cache_that_holds_nothing_is_a_no_op() {
        BlockIndex::prune_cache(&[]);
        BlockIndex::prune_cache(&[BlockHash::new([1u8; 32]), BlockHash::new([2u8; 32])]);
    }

    /// The cache must stay bounded even when finality never advances, and eviction must not invalidate
    /// an index already handed to a merge. The latter is the reason entries are `Arc`-shared: removing
    /// the cache's reference only changes whether the next lookup recomputes it.
    #[test]
    fn capacity_eviction_bounds_the_cache_and_keeps_in_flight_indices_alive() {
        let first_hash = block_hash(0);
        let first = Arc::new(BlockIndex {
            block_hash: first_hash,
            deploy_chains: Vec::new(),
        });
        let mut cache = BlockIndexCache::default();
        assert_eq!(cache.insert(first_hash, Arc::clone(&first)), 0);

        let mut last = Arc::clone(&first);
        for id in 1..=BLOCK_INDEX_CACHE_MAX_ENTRIES {
            let hash = block_hash(u16::try_from(id).expect("cache test id fits in u16"));
            last = Arc::new(BlockIndex {
                block_hash: hash,
                deploy_chains: Vec::new(),
            });
            cache.insert(hash, Arc::clone(&last));
        }

        assert_eq!(cache.len(), BLOCK_INDEX_CACHE_MAX_ENTRIES);
        assert!(
            cache.get(&first_hash).is_none(),
            "the least-recently-used cache entry is evicted"
        );
        assert_eq!(
            first.block_hash, first_hash,
            "an in-flight Arc remains valid after cache eviction"
        );
        let cached_last = cache
            .get(&last.block_hash)
            .expect("the newest index remains cached");
        assert!(Arc::ptr_eq(&cached_last, &last));
    }

    /// A hit refreshes recency. This distinguishes the shipped policy from FIFO: after filling the
    /// cache, touching the oldest entry must keep it resident when one new entry arrives, and the
    /// second-oldest entry is the one evicted.
    #[test]
    fn a_cache_hit_refreshes_lru_recency() {
        let mut cache = BlockIndexCache::default();
        for id in 0..u16::try_from(BLOCK_INDEX_CACHE_MAX_ENTRIES).expect("cache bound fits in u16")
        {
            let hash = block_hash(id);
            cache.insert(
                hash,
                Arc::new(BlockIndex {
                    block_hash: hash,
                    deploy_chains: Vec::new(),
                }),
            );
        }

        let oldest = block_hash(0);
        let second_oldest = block_hash(1);
        let _ = cache.get(&oldest).expect("oldest entry is resident");

        let newcomer = block_hash(
            u16::try_from(BLOCK_INDEX_CACHE_MAX_ENTRIES).expect("cache bound fits in u16"),
        );
        cache.insert(
            newcomer,
            Arc::new(BlockIndex {
                block_hash: newcomer,
                deploy_chains: Vec::new(),
            }),
        );

        assert!(
            cache.get(&oldest).is_some(),
            "the refreshed entry stays resident"
        );
        assert!(
            cache.get(&second_oldest).is_none(),
            "the least-recently-used entry is evicted"
        );
        assert!(cache.get(&newcomer).is_some());
        assert_eq!(cache.len(), BLOCK_INDEX_CACHE_MAX_ENTRIES);
    }

    /// Explicit finality pruning must also remove its bookkeeping entry; otherwise the queue used to
    /// enforce the hard bound would become the new unbounded structure.
    #[test]
    fn finality_pruning_removes_cache_order_bookkeeping_too() {
        let mut cache = BlockIndexCache::default();
        for id in 0..4u16 {
            let hash = block_hash(id);
            cache.insert(
                hash,
                Arc::new(BlockIndex {
                    block_hash: hash,
                    deploy_chains: Vec::new(),
                }),
            );
        }

        assert_eq!(cache.remove_many(&[block_hash(1), block_hash(3)]), 2);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.insertion_order.len(), 2);
        assert!(cache.get(&block_hash(1)).is_none());
        assert!(cache.get(&block_hash(3)).is_none());
    }

    fn block_hash(id: u16) -> BlockHash {
        let mut bytes = [0u8; 32];
        bytes[..2].copy_from_slice(&id.to_le_bytes());
        BlockHash::new(bytes)
    }

    /// The rejections a finaliser declares, as a one-element set — the per-block fact the derivation
    /// folds. Named `finaliser_rejects` rather than `fringe_data`: there is no record type any more.
    fn finaliser_rejects(marker: u8) -> BTreeSet<Vec<u8>> {
        [vec![marker]].into_iter().collect()
    }

    /// **The merge's rejection map is keyed by block, for the final scope only — absence means "not
    /// rejected".**
    ///
    /// This replaces `rejections_are_indexed_for_the_final_scope_only`. That test asserted the merge
    /// built its map by walking every fringe in `fringe_states` and cloning each set — the whole-DAG
    /// shape, with a *bound* (50 fringes × 10 blocks became 500 clones). **Both the store and that
    /// walk retired** (Law 66/68; C250's residue, C270): the derivation is per-block now and lives at
    /// the caller (`multi_parent_casper.rs::final_scope_rejections`), bounded by the fringe's members
    /// and their children rather than by the DAG. What is left here, and what `merge` itself reads,
    /// is the *shape*: one entry per final-scope block, the union of that fringe's finalisers, and
    /// **absence read as "not rejected"** (`merge`'s `_ => false`).
    ///
    /// The old test's bound half is not re-expressible here — there is no whole-DAG walk on this side
    /// of the call any more to be large — so it is *moved*, not weakened: the caller's derivation is
    /// where the bound now has to be shown, and the derivation is exercised over a real DAG by
    /// `casper/tests/merge_determinism.rs`. What stays here is the part `merge` owns.
    #[test]
    fn rejections_are_keyed_by_block_for_the_final_scope_and_absence_means_not_rejected() {
        // A fringe of three blocks, finalised by two blocks that both rejected the deploy `7`.
        let members: BTreeSet<BlockHash> = [490u16, 491, 492].into_iter().map(block_hash).collect();
        let map = rejections_of_fringe(&members, [finaliser_rejects(7), finaliser_rejects(7)]);

        assert_eq!(
            map.len(),
            3,
            "one entry per fringe member — keyed by block, not by a fringe key"
        );
        for h in &members {
            assert_eq!(
                map.get(h).expect("every member of the fringe has an entry"),
                &finaliser_rejects(7),
                "and the entry is the finalisers' rejected set"
            );
        }
        // The merge's own read: a block the derivation did not put in the map is not rejected
        // (`_ => false`), and a deploy outside the set is not rejected either.
        let absent = block_hash(499);
        assert!(
            !map.get(&absent).is_some_and(|rej| rej.contains(&vec![7u8])),
            "absence reads as 'not rejected'"
        );
        assert!(
            !map.get(&block_hash(490))
                .is_some_and(|rej| rej.contains(&vec![9u8])),
            "and so does a deploy the set does not name"
        );
    }

    /// **A disagreement between a fringe's finalisers is a union, not a written value.**
    ///
    /// This replaces `two_caches_of_one_block_set_reject_differently`, C215's second link. That test
    /// showed the merge's *input* was a function of the cache: two finalisers of one fringe wrote one
    /// key, last write won, and the two arrival orders read different records → different verdicts on
    /// the same chain. **The key retired with the store** (Law 66/68; C250's residue, C270), so that
    /// collision cannot arise — there is no one-key-many-bases to be written — and the assertion is
    /// re-expressed around what replaced it: the map is folded by **union**, so it is the same value
    /// in either arrival order, and it is *not* vacuously so (the two finalisers' sets differ, and
    /// the union is strictly larger than either).
    ///
    /// This is the property the whole unit exists for: the merge's answer is a function of the blocks,
    /// not of the order they arrived in.
    #[test]
    fn a_disagreement_between_a_fringes_finalisers_is_a_union_not_a_written_value() {
        let members: BTreeSet<BlockHash> = (490u16..500).map(block_hash).collect();

        // Two finalisers of *one* fringe that disagree about what it rejected — the C215 pair.
        let a = finaliser_rejects(7);
        let b = finaliser_rejects(8);
        assert_ne!(
            a, b,
            "the finalisers disagree — without this the union test says nothing"
        );

        let forward = rejections_of_fringe(&members, [a.clone(), b.clone()]);
        let reversed = rejections_of_fringe(&members, [b.clone(), a.clone()]);
        assert_eq!(
            forward, reversed,
            "arrival order does not reach the map: the fold is a union, so every node holding these \
             blocks derives the same rejections"
        );

        // …and the union is not a constant: it is strictly the two finalisers' sets together, larger
        // than either alone, so the equality above is about the union and not about an empty map.
        let expected: BTreeSet<Vec<u8>> = a.union(&b).cloned().collect();
        let block = block_hash(495);
        assert_eq!(
            forward.get(&block),
            Some(&expected),
            "the entry is the union of what the finalisers rejected"
        );
        assert!(
            expected.len() > a.len() && expected.len() > b.len(),
            "the union is strictly larger than either finaliser's set — the disagreement is not folded \
             away, it is *added*"
        );
    }
}

#[cfg(test)]
mod merge_relation_tests {
    use super::*;

    use rchain_rspace::trace::event::Produce as RProduce;

    fn hash(byte: u8) -> Blake2b256Hash {
        Blake2b256Hash::from_bytes([byte; 32])
    }

    fn deploy_id(byte: u8, cost: i64) -> DeployIdWithCost {
        DeployIdWithCost {
            id: vec![byte],
            cost,
        }
    }

    /// A deploy index whose event log touches one produce (so conflicts/dependencies can be built).
    fn deploy_index(id: Vec<u8>, cost: i64, channel: u8) -> DeployIndex {
        let produce = RProduce::apply(&format!("chan{channel}"), &format!("datum{channel}"), false);
        DeployIndex {
            deploy_id: id,
            cost,
            event_log_index: EventLogIndex {
                produces_linear: [produce.clone()].into_iter().collect(),
                ..Default::default()
            },
        }
    }

    fn chain(host: u8, post: u8, deploys: &[(u8, i64, u8)]) -> DeployChainIndex {
        DeployChainIndex {
            native_effects: Vec::new(),
            first_deploy_ordinal: 0,
            host_block: hash(host),
            deploys_with_cost: deploys
                .iter()
                .map(|(id, cost, _)| deploy_id(*id, *cost))
                .collect(),
            pre_state_hash: hash(0),
            post_state_hash: hash(post),
            event_log_index: deploys
                .iter()
                .fold(EventLogIndex::empty(), |acc, (_, _, ch)| {
                    // A test chain's diffs are small; an error here would be a test bug, not a merge path.
                    EventLogIndex::combine(&acc, &deploy_index(vec![1], 0, *ch).event_log_index)
                        .expect("a test chain's accumulation cannot overflow")
                }),
            state_changes: StateChange::empty(),
            cost_moves: BTreeMap::new(),
            executor: String::new(),
        }
    }

    /// `DeployIndex` orders by **deploy id**, not cost: the rejection-option search sorts deploys by
    /// id, so two deploys with the same cost are distinguished only by their id.
    #[test]
    fn a_deploy_index_orders_by_id() {
        let a = deploy_index(vec![1], 100, 1);
        let b = deploy_index(vec![2], 1, 2);
        assert!(a < b, "id 1 sorts before id 2 regardless of cost");
        assert_eq!(
            a.deploy_id,
            vec![1],
            "the ordering key is the id field itself"
        );
        assert_eq!(DeployIndex::sys_slash_deploy_id(), vec![1]);
        assert_eq!(DeployIndex::sys_close_block_deploy_id(), vec![2]);
        assert_eq!(DeployIndex::sys_empty_deploy_id(), vec![3]);
        assert_eq!(DeployIndex::SYS_SLASH_DEPLOY_COST, 0);
    }

    /// **A chain's equality, ordering and hash are one key** — `(host_block, post_state_hash,
    /// deploys_with_cost)` — which is what Rust requires of a type used as a `BTreeMap`/`BTreeSet` key.
    ///
    /// Until #281 this test asserted the *mismatch*: "equality over the deploy set (the Scala override),
    /// ordering over `(host_block, post_state_hash)` … pinned here so the mismatch is visible rather
    /// than latent." It was visible, and it still hung the merge. Chains of one block are equal on the
    /// hash pair and distinct on the deploy set, so the dependency map the chain-level native relation
    /// builds could hold one key whose own value looked that key up again — and `traverse_tree`, which
    /// had no visited set, walked a one-element cycle until CI's 45-minute job limit killed the job. The
    /// debt the pin marked is paid here rather than carried further.
    #[test]
    fn a_chains_equality_ordering_and_hash_are_one_key() {
        let a = chain(1, 2, &[(1, 10, 1)]);
        let mut b = chain(9, 8, &[(1, 10, 1)]);
        b.pre_state_hash = hash(7);

        assert_ne!(
            a, b,
            "same deploys, different block hashes — different chains, so not equal"
        );
        assert_ne!(
            a.cmp(&b),
            Ordering::Equal,
            "and the ordering agrees with that, which is the whole requirement"
        );

        // Identical in every part of the key: equal, and ordered equal.
        let identical = chain(1, 2, &[(1, 10, 1)]);
        assert_eq!(a, identical);
        assert_eq!(a.cmp(&identical), Ordering::Equal);

        // A different deploy set on the same hashes is a different chain: the deploy key is the
        // tie-breaker the hash pair alone does not carry.
        let different_deploys = chain(1, 2, &[(2, 5, 1)]);
        assert_ne!(a, different_deploys);
        assert_ne!(a.cmp(&different_deploys), Ordering::Equal);

        // The Scala ordering's primary pair still decides chains of *different* blocks, so the relative
        // order the merge relied on is unchanged: post-state hash 2 sorts before 3.
        let later_post = chain(1, 3, &[(1, 10, 1)]);
        assert!(a < later_post, "post state hash 2 < 3");
    }

    /// `deploy_chain_cost` is the sum of the member costs (the merge's cost objective).
    #[test]
    fn the_chain_cost_is_the_sum_of_its_deploys() {
        let c = chain(1, 2, &[(1, 10, 1), (2, 5, 2), (3, 0, 3)]);
        assert_eq!(DeployChainIndex::deploy_chain_cost(&c), 15);
        assert_eq!(DeployChainIndex::deploy_chain_cost(&chain(1, 2, &[])), 0);
    }

    /// Two chains that share a deploy id conflict (the same deploy cannot be in both branches), and
    /// so do two chains whose event logs conflict — but *disjoint, non-conflicting* chains do not.
    #[test]
    fn chain_conflicts_cover_the_id_overlap_and_the_event_relation() {
        let a = chain(1, 2, &[(1, 10, 1)]);
        let shared_id = chain(3, 4, &[(1, 10, 9)]);
        let disjoint = chain(5, 6, &[(2, 10, 2)]);

        assert!(
            DeployChainIndex::deploys_are_conflicting(&a, &shared_id),
            "a shared deploy id is a conflict"
        );
        assert!(
            !DeployChainIndex::deploys_are_conflicting(&a, &disjoint),
            "disjoint deploys on different channels do not conflict"
        );

        // Two chains that destroy the *same* produce (different ids, same channel) conflict through
        // their event logs, not through their ids — the second half of the relation.
        let produced = RProduce::apply(&"shared".to_string(), &"datum".to_string(), false);
        let destroys = |id: u8, host: u8, produced: RProduce| DeployChainIndex {
            native_effects: Vec::new(),
            first_deploy_ordinal: 0,
            host_block: hash(host),
            deploys_with_cost: [deploy_id(id, 0)].into_iter().collect(),
            pre_state_hash: hash(0),
            post_state_hash: hash(host),
            event_log_index: EventLogIndex {
                produces_consumed: [produced].into_iter().collect(),
                ..Default::default()
            },
            state_changes: StateChange::empty(),
            cost_moves: BTreeMap::new(),
            executor: String::new(),
        };
        let first = destroys(10, 1, produced.clone());
        let second = destroys(11, 2, produced.clone());
        assert!(
            DeployChainIndex::deploys_are_conflicting(&first, &second),
            "both destroy the same produce: a conflict with no shared id"
        );
        assert!(
            !DeployChainIndex::deploys_are_conflicting(&first, &disjoint),
            "and a chain that touches neither the produce nor the id does not conflict"
        );
    }

    /// `branches_are_conflicting` lifts the same test to *sets* of chains: a shared id anywhere in
    /// either branch is a conflict, and so is a conflicting pair of combined event logs. It is
    /// fallible (AUDIT C41), so this test unwraps: a test chain's diffs are small.
    fn conflicts(a: &BTreeSet<Arc<DeployChainIndex>>, b: &BTreeSet<Arc<DeployChainIndex>>) -> bool {
        DeployChainIndex::branches_are_conflicting(a, b)
            .expect("a test chain's accumulation cannot overflow")
    }

    #[test]
    fn branch_conflicts_lift_the_chain_relation() {
        let a: BTreeSet<Arc<DeployChainIndex>> =
            [Arc::new(chain(1, 2, &[(1, 10, 1)]))].into_iter().collect();
        let b: BTreeSet<Arc<DeployChainIndex>> =
            [Arc::new(chain(3, 4, &[(1, 10, 9)]))].into_iter().collect();
        let c: BTreeSet<Arc<DeployChainIndex>> =
            [Arc::new(chain(5, 6, &[(2, 10, 2)]))].into_iter().collect();

        assert!(conflicts(&a, &b));
        assert!(!conflicts(&a, &c));
        // An empty branch conflicts with nothing.
        assert!(!conflicts(&BTreeSet::new(), &a));
    }

    /// `depends` between chains is the event-log dependency: a target that consumed what the source
    /// created depends on it. With disjoint logs it does not.
    #[test]
    fn chain_dependency_follows_the_event_logs() {
        let produce = RProduce::apply(&"chan".to_string(), &"datum".to_string(), false);
        let source = DeployChainIndex {
            native_effects: Vec::new(),
            first_deploy_ordinal: 0,
            host_block: hash(1),
            deploys_with_cost: [deploy_id(1, 0)].into_iter().collect(),
            pre_state_hash: hash(0),
            post_state_hash: hash(1),
            event_log_index: EventLogIndex {
                produces_linear: [produce.clone()].into_iter().collect(),
                ..Default::default()
            },
            state_changes: StateChange::empty(),
            cost_moves: BTreeMap::new(),
            executor: String::new(),
        };
        // The target consumed the same produce.
        let target = DeployChainIndex {
            native_effects: Vec::new(),
            first_deploy_ordinal: 0,
            host_block: hash(2),
            deploys_with_cost: [deploy_id(2, 0)].into_iter().collect(),
            pre_state_hash: hash(1),
            post_state_hash: hash(2),
            event_log_index: EventLogIndex {
                produces_consumed: [produce].into_iter().collect(),
                ..Default::default()
            },
            state_changes: StateChange::empty(),
            cost_moves: BTreeMap::new(),
            executor: String::new(),
        };

        assert!(DeployChainIndex::depends(&target, &source));
        assert!(!DeployChainIndex::depends(&source, &target));
        assert!(!DeployChainIndex::depends(&source, &source));
    }

    /// The deploy id is what the wire types carry too: `DeployIdWithCost` hashes and orders by both
    /// fields (the pair), unlike `DeployIndex` which orders by the id alone.
    #[test]
    fn a_deploy_with_cost_orders_by_id_then_cost() {
        let a = deploy_id(1, 100);
        let b = deploy_id(1, 5);
        let c = deploy_id(2, 0);
        assert!(b < a, "same id: the lower cost sorts first");
        assert!(a < c, "a lower id sorts first regardless of cost");
        // Hashing covers both fields (a `HashSet` keyed by the pair).
        let mut set = std::collections::HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&a));
    }
}

/// The falsifier for issue #74: a multi-parent merge must carry the **native** writes of the branches
/// it merges.
///
/// `DeployChainIndex::state_changes` is a `StateChange` - datums, continuations and joins only. Native
/// system-contract state (PoS pool/active/trusted/params, vault balances, registry) lives under
/// dedicated trie prefixes and produces no Rholang event, so it has no representation there. Before
/// this change `compute_merged_state` checkpointed a branch's tuple-space effects without its native
/// ones, and every PoS write in a merged branch - a `trust`, a `bond`, an epoch's activation, a
/// slashed stake - silently reverted. That is the state loss #74 reported (`getBonds` 3 -> 1 with no
/// slashing and no error), and it is why a one-validator chain (single parent, no merge) grew its
/// validator set while a multi-validator chain could not.
#[cfg(test)]
mod native_merge_tests {
    use super::*;

    use rchain_rspace::factory::create_history_repository;
    use rchain_rspace::native_store::PREFIX_POS;
    use rchain_shared::store_manager::InMemoryStoreManager;

    fn key(byte: u8) -> Blake2b256Hash {
        Blake2b256Hash::from_bytes([byte; 32])
    }

    async fn empty_repo() -> RhoHistoryRepository {
        let manager = InMemoryStoreManager::default();
        create_history_repository::<SortedProc, BindPattern, ListParWithRandom, TaggedContinuation>(
            &manager, "rspace",
        )
        .await
        .expect("an in-memory history repository")
    }

    /// **The two intra-block obligations, which the block-level rule never had** (#280).
    ///
    /// They are the half through which a chain-level relation could fix the incident and introduce a
    /// *different* lost write, which is why they are asserted rather than left to the implementation:
    ///
    /// - **order** — two chains of one block that write one slot are applied in **deploy order**, so
    ///   the slot holds the later chain's value. (The `(seen_count, host)` key the fold used before
    ///   cannot express this: two chains of one block tie on both, so their order was the `BTreeSet`'s.)
    /// - **dependency** — the later chain *depends* on the earlier one and not the reverse, because its
    ///   absolute value was computed on top of the earlier's. Without the edge, resolution could keep
    ///   the later chain and reject the earlier, applying a value built on a write the state does not
    ///   hold.
    #[tokio::test]
    async fn a_blocks_own_chains_are_ordered_and_dependent() {
        let base_repo = empty_repo().await;
        let base_state = base_repo.root();
        let shared = key(21);
        let host = BlockHash::new([0xa0; 32]);

        let chain = |ordinal: u32, id: u8, value: u8| DeployChainIndex {
            host_block: Blake2b256Hash::from_bytes(*host.as_bytes()),
            deploys_with_cost: BTreeSet::from([DeployIdWithCost {
                id: vec![id],
                cost: 0,
            }]),
            pre_state_hash: base_state,
            post_state_hash: base_state,
            event_log_index: EventLogIndex::empty(),
            state_changes: StateChange::empty(),
            cost_moves: BTreeMap::new(),
            executor: String::new(),
            native_effects: vec![NativeStoreAction::Put {
                prefix: PREFIX_POS,
                key: shared,
                value: vec![value],
            }],
            first_deploy_ordinal: ordinal,
        };
        let earlier = chain(0, 1, 1);
        let later = chain(1, 2, 2);

        // **Dependency**: later → earlier, and never the reverse.
        let rel = NativeRelations {
            ancestry: &BTreeMap::new(),
        };
        assert!(
            rel.depends(&later, &earlier),
            "the later chain's value was computed on top of the earlier's, so it depends on it"
        );
        assert!(
            !rel.depends(&earlier, &later),
            "and not the reverse — the earlier chain's value cannot contain the later's"
        );
        assert!(
            !rel.conflicting(&earlier, &later),
            "they are one block's own chains, so the native relation does not call them concurrent: \
             the deploy order decides, which is the obligation above"
        );

        // **Order**: merged alone, the slot holds the **later** chain's value.
        let block = BlockIndex {
            block_hash: host,
            deploy_chains: vec![Arc::new(earlier), Arc::new(later)],
        };
        let scope = MergeScope {
            final_scope: BTreeSet::new(),
            conflict_scope: BTreeSet::from([host]),
            ancestry: BTreeMap::new(),
        };
        let lookup = move |h: BlockHash| {
            let found = (block.block_hash == h).then(|| block.clone());
            async move {
                found
                    .map(Arc::new)
                    .ok_or_else(|| format!("no index for {h:?}"))
            }
        };
        let outcome = MergeScope::merge(
            &scope,
            base_state,
            &RejectionsMap::new(),
            &base_repo,
            &lookup,
            DeployChainIndex::deploy_chain_cost,
        )
        .await
        .expect("a block with two chains on one slot merges");
        assert!(
            outcome.rejected_deploys.is_empty(),
            "nothing to conflict with: {:?}",
            outcome.rejected_deploys
        );
        let reader = base_repo.get_history_reader(outcome.state).await;
        assert_eq!(
            reader
                .get_native(PREFIX_POS, shared)
                .await
                .expect("a readable native slot"),
            Some(vec![2]),
            "the slot holds the **later** chain's value: the deploy order is applied, which is the \
             obligation the `(seen_count, host)` key could not express"
        );
    }

    /// **A chain's order must agree with its equality**, because `BTreeMap`, `BTreeSet` and `sort` all
    /// read the order as the identity.
    ///
    /// The port kept the Scala's split — order over `(hostBlock, postStateHash)`, equality over
    /// `deploysWithCost` — which is legal in Scala (a `TreeMap` there reads only the `Ordering`) and is
    /// not legal in Rust. Two chains of one block, same host and same post-state but different deploys,
    /// therefore compared `Ordering::Equal` while `==` said they differed: a `BTreeSet` of them held
    /// **one**, and the dependency map the merge builds could hold a key whose own value looked the key
    /// up again.
    ///
    /// Falsifier, both forms. **Pre-fix (witnessed)**: the two assertions below fail on the old impls —
    /// `cmp` answered `Equal` for the pair and the set held one element — and the merge test above does
    /// not fail at all for the same input, it **hangs**: the dependency map had one key whose value was
    /// the other chain, so `traverse_tree`, which had no visited set, walked a one-element cycle until
    /// CI's 45-minute job limit killed the job (`#281`). Post-fix: the order is total, and the set holds
    /// both chains.
    #[test]
    fn a_chains_order_agrees_with_its_equality() {
        let chain = |ordinal: u32, id: u8, value: u8| DeployChainIndex {
            host_block: Blake2b256Hash::from_bytes(*BlockHash::new([0xa0; 32]).as_bytes()),
            deploys_with_cost: BTreeSet::from([DeployIdWithCost {
                id: vec![id],
                cost: 0,
            }]),
            pre_state_hash: key(31),
            post_state_hash: key(32),
            event_log_index: EventLogIndex::empty(),
            state_changes: StateChange::empty(),
            cost_moves: BTreeMap::new(),
            executor: String::new(),
            native_effects: vec![NativeStoreAction::Put {
                prefix: PREFIX_POS,
                key: key(21),
                value: vec![value],
            }],
            first_deploy_ordinal: ordinal,
        };
        let earlier = chain(0, 1, 1);
        let later = chain(1, 2, 2);

        assert_ne!(earlier, later, "different deploys are different chains");
        assert_ne!(
            earlier.cmp(&later),
            Ordering::Equal,
            "so the order must not call them equal — this is the disagreement that hung the merge"
        );
        assert_eq!(earlier, chain(0, 1, 1), "the same chain is equal to itself");
        assert_eq!(
            earlier.cmp(&chain(0, 1, 1)),
            Ordering::Equal,
            "and it orders equal to itself, the other direction of the same requirement"
        );

        let set: BTreeSet<DeployChainIndex> = [earlier.clone(), later].into_iter().collect();
        assert_eq!(
            set.len(),
            2,
            "a set of two distinct chains holds two elements; holding one is a lost chain"
        );
    }

    /// **The merge-level acceptance condition** (aixaria0's review on #281): two deploys in one block
    /// write the same native slot, the **later** writer is rejected by an actual merge conflict, and the
    /// **earlier** accepted value is what the merged state holds.
    ///
    /// `a_blocks_own_chains_are_ordered_and_dependent` pins the *relation* — a block's own chains are
    /// ordered by deploy order and dependent, later on earlier. What that cannot show is the consequence
    /// the relation exists for. Here a sibling block writes the second slot the later chain wrote, so
    /// resolution must reject exactly one of {later, sibling}; the later chain carries the smaller cost,
    /// so it is the one rejected — and the earlier chain's value must still be **in the merged state**,
    /// rather than going down with its block-mate.
    ///
    /// **What this adds over the rejection-set tests, including the one beside it.**
    /// `a_rejected_boundary_chain_does_not_take_its_blocks_other_chains` asserts *which* chains land in
    /// the rejection set and says nothing about the merged state. This asserts the **value**: the earlier
    /// chain's write is what the merged state holds for the contended slot. That is the condition a reader
    /// of the incident actually depends on — the lost write was a *value* missing from the tip, not an id
    /// in a set — and it is the form the pre-#280 rule fails, because `reject_whole_blocks` closed a
    /// rejection over the whole block: the earlier chain would have been rejected as the later one's
    /// block-mate and the slot would hold nothing at all. The inversion of
    /// `a_rejected_native_block_loses_every_chain` into the test beside it is the witness that the old
    /// rule did exactly that.
    #[tokio::test]
    async fn a_rejected_later_chain_leaves_the_earlier_ones_value_in_the_merged_state() {
        let base_repo = empty_repo().await;
        let base_state = base_repo.root();
        let contended = key(21);
        let only_later = key(22);

        let host = BlockHash::new([0xa0; 32]);
        let sibling_host = BlockHash::new([0xb0; 32]);

        let chain =
            |host: BlockHash, ordinal: u32, id: u8, cost: i64, writes: &[(Blake2b256Hash, u8)]| {
                DeployChainIndex {
                    host_block: Blake2b256Hash::from_bytes(*host.as_bytes()),
                    deploys_with_cost: BTreeSet::from([DeployIdWithCost { id: vec![id], cost }]),
                    pre_state_hash: base_state,
                    post_state_hash: base_state,
                    event_log_index: EventLogIndex::empty(),
                    state_changes: StateChange::empty(),
                    cost_moves: BTreeMap::new(),
                    executor: String::new(),
                    native_effects: writes
                        .iter()
                        .map(|(key, value)| NativeStoreAction::Put {
                            prefix: PREFIX_POS,
                            key: *key,
                            value: vec![*value],
                        })
                        .collect(),
                    first_deploy_ordinal: ordinal,
                }
            };

        // The block's own two chains: both write `contended`, and the later one also writes `only_later`.
        let earlier = chain(host, 0, 0xa1, 1000, &[(contended, 1)]);
        let later = chain(host, 1, 0xa2, 1, &[(contended, 2), (only_later, 2)]);
        // The sibling wrote only `only_later`, so it conflicts with the later chain and with nothing
        // else; it carries the larger cost, so resolution rejects the later chain rather than it.
        let sibling = chain(sibling_host, 0, 0xb1, 1000, &[(only_later, 9)]);

        let block = BlockIndex {
            block_hash: host,
            deploy_chains: vec![Arc::new(earlier), Arc::new(later)],
        };
        let other = BlockIndex {
            block_hash: sibling_host,
            deploy_chains: vec![Arc::new(sibling)],
        };
        let lookup = move |h: BlockHash| {
            let found = if block.block_hash == h {
                Some(block.clone())
            } else if other.block_hash == h {
                Some(other.clone())
            } else {
                None
            };
            async move {
                found
                    .map(Arc::new)
                    .ok_or_else(|| format!("no index for {h:?}"))
            }
        };
        let scope = MergeScope {
            final_scope: BTreeSet::new(),
            conflict_scope: BTreeSet::from([host, sibling_host]),
            ancestry: BTreeMap::new(),
        };
        let outcome = MergeScope::merge(
            &scope,
            base_state,
            &RejectionsMap::new(),
            &base_repo,
            &lookup,
            DeployChainIndex::deploy_chain_cost,
        )
        .await
        .expect("a block whose own chains share a slot, beside a conflicting sibling, merges");

        assert_eq!(
            outcome.rejected_deploys,
            BTreeSet::from([vec![0xa2u8]]),
            "the later chain is rejected, and only it"
        );
        let reader = base_repo.get_history_reader(outcome.state).await;
        assert_eq!(
            reader
                .get_native(PREFIX_POS, contended)
                .await
                .expect("a readable native slot"),
            Some(vec![1]),
            "so the merged state holds the **earlier** chain's value: the later chain's rejection \
             cannot take a write it never made"
        );
        assert_eq!(
            reader
                .get_native(PREFIX_POS, only_later)
                .await
                .expect("a readable native slot"),
            Some(vec![9]),
            "and the sibling that won the contended slot keeps its write"
        );
    }

    #[tokio::test]
    async fn a_merge_carries_the_native_writes_of_the_branches_it_merges() {
        // A base state that already holds a native leaf, so the test also shows the merge keeps the
        // base's own native state.
        let base_key = key(1);
        let base_repo = empty_repo()
            .await
            .do_checkpoint_with_native(
                &[],
                &[NativeStoreAction::Put {
                    prefix: PREFIX_POS,
                    key: base_key,
                    value: vec![9],
                }],
            )
            .await
            .expect("the base checkpoint");
        let base_state = base_repo.root();

        // One branch block whose only effect is a native write: a PoS call produces no tuple-space
        // chain, which is exactly the shape that was dropped.
        let branch_key = key(2);
        let child = BlockHash::new([3u8; 32]);
        let branch_index = BlockIndex {
            block_hash: child,
            deploy_chains: vec![Arc::new(DeployChainIndex {
                first_deploy_ordinal: 0,
                host_block: key(3),
                deploys_with_cost: BTreeSet::from([DeployIdWithCost {
                    id: vec![42],
                    cost: 0,
                }]),
                pre_state_hash: base_state,
                post_state_hash: base_state,
                event_log_index: EventLogIndex::empty(),
                state_changes: StateChange::empty(),
                cost_moves: BTreeMap::new(),
                executor: String::new(),
                // **The write rides on the chain, not on the block** (#280) — which is what the
                // merge now reads, and the only shape in which a write can be attributed at all.
                native_effects: vec![NativeStoreAction::Put {
                    prefix: PREFIX_POS,
                    key: branch_key,
                    value: vec![7],
                }],
            })],
        };
        let block_index = move |_h: BlockHash| {
            let index = branch_index.clone();
            async move { Ok::<Arc<BlockIndex>, String>(Arc::new(index)) }
        };

        // The branch is the conflict scope; nothing has finalised.
        let scope = MergeScope {
            final_scope: BTreeSet::new(),
            conflict_scope: BTreeSet::from([child]),
            ancestry: BTreeMap::new(),
        };
        let outcome = MergeScope::merge(
            &scope,
            base_state,
            &RejectionsMap::new(),
            &base_repo,
            &block_index,
            |_| 0,
        )
        .await
        .expect("the merge");
        let (merged, _rejected) = (outcome.state, outcome.rejected_deploys);

        let reader = base_repo.get_history_reader(merged).await;
        assert!(
            reader
                .get_native(PREFIX_POS, branch_key)
                .await
                .expect("a readable native leaf")
                .is_some(),
            "the branch's native write must survive the merge (#74)"
        );
        assert!(
            reader
                .get_native(PREFIX_POS, base_key)
                .await
                .expect("a readable native leaf")
                .is_some(),
            "and the base's native state must still be there"
        );
    }

    /// **The reproduction for #83, at the granularity the defect lives at.**
    ///
    /// `merge` derives one action per slot from the accepted chains' own effects (`NativeRelations::
    /// fold`). Each *chain's* own list is duplicate-free by construction —
    /// `InMemNativeStore::drain_native` maps a `BTreeMap<(prefix, key), _>`, one action per slot, and
    /// clears the overlay — but **two blocks at the same height can write the same slot**, and
    /// at an epoch boundary that is not a rare race: *every* proposer runs `close_block`, which
    /// writes the same five `PREFIX_POS` leaves (`bonds`, `active`, `withdrawers`,
    /// `pending_withdrawers`, `committed_rewards`) from the same pre-state. Two of those blocks both
    /// accepted into one merge produce the same key twice, and `RadixHistory::process` refuses the
    /// batch with the panic the issue reports — on every node, at the same height, because the
    /// inputs are identical.
    ///
    /// This is the deterministic half of the diagnosis: no devnet, no scheduler, two branches and
    /// one slot. What it asserts is the contract a fix owes, which is *not* merely "does not panic":
    /// the two concurrent writers **conflict**, so exactly one host is rejected and the slot holds
    /// the other's value whole. (The first fix kept the last host's write in hash order and rejected
    /// nothing; `boundary_merge_tests` shows why a kept-both merge loses a transition.)
    ///
    /// The fixture is chosen so that assertion can discriminate. The *values* are swapped between
    /// the two rounds, so the choice of which host survives must not depend on them: a rule that
    /// took the numerically larger value, or the last-arriving write, would pick differently.
    #[tokio::test]
    async fn two_branch_blocks_writing_one_native_slot_do_not_panic_the_merge() {
        let base_repo = empty_repo().await;
        let base_state = base_repo.root();
        let shared = key(9);

        let branch = |host: u8, value: u8| BlockIndex {
            block_hash: BlockHash::new([host; 32]),
            deploy_chains: vec![Arc::new(DeployChainIndex {
                first_deploy_ordinal: 0,
                host_block: key(host),
                deploys_with_cost: BTreeSet::from([DeployIdWithCost {
                    id: vec![host],
                    cost: 0,
                }]),
                pre_state_hash: base_state,
                post_state_hash: base_state,
                event_log_index: EventLogIndex::empty(),
                state_changes: StateChange::empty(),
                cost_moves: BTreeMap::new(),
                executor: String::new(),
                // **The write rides on the chain** (#280) — see `branch_index` above.
                native_effects: vec![NativeStoreAction::Put {
                    prefix: PREFIX_POS,
                    key: shared,
                    value: vec![value],
                }],
            })],
        };
        let hosts = [BlockHash::new([1u8; 32]), BlockHash::new([2u8; 32])];
        let mut rejected_hosts = Vec::new();

        for (first_value, second_value) in [(99u8, 22u8), (22u8, 99u8)] {
            let indexes: BTreeMap<BlockHash, BlockIndex> = [
                (hosts[0], branch(1, first_value)),
                (hosts[1], branch(2, second_value)),
            ]
            .into_iter()
            .collect();
            let block_index = move |h: BlockHash| {
                let index = indexes.get(&h).cloned();
                async move {
                    index
                        .map(Arc::new)
                        .ok_or_else(|| format!("no index for {h:?}"))
                }
            };
            let scope = MergeScope {
                final_scope: BTreeSet::new(),
                conflict_scope: BTreeSet::from(hosts),
                ancestry: BTreeMap::new(),
            };

            let outcome = MergeScope::merge(
                &scope,
                base_state,
                &RejectionsMap::new(),
                &base_repo,
                &block_index,
                |_| 0,
            )
            .await
            .expect(
                "two accepted blocks writing one native slot must merge rather than panic (#83): \
                 an epoch boundary puts several `close_block`s at one height by construction",
            );
            let (merged, rejected) = (outcome.state, outcome.rejected_deploys);

            assert_eq!(rejected.len(), 1, "the two concurrent writers conflict");
            let rejected_host = rejected.iter().next().expect("one rejected host")[0];
            rejected_hosts.push(rejected_host);
            let surviving_value = if rejected_host == 1 {
                second_value
            } else {
                first_value
            };
            let reader = base_repo.get_history_reader(merged).await;
            assert_eq!(
                reader
                    .get_native(PREFIX_POS, shared)
                    .await
                    .expect("a readable native leaf"),
                Some(vec![surviving_value]),
                "the slot holds the surviving host's write"
            );
        }
        assert_eq!(
            rejected_hosts[0], rejected_hosts[1],
            "which host is rejected must not depend on what the hosts wrote: every node decides it \
             from the same DAG, whatever the values"
        );
    }
}

/// Issue #83: two blocks that each wrote a native key, merged in one scope.
///
/// Every epoch boundary's `close_block` writes the same six PoS keys, so two unfinalised sibling
/// blocks at a boundary height - routine with three validators - used to reach radix history as one
/// batch with every key twice, and every node panicked (`Cannot process duplicate actions on one
/// key`) in the same second. These play the boundary for real on `NativeSystemState`, then merge.
#[cfg(test)]
mod rejection_reason_tests {
    use super::rejection_has_a_reason;
    use std::collections::{BTreeMap, BTreeSet};

    fn set(xs: impl IntoIterator<Item = i32>) -> BTreeSet<i32> {
        xs.into_iter().collect()
    }

    #[test]
    fn i1_includes_finalized_conflicts_and_rejected_dependencies() {
        let conflicts = BTreeMap::from([(1, set([10]))]);
        let dependencies = BTreeMap::from([(20, set([2]))]);
        assert!(rejection_has_a_reason(
            &1,
            &set([]),
            &set([1]),
            &conflicts,
            &dependencies,
            &set([10]),
            &set([20]),
        ));
        assert!(rejection_has_a_reason(
            &2,
            &set([]),
            &set([2]),
            &conflicts,
            &dependencies,
            &set([10]),
            &set([20]),
        ));
        assert!(!rejection_has_a_reason(
            &3,
            &set([]),
            &set([3]),
            &conflicts,
            &dependencies,
            &set([10]),
            &set([20]),
        ));
    }

    #[test]
    fn i1_accepts_a_dependency_cascade_and_rejects_self_justification() {
        let kept = set([10]);
        let rejected = set([1, 2, 3]);

        // 1 conflicts with a kept chain: a direct reason.
        let conflicts = BTreeMap::from([(1, set([10])), (3, set([2]))]);
        // 2 depends on rejected 1; 3 depends on rejected 2.  These are the exact directed edges
        // `with_dependencies` follows when it cascades a rejection.
        let dependencies = BTreeMap::from([(1, set([2])), (2, set([3]))]);

        assert!(rejection_has_a_reason(
            &1,
            &kept,
            &rejected,
            &conflicts,
            &dependencies,
            &set([]),
            &set([])
        ));
        assert!(rejection_has_a_reason(
            &2,
            &kept,
            &rejected,
            &conflicts,
            &dependencies,
            &set([]),
            &set([])
        ));
        assert!(rejection_has_a_reason(
            &3,
            &kept,
            &rejected,
            &conflicts,
            &dependencies,
            &set([]),
            &set([])
        ));

        // A conflict only with another rejected chain is not a reason by itself.
        let no_dependencies = BTreeMap::new();
        assert!(!rejection_has_a_reason(
            &3,
            &BTreeSet::new(),
            &set([2, 3]),
            &conflicts,
            &no_dependencies,
            &set([]),
            &set([]),
        ));
    }
}

#[cfg(test)]
mod boundary_merge_tests {
    use super::*;
    use std::sync::Arc;

    use rchain_crypto::public_key::PublicKey;
    use rchain_rholang::native_state::{NativeSystemState, PosGenesis, PosParams};
    use rchain_rholang::util::rev_address::RevAddress;
    use rchain_rspace::factory::create_history_repository;
    use rchain_rspace::native_store::{InMemNativeStore, NativeWriter, PREFIX_POS};
    use rchain_shared::store_manager::InMemoryStoreManager;

    const EPOCH: i64 = 10;

    fn validator(byte: u8) -> Validator {
        Validator::from_slice(&[byte; 65])
    }

    fn nn(x: i64) -> NonNegI64 {
        NonNegI64::try_from(x).unwrap()
    }

    fn payer(byte: u8) -> (PublicKey, String) {
        let pk = PublicKey::new(vec![byte; 65]);
        let address = RevAddress::from_public_key(&pk).unwrap().to_base58();
        (pk, address)
    }

    /// The issue's genesis - bonds 100/100/50, `--epoch-length 10` - with two funded deployers.
    async fn genesis() -> (RhoHistoryRepository, Blake2b256Hash) {
        let manager = InMemoryStoreManager::default();
        let repo = create_history_repository::<
            SortedProc,
            BindPattern,
            ListParWithRandom,
            TaggedContinuation,
        >(&manager, "rspace")
        .await
        .unwrap();
        let store = Arc::new(InMemNativeStore::empty());
        let native = NativeSystemState::new(store.clone());
        native
            .install_genesis(&PosGenesis {
                bonds: [
                    (validator(1), nn(100)),
                    (validator(2), nn(100)),
                    (validator(3), nn(50)),
                ]
                .into_iter()
                .collect(),
                trusted: BTreeSet::new(),
                params: PosParams {
                    epoch_length: EPOCH,
                    quarantine_length: EPOCH,
                    minimum_bond: nn(1),
                    maximum_bond: nn(100),
                    number_of_active_validators: 10,
                    executor_share: nn(0),
                    absence_slack: nn(0),
                    participation_grace: nn(0),
                    min_phlo_price: NonNegI64::zero(),
                    max_number_of_parents: 0,
                    dev_tax_vesting_blocks: NonNegI64::zero(),
                    liveness_absence_threshold: 0,
                    liveness_jail_after_boundaries: 0,
                    liveness_jail_epochs: 0,
                    liveness_vote_window: 0,
                    liveness_slash_severity: 0,
                },
                dev_vesting: None,
            })
            .unwrap();
        for byte in [8, 9] {
            native.set_vault_balance(&payer(byte).1, nn(1000));
        }
        let repo = repo
            .do_checkpoint_with_native(&[], &store.drain_changes())
            .await
            .unwrap();
        let root = repo.root();
        (repo, root)
    }

    /// What a block does besides closing itself.
    enum Body {
        Nothing,
        /// Validator 3 asks to withdraw: the stake change.
        Withdraw,
        /// A user deploy by `payer(byte)` whose phlo, 500, goes into the staking vault.
        Deploy(u8),
    }

    /// What playing a block left behind, split the way the block path splits it (AUDIT C207): the
    /// writes the block's **own** terms and its boundary made, and the cost-accounting moves the
    /// merge re-derives from the accepted deploys rather than the sidecar carrying them.
    #[derive(Clone)]
    struct Played {
        own: Vec<NativeStoreAction>,
        moves: BTreeMap<Vec<u8>, CostMoves>,
        /// The block's **complete** native effect — `own` plus the cost-accounting writes — which is
        /// what the block's own post-state holds. Only `own` travels in the sidecar; a test that
        /// needs to stand up a descendant's pre-state needs both.
        full: Vec<NativeStoreAction>,
    }

    /// Play block `number` on `pre_state`: its body, then `close_block`. Returns the block's own
    /// native writes and its cost-accounting moves, split as `RSpace::create_checkpoint` splits them
    /// (`begin_cost_accounting` around the system deploys is the whole of the difference).
    async fn play(
        repo: &RhoHistoryRepository,
        pre_state: Blake2b256Hash,
        number: i64,
        fringe: u8,
        body: Body,
    ) -> Played {
        let store = Arc::new(InMemNativeStore::new(
            repo.get_native_reader(pre_state).await,
        ));
        let native = NativeSystemState::new(store.clone());
        let mut moves: BTreeMap<Vec<u8>, CostMoves> = BTreeMap::new();
        // **The block's deploy list, in the order the ordinal names** (#280): the user deploy (when
        // there is one) first, then the block-level system deploy — `close_block` — whose window is
        // opened below. Without these windows a write belongs to no deploy and `from_drain` refuses
        // it, which is the type doing its job rather than a fixture inconvenience.
        let user_ordinal = match body {
            Body::Nothing | Body::Withdraw => None,
            Body::Deploy(_) => Some(0u32),
        };
        let sys_ordinal = if user_ordinal.is_some() { 1u32 } else { 0u32 };
        match body {
            Body::Nothing => {}
            Body::Withdraw => native
                .withdraw(&validator(3), number)
                .await
                .unwrap()
                .unwrap(),
            Body::Deploy(byte) => {
                // The window `play_deploy_with_cost_accounting_once` opens around `pre_charge`,
                // `refund` and `pay_executor`. Here only the charge runs, so the deploy is charged
                // its whole 500 and refunded nothing — which is what the moves below say. The cost
                // window nests inside the deploy's own, exactly as the runtime opens them.
                store.begin_writer(NativeWriter::Deploy(user_ordinal.unwrap()));
                store.begin_writer(NativeWriter::CostAccounting);
                native
                    .pre_charge(&payer(byte).0, nn(500))
                    .await
                    .unwrap()
                    .unwrap();
                store.end_writer();
                store.end_writer();
                moves.insert(
                    vec![number as u8],
                    CostMoves {
                        deployer: payer(byte).1,
                        charge: 500,
                        refund: 0,
                        burned: 500,
                    },
                );
            }
        }
        store.begin_writer(NativeWriter::Deploy(sys_ordinal));
        native
            .close_block(number, Blake2b256Hash::create(&[fringe]), &BTreeMap::new())
            .await
            .unwrap()
            .unwrap();
        store.end_writer();
        let drain = store.drain_native();
        // `by_deploy` is provenance and can legitimately contain several historical writes to the
        // same slot.  The block's actual post-state is the drain's final checkpoint view: exactly one
        // action per slot.  Reconstructing it as `own + cost` would re-introduce duplicate actions.
        let full = drain.all();
        let own: Vec<NativeStoreAction> = drain
            .by_deploy
            .values()
            .flat_map(|actions| actions.iter().cloned())
            .collect();
        Played { own, moves, full }
    }

    fn block_hash(n: u8) -> BlockHash {
        BlockHash::new([n; 32])
    }

    /// A block index with one (close-block) chain, as every block has.
    ///
    /// **The block's own writes ride on that chain** (#280) — that is the whole representation
    /// change: a block has no native set of its own any more, and a chain carries what its own deploys
    /// wrote.
    fn index(n: u8, pre_state: Blake2b256Hash, played: Played) -> BlockIndex {
        BlockIndex {
            block_hash: block_hash(n),
            deploy_chains: vec![Arc::new(DeployChainIndex {
                first_deploy_ordinal: 0,
                native_effects: played.own.clone(),
                host_block: Blake2b256Hash::from_bytes([n; 32]),
                deploys_with_cost: BTreeSet::from([DeployIdWithCost {
                    id: vec![n],
                    cost: 0,
                }]),
                pre_state_hash: pre_state,
                post_state_hash: pre_state,
                event_log_index: EventLogIndex::empty(),
                state_changes: StateChange::empty(),
                cost_moves: played.moves,
                executor: payer(1).1,
            })],
        }
    }

    /// Regression for the finer-grained #280 failure found in review: two deploys in one block write
    /// the same native slot; a concurrent chain conflicts with the **later** deploy for an independent
    /// reason; resolution rejects that later chain; the earlier accepted native value must remain.
    ///
    /// This deliberately drives the real resolver and `NativeRelations::fold`.  A store-only test can
    /// prove provenance was recorded, but not that the merge consumes that provenance correctly.
    #[test]
    fn rejecting_a_later_same_block_writer_keeps_the_earlier_value() {
        let base = Blake2b256Hash::from_bytes([0x11; 32]);
        let slot = Blake2b256Hash::from_bytes([0x44; 32]);
        let host = Blake2b256Hash::from_bytes([0xa0; 32]);
        let other_host = Blake2b256Hash::from_bytes([0xc0; 32]);

        let chain = |host_block: Blake2b256Hash, ordinal: u32, id: u8, value: Option<u8>| {
            Arc::new(DeployChainIndex {
                first_deploy_ordinal: ordinal,
                native_effects: value
                    .map(|v| {
                        vec![NativeStoreAction::Put {
                            prefix: PREFIX_POS,
                            key: slot,
                            value: vec![v],
                        }]
                    })
                    .unwrap_or_default(),
                host_block,
                deploys_with_cost: BTreeSet::from([DeployIdWithCost {
                    id: vec![id],
                    cost: 0,
                }]),
                pre_state_hash: base,
                post_state_hash: base,
                event_log_index: EventLogIndex::empty(),
                state_changes: StateChange::empty(),
                cost_moves: BTreeMap::new(),
                executor: String::new(),
            })
        };

        let earlier = chain(host, 0, 1, Some(1));
        let later = chain(host, 1, 2, Some(2));
        let concurrent = chain(other_host, 0, 3, None);
        let native = NativeRelations {
            ancestry: &BTreeMap::new(),
        };

        assert!(
            native.depends(&later, &earlier),
            "later same-block writer must depend on earlier"
        );
        assert!(!native.depends(&earlier, &later), "dependency is directed");

        // The conflict resolver has rejected `later` (its cascade semantics are pinned separately by
        // `rejection_reason_tests`).  At the merge boundary the accepted set must therefore be able
        // to reconstruct the state from the earlier deploy rather than silently losing the slot.
        let accepted = BTreeSet::from([earlier.clone(), concurrent]);
        let folded = native.fold(&accepted).expect("accepted native writes fold");
        let (_, action) = folded
            .get(&(PREFIX_POS, slot))
            .expect("the earlier native slot remains in the batch");
        assert_eq!(
            action,
            &NativeStoreAction::Put {
                prefix: PREFIX_POS,
                key: slot,
                value: vec![1],
            },
            "rejecting the later writer must reveal the earlier accepted value, not erase the slot"
        );
    }

    /// The same merge, with the outcome rather than the tuple — for the tests that assert on what the
    /// merge *did* (#280).
    async fn merge_with_report(
        repo: &RhoHistoryRepository,
        base: Blake2b256Hash,
        blocks: Vec<BlockIndex>,
    ) -> Result<MergeOutcome, String> {
        let scope = MergeScope {
            final_scope: BTreeSet::new(),
            conflict_scope: blocks.iter().map(|b| b.block_hash).collect(),
            ancestry: BTreeMap::new(),
        };
        let lookup = move |h: BlockHash| {
            let found = blocks.iter().find(|b| b.block_hash == h).cloned();
            async move {
                found
                    .map(Arc::new)
                    .ok_or_else(|| format!("no index for {h:?}"))
            }
        };
        MergeScope::merge(
            &scope,
            base,
            &RejectionsMap::new(),
            repo,
            &lookup,
            DeployChainIndex::deploy_chain_cost,
        )
        .await
    }

    /// A second chain for `block`, as a block that carried a deploy has: **the deploy's own chain**,
    /// which wrote **nothing native** (#280).
    ///
    /// That emptiness is the point of the fixture and it is why this is a function rather than a
    /// clone at each call site: a clone would inherit the first chain's native effects, and a chain
    /// that "carries" its host's boundary writes is exactly the shape the old rule assumed and this
    /// one removes. The ordinal is 1 — the deploy runs before the block's `close_block`.
    fn user_chain(block: &BlockIndex, id: u8) -> Arc<DeployChainIndex> {
        let mut second = (*block.deploy_chains[0]).clone();
        second.deploys_with_cost = BTreeSet::from([DeployIdWithCost {
            id: vec![id],
            cost: 0,
        }]);
        second.native_effects = Vec::new();
        second.first_deploy_ordinal = 1;
        Arc::new(second)
    }

    /// A boundary round's **deploying** block, as the incident's was: its `close_block` chain and,
    /// beside it, the chain of the deploy it carried — the charge on the deploy's chain (which is
    /// where the runtime puts `cost_moves`, keyed by deploy id) and the boundary's writes on the
    /// boundary's, since those are what each deploy actually wrote (#280).
    fn index_with_deploy(n: u8, pre_state: Blake2b256Hash, played: Played, id: u8) -> BlockIndex {
        let block = index(n, pre_state, played.clone());
        let mut boundary = (*block.deploy_chains[0]).clone();
        boundary.cost_moves = BTreeMap::new();
        let mut deploy = (*block.deploy_chains[0]).clone();
        deploy.deploys_with_cost = BTreeSet::from([DeployIdWithCost {
            id: vec![id],
            cost: 500,
        }]);
        deploy.native_effects = Vec::new();
        deploy.first_deploy_ordinal = 1;
        deploy.cost_moves = played.moves;
        BlockIndex {
            block_hash: block.block_hash,
            deploy_chains: vec![Arc::new(boundary), Arc::new(deploy)],
        }
    }

    async fn merge(
        repo: &RhoHistoryRepository,
        base: Blake2b256Hash,
        blocks: Vec<BlockIndex>,
        ancestry: BTreeMap<BlockHash, BTreeSet<BlockHash>>,
    ) -> Result<(Blake2b256Hash, BTreeSet<Vec<u8>>), String> {
        let scope = MergeScope {
            final_scope: BTreeSet::new(),
            conflict_scope: blocks.iter().map(|b| b.block_hash).collect(),
            ancestry,
        };
        let lookup = move |h: BlockHash| {
            let found = blocks.iter().find(|b| b.block_hash == h).cloned();
            async move {
                found
                    .map(Arc::new)
                    .ok_or_else(|| format!("no index for {h:?}"))
            }
        };
        MergeScope::merge(&scope, base, &RejectionsMap::new(), repo, &lookup, |_| 0)
            .await
            .map(|o| (o.state, o.rejected_deploys))
    }

    /// Every native value the writes of `actions` leave behind, read back from `state`.
    async fn read_back(
        repo: &RhoHistoryRepository,
        state: Blake2b256Hash,
        actions: &[NativeStoreAction],
    ) -> Vec<Option<Vec<u8>>> {
        let reader = repo.get_native_reader(state).await;
        let mut out = Vec::new();
        for action in actions {
            let (prefix, key) = action.slot();
            out.push(reader.get_native(prefix, key).await.unwrap());
        }
        out
    }

    fn values(actions: &[NativeStoreAction]) -> Vec<Option<Vec<u8>>> {
        actions
            .iter()
            .map(|a| match a {
                NativeStoreAction::Put { value, .. } => Some(value.clone()),
                NativeStoreAction::Delete { .. } => None,
            })
            .collect()
    }

    /// Two sibling boundary blocks on one pre-state: the merge must neither panic nor error, must
    /// reject exactly one of them, and must leave exactly the other's boundary behind - never a mix.
    async fn siblings_resolve_to_one(a_body: Body, a_fringe: u8, b_body: Body, b_fringe: u8) {
        let (repo, base) = genesis().await;
        let a = play(&repo, base, EPOCH, a_fringe, a_body).await;
        let b = play(&repo, base, EPOCH, b_fringe, b_body).await;
        let (merged, rejected) = merge(
            &repo,
            base,
            vec![index(0xa0, base, a.clone()), index(0xb0, base, b.clone())],
            BTreeMap::new(),
        )
        .await
        .expect("sibling boundary blocks merge (#83)");

        assert_eq!(rejected.len(), 1, "exactly one sibling is rejected");
        let kept = if rejected.contains(&vec![0xa0]) {
            &b
        } else {
            &a
        };
        assert_eq!(
            read_back(&repo, merged, &kept.own).await,
            values(&kept.own),
            "the merged state is the surviving sibling's boundary, whole"
        );
    }

    /// The panic of #83 itself: identical boundaries (same pre-state, same fringe, no stake change).
    /// Equal writes are still two transitions, so one sibling is rejected rather than both kept.
    #[tokio::test]
    async fn identical_sibling_boundaries_merge() {
        siblings_resolve_to_one(Body::Nothing, 1, Body::Nothing, 1).await;
    }

    /// **The shape the merge search is really handed (#117).** Every other statement about the search's
    /// cost in this tree is made against a *chosen* shape (`sdk/tests/merging_scaling.rs` picks a fork;
    /// `sdk/src/dag/merging.rs`'s table picks four). This one is produced by the merge path itself, out
    /// of the fixtures the sibling-boundary tests use, and it answers the question the choice of fix
    /// turns on: **is the relation symmetric?** If it is, the exact output-sensitive rewrite is the
    /// classical maximal-independent-set enumeration and is output-polynomial; if it is not, the
    /// rewrite needs the directed argument instead.
    ///
    /// The assertion is on a process-wide maximum, and it is still deterministic: `MAX_ASYMMETRIC` is
    /// monotone, so it holds every merge this test binary ran, and any test whose real merge produced an
    /// asymmetric pair would raise it above zero regardless of the order the tests ran in.
    /// The width distribution's boundaries — the half of C182 that can be tested without racing the
    /// process-wide counters, since every other test in this binary that runs a merge updates them.
    #[test]
    fn the_width_buckets_are_inclusive_at_their_edges() {
        use crate::merging::search_census::{bucket_index, EXPANDED_EDGES, WIDTH_EDGES};
        assert_eq!(bucket_index(&WIDTH_EDGES, 0), 0);
        assert_eq!(bucket_index(&WIDTH_EDGES, 16), 0, "the edge is inclusive");
        assert_eq!(bucket_index(&WIDTH_EDGES, 17), 1);
        assert_eq!(bucket_index(&WIDTH_EDGES, 32), 1);
        assert_eq!(bucket_index(&WIDTH_EDGES, 33), 2);
        assert_eq!(bucket_index(&WIDTH_EDGES, 64), 2);
        assert_eq!(bucket_index(&WIDTH_EDGES, 65), 3);
        assert_eq!(bucket_index(&WIDTH_EDGES, 128), 3);
        assert_eq!(
            bucket_index(&WIDTH_EDGES, 129),
            4,
            "past the last edge is the open-ended bucket"
        );
        assert_eq!(bucket_index(&WIDTH_EDGES, 1_000_000), 4);
        // And the cost's edges, which are logarithmic — a merge that expanded a million states is the
        // order the heap profile reached, and it must not land in the same bucket as one that did a
        // thousand.
        assert_eq!(bucket_index(&EXPANDED_EDGES, 1_000), 0);
        assert_eq!(bucket_index(&EXPANDED_EDGES, 1_001), 1);
        assert_eq!(bucket_index(&EXPANDED_EDGES, 1_000_000), 3);
        assert_eq!(
            bucket_index(&EXPANDED_EDGES, 1_663_395),
            4,
            "the census run's worst merge"
        );
    }

    #[tokio::test]
    async fn the_merge_search_sees_the_shape_the_merge_builds() {
        siblings_resolve_to_one(Body::Nothing, 1, Body::Nothing, 1).await;
        eprintln!("{}", search_census::summary());
        assert_eq!(
            search_census::MAX_ASYMMETRIC.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "the conflict relation the merge hands the search is not symmetric, so the exact \
             rewrite cannot be the symmetric (maximal-independent-set) case without more work"
        );
    }

    /// The proposers saw different finalised fringes, so their epoch seeds differ.
    #[tokio::test]
    async fn sibling_boundaries_with_different_fringes_merge() {
        siblings_resolve_to_one(Body::Nothing, 1, Body::Nothing, 2).await;
    }

    /// The stake changed in one sibling only (a withdrawal staged in the boundary block).
    #[tokio::test]
    async fn sibling_boundaries_with_a_stake_change_merge() {
        siblings_resolve_to_one(Body::Withdraw, 1, Body::Nothing, 1).await;
    }

    /// One sibling carried a deploy, so its reward pot - and the committed rewards - differ.
    #[tokio::test]
    async fn sibling_boundaries_with_different_pots_merge() {
        siblings_resolve_to_one(Body::Deploy(9), 1, Body::Nothing, 1).await;
    }

    /// **The invariant that made `_do_not_destroy_rev` the name of this test, and still does.**
    ///
    /// Two concurrent blocks off a boundary, each charging a different deployer the same 500 of
    /// phlo: both left `pos:vault` at `base + 500`. Keeping **one** value for both — which is what
    /// de-duplicating equal writes does — credits the vault once for two debits and destroys 500
    /// REV. When that was the only recourse the merge had, the test pinned the resolution that
    /// avoided it (reject one block, and with it both debits and both credits).
    ///
    /// **Since AUDIT C207 the merge does better than choosing:** each charge is a *move* the merge
    /// re-applies per accepted deploy, so the two compose. The test's claim is unchanged — REV is
    /// conserved — and it now has a sharper half: the merge keeps **both** blocks and the vault
    /// holds **both** charges, which a "keep one" rule fails on the same assertion REV does.
    #[tokio::test]
    async fn equal_concurrent_vault_writes_do_not_destroy_rev() {
        let (repo, base) = genesis().await;
        let x = play(&repo, base, EPOCH - 3, 1, Body::Deploy(8)).await;
        let y = play(&repo, base, EPOCH - 3, 1, Body::Deploy(9)).await;
        let (merged, rejected) = merge(
            &repo,
            base,
            vec![index(0xa0, base, x), index(0xb0, base, y)],
            BTreeMap::new(),
        )
        .await
        .expect("the merge");
        assert!(
            rejected.is_empty(),
            "the two charges are moves, not competing snapshots, so neither block is rejected"
        );

        let read = |state: Blake2b256Hash| {
            let repo = repo.clone();
            async move {
                let native = NativeSystemState::new(Arc::new(InMemNativeStore::new(
                    repo.get_native_reader(state).await,
                )));
                let mut vaults = Vec::new();
                for byte in [8, 9] {
                    vaults.push(
                        native
                            .vault_balance(&payer(byte).1)
                            .await
                            .unwrap()
                            .map_or(0, i64::from),
                    );
                }
                (i64::from(native.pos_vault_balance().await.unwrap()), vaults)
            }
        };
        let (merged_vault, merged_payers) = read(merged).await;
        let (base_vault, base_payers) = read(base).await;

        assert_eq!(
            merged_vault,
            base_vault + 1000,
            "the staking vault holds **both** charges: paying it once for two debits is the REV this \
             test exists to keep"
        );
        let paid = |base: i64| base - 500;
        assert_eq!(
            merged_payers,
            base_payers.iter().map(|b| paid(*b)).collect::<Vec<_>>(),
            "and each payer is out exactly its own charge — not one payer twice"
        );
        assert_eq!(
            merged_vault + merged_payers.iter().sum::<i64>(),
            base_vault + base_payers.iter().sum::<i64>(),
            "REV is conserved"
        );
    }

    /// A block with a user deploy has at least two chains (the deploy's and the close-block's), and
    /// its native writes make them one unit. The merge must terminate on such a block: the
    /// dependency map is walked by traversals that do not tolerate a cycle.
    #[tokio::test]
    async fn a_native_block_with_two_chains_merges() {
        let (repo, base) = genesis().await;
        let x = play(&repo, base, EPOCH - 3, 1, Body::Deploy(8)).await;
        let block = index_with_deploy(0xa0, base, x, 0xa1);
        let merged = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            merge(&repo, base, vec![block], BTreeMap::new()),
        )
        .await
        .expect("the merge terminates")
        .expect("the merge");
        assert!(merged.1.is_empty(), "nothing conflicts with a lone block");
    }

    /// A rejected native block loses every chain, including a second one: its user deploy's
    /// effects go with the boundary it rode in on.
    #[tokio::test]
    async fn a_rejected_boundary_chain_does_not_take_its_blocks_other_chains() {
        let (repo, base) = genesis().await;
        let a = play(&repo, base, EPOCH, 1, Body::Nothing).await;
        let b = play(&repo, base, EPOCH, 2, Body::Nothing).await;
        let with_second_chain = |mut block: BlockIndex, id: u8| {
            block.deploy_chains.push(user_chain(&block, id));
            block
        };
        let (_, rejected) = merge(
            &repo,
            base,
            vec![
                with_second_chain(index(0xa0, base, a), 0xa1),
                with_second_chain(index(0xb0, base, b), 0xb1),
            ],
            BTreeMap::new(),
        )
        .await
        .expect("sibling boundary blocks with two chains each merge");
        // **This test used to assert the opposite, and the inversion is the fix** (#280). It read
        // "a rejected native block loses every chain, including a second one: its user deploy's
        // effects go with the boundary it rode in on" — the defect written down as intent, which is
        // why nothing caught it: the boundary chains are the ones that contend, and the user chains
        // wrote nothing contested, so they cannot be conflict partners at all now.
        assert_eq!(
            rejected.len(),
            1,
            "exactly one **boundary** chain is rejected: {rejected:?}"
        );
        assert!(
            rejected.contains(&vec![0xa0]) || rejected.contains(&vec![0xb0]),
            "and it is a boundary chain: {rejected:?}"
        );
        assert!(
            !rejected.contains(&vec![0xa1]) && !rejected.contains(&vec![0xb1]),
            "while neither block's user chain is rejected — it wrote no contended slot, so it has no \
             business in the rejection set: {rejected:?}"
        );
    }

    /// The same two charges, but the second block has seen the first: its vault balance already
    /// includes the first charge, so both blocks are kept and the descendant's value is the state.
    #[tokio::test]
    async fn an_ancestor_and_descendant_writing_one_key_both_merge() {
        let (repo, base) = genesis().await;
        let x = play(&repo, base, EPOCH - 3, 1, Body::Deploy(8)).await;
        let after_x = repo
            .reset(base)
            .await
            .unwrap()
            .do_checkpoint_with_native(&[], &x.full)
            .await
            .unwrap()
            .root();
        let y = play(&repo, after_x, EPOCH - 2, 1, Body::Deploy(9)).await;
        let ancestry = BTreeMap::from([(block_hash(0xb0), BTreeSet::from([block_hash(0xa0)]))]);
        let (merged, rejected) = merge(
            &repo,
            base,
            vec![index(0xa0, base, x), index(0xb0, after_x, y)],
            ancestry,
        )
        .await
        .expect("the merge");
        assert!(rejected.is_empty(), "a chain of writers is not a conflict");

        let native = NativeSystemState::new(Arc::new(InMemNativeStore::new(
            repo.get_native_reader(merged).await,
        )));
        let base_native = NativeSystemState::new(Arc::new(InMemNativeStore::new(
            repo.get_native_reader(base).await,
        )));
        assert_eq!(
            i64::from(native.pos_vault_balance().await.unwrap()),
            i64::from(base_native.pos_vault_balance().await.unwrap()) + 1000,
            "both charges reach the staking vault"
        );
    }
    /// **C207's falsifier, inverted by the fix: a cost-accounted block keeps its *own* native writes
    /// through a merge with a concurrent sibling.**
    ///
    /// The defect this replaces was in process the whole time and looked like nothing: `pre_charge`,
    /// `refund` and `pay_executor` run for **every user deploy** and all three write `pos:vault`, so
    /// on a network with concurrent proposers **every** user-deploy block overlapped every sibling on
    /// that one slot. Two concurrent absolute snapshots of one slot cannot be composed, so the merge
    /// rejected one block **whole** — and the deploy's *own* writes (a delegation, a bond, a `trust`)
    /// died with it. The live ablation is in `spec/audit/evidence/c207-merge-loses-native-writes.md`:
    /// two validators with autopropose lose the write, one validator, or two without autopropose,
    /// keep it.
    ///
    /// **A system deploy never had the symptom**, and that asymmetry is why nothing looked wrong: a
    /// boundary writes no `pos:vault`, and two sibling boundaries compute identical values from one
    /// pre-state, so rejecting one leaves the other's equal write in place while the epoch machinery
    /// keeps working.
    ///
    /// The fixture is the network's shape rather than a hand-written overlap: two blocks at one
    /// height, **each carrying a user deploy** (so each charges a payer and the staking vault), and
    /// one of them additionally making a native write of its own — the stand-in for `pos:delegations`.
    /// The unique write is put on the *first* block and then on the *second*, because the merge's
    /// surviving host is decided by the DAG and the hash rather than by the values.
    ///
    /// Both halves are asserted, because either alone is satisfiable by a bad fix: the merge keeps
    /// **both** blocks (so the write survives) *and* the merged vault is the **sum** of the two
    /// charges (so nothing was minted or destroyed by composing them). The control is the same block
    /// merged alone, which shows the write survives on its own too — so a failure is the concurrency,
    /// not the merge's application.
    #[tokio::test]
    async fn a_cost_accounted_block_keeps_its_own_native_writes_through_the_merge() {
        let (repo, base) = genesis().await;
        // Stands in for `pos:delegations`: a leaf written by one block's own terms and no other's.
        let unique = Blake2b256Hash::from_bytes([10; 32]);
        let with_unique_write = |played: &mut Played| {
            played.own.push(NativeStoreAction::Put {
                prefix: PREFIX_POS,
                key: unique,
                value: vec![40],
            });
        };

        // --- the control: the writing block merged alone keeps its write ---
        let mut alone = play(&repo, base, EPOCH - 3, 1, Body::Deploy(8)).await;
        with_unique_write(&mut alone);
        let (merged_alone, rejected_alone) =
            merge(&repo, base, vec![index(0xa0, base, alone)], BTreeMap::new())
                .await
                .expect("a single block merges");
        assert!(
            rejected_alone.is_empty(),
            "nothing to conflict with, so nothing is rejected"
        );
        assert_eq!(
            repo.get_native_reader(merged_alone)
                .await
                .get_native(PREFIX_POS, unique)
                .await
                .expect("a readable native leaf"),
            Some(vec![40]),
            "**control**: merged alone, the unique write survives — so a loss in the concurrent case \
             is the concurrency and not the merge's application"
        );

        // --- C207: the same block, concurrent with another cost-accounted block ---
        //
        // Both orderings, because which host the DAG keeps is hash-determined rather than
        // value-determined (`two_branch_blocks_writing_one_native_slot_do_not_panic_the_merge`
        // asserts exactly that). Running one ordering would let the test pass by luck of which host
        // won.
        let base_vault = {
            let native = NativeSystemState::new(Arc::new(InMemNativeStore::new(
                repo.get_native_reader(base).await,
            )));
            i64::from(native.pos_vault_balance().await.unwrap())
        };
        for unique_on in [0xa0u8, 0xb0u8] {
            let mut first = play(&repo, base, EPOCH - 3, 1, Body::Deploy(8)).await;
            let mut second = play(&repo, base, EPOCH - 3, 1, Body::Deploy(9)).await;
            if unique_on == 0xa0 {
                with_unique_write(&mut first);
            } else {
                with_unique_write(&mut second);
            }
            let (merged, rejected) = merge(
                &repo,
                base,
                vec![index(0xa0, base, first), index(0xb0, base, second)],
                BTreeMap::new(),
            )
            .await
            .expect("two concurrent cost-accounted blocks merge");

            assert!(
                rejected.is_empty(),
                "each block's cost accounting is now a set of moves the merge re-applies, so two \
                 concurrent charges are not competing snapshots of one slot and neither block loses \
                 its own writes with a whole-block rejection"
            );
            let reader = repo.get_native_reader(merged).await;
            assert_eq!(
                reader
                    .get_native(PREFIX_POS, unique)
                    .await
                    .expect("a readable native leaf"),
                Some(vec![40]),
                "**C207, fixed**: the block carrying the unique write is kept, and so is the write"
            );
            let native = NativeSystemState::new(Arc::new(InMemNativeStore::new(
                repo.get_native_reader(merged).await,
            )));
            assert_eq!(
                i64::from(native.pos_vault_balance().await.unwrap()),
                base_vault + 1000,
                "and the vault holds the **sum** of the two charges: keeping both blocks must not \
                 become paying the vault once for two debits, which is the REV this whole path is \
                 answerable for"
            );
        }
    }

    /// `merge`, but with the rejection key the **production** path uses.
    ///
    /// `merge` above passes `|_| 0`, which is law 17a's cost component discarded: every option costs
    /// the same, so the resolution is decided by size and then by the sorted chain set alone. That is
    /// right for the sibling-boundary tests, which are about *values*, and it is exactly wrong for
    /// #280, where the whole question is whether the option that keeps a user chain loses to the one
    /// that keeps three zero-cost boundary chains. Production passes
    /// `DeployChainIndex::deploy_chain_cost` (`casper/src/multi_parent_casper.rs`), and so does this.
    async fn merge_by_cost(
        repo: &RhoHistoryRepository,
        base: Blake2b256Hash,
        blocks: Vec<BlockIndex>,
    ) -> Result<(Blake2b256Hash, BTreeSet<Vec<u8>>), String> {
        let scope = MergeScope {
            final_scope: BTreeSet::new(),
            conflict_scope: blocks.iter().map(|b| b.block_hash).collect(),
            ancestry: BTreeMap::new(),
        };
        let lookup = move |h: BlockHash| {
            let found = blocks.iter().find(|b| b.block_hash == h).cloned();
            async move {
                found
                    .map(Arc::new)
                    .ok_or_else(|| format!("no index for {h:?}"))
            }
        };
        MergeScope::merge(
            &scope,
            base,
            &RejectionsMap::new(),
            repo,
            &lookup,
            DeployChainIndex::deploy_chain_cost,
        )
        .await
        .map(|o| (o.state, o.rejected_deploys))
    }

    /// **The merge's report is the ledger the live incident lacked** (#280).
    ///
    /// On the net, a merge rejected a boundary chain and with it a user deploy; the deploy's id went
    /// into the block's `rejectedDeploys`, its own status stayed `ProcessedWithSuccess`, and **nothing
    /// anywhere said what the rejection cost** — no counter, no line, no record of the effects that
    /// went unwritten. So this asserts the report against a rejection that really happens: which slots
    /// were dropped and who wrote them, that a rejected cost-accounted chain is counted, that the
    /// winner of each surviving slot is named, and — the two that must never fire — that every
    /// rejection has a reason (I1) and that every kept chain's write reached the batch (I2).
    #[tokio::test]
    async fn the_merge_report_names_every_dropped_native_write() {
        let (repo, base) = genesis().await;
        let a = play(&repo, base, EPOCH, 1, Body::Nothing).await;
        let b = play(&repo, base, EPOCH, 2, Body::Nothing).await;
        let a_slots = a.own.len();
        let report = merge_with_report(
            &repo,
            base,
            vec![index(0xa0, base, a), index(0xb0, base, b)],
        )
        .await
        .expect("sibling boundary blocks merge")
        .report;

        assert!(!report.is_quiet(), "a rejection is never quiet: {report:?}");
        assert_eq!(report.conflict_chains, 2, "two chains were in scope");
        assert_eq!(report.kept_chains, 1, "one survived");
        assert_eq!(report.rejected_chains, 1, "and one was rejected");
        assert_eq!(
            report.dropped_native_slots.len(),
            a_slots,
            "**every** native slot the rejected chain would have written is named, with its host — \
             this is the list the incident could not produce: {report:?}"
        );
        let in_scope = [
            Blake2b256Hash::from_bytes([0xa0; 32]),
            Blake2b256Hash::from_bytes([0xb0; 32]),
        ];
        assert!(
            !report.native_writer_of_slot.is_empty(),
            "a boundary round writes slots, so the survivors are named: {report:?}"
        );
        assert!(
            report
                .native_writer_of_slot
                .values()
                .all(|h| in_scope.contains(h)),
            "every surviving slot names **one of the blocks in scope** as its writer — the map the \
             incident had no way to read: {:?}",
            report.native_writer_of_slot
        );
        assert!(
            report.rejections_without_a_conflict.is_empty(),
            "**I1**: every rejected chain conflicts with a kept chain or depends on a rejected one — \
             a rejection with neither is a chain dropped for no reason of its own: {report:?}"
        );
        assert!(
            report.unapplied_kept_writes.is_empty(),
            "**I2**: every slot a kept chain wrote has its action in the merged batch: {report:?}"
        );
        assert!(
            report.describe().contains("chains in scope"),
            "and the line an operator gets names them: {}",
            report.describe()
        );
    }

    /// **#280: an epoch-boundary round loses the deploy that rode in on a contending block.**
    ///
    /// C207 removed the conflict that was *universal* — cost accounting left the block's native set,
    /// so a user-deploy block stopped overlapping every concurrent sibling on `pos:vault`. What it
    /// did not remove is the conflict C207's own note calls safe, in as many words: *"a boundary
    /// writes no `pos:vault`, and two sibling boundaries compute identical values from one pre-state,
    /// so rejecting one leaves the other's equal write in place while the epoch machinery keeps
    /// working."* At an **epoch boundary every** block runs `close_block`, so every block's native set
    /// holds the PoS slots the boundary writes and **all of them contend**. Two things then compose
    /// badly: the conflict relation is over the **host block** rather than the chain
    /// (`NativeRelations::conflicting` compares host key-sets), and the rejection rule is over the host
    /// block too (`reject_whole_blocks`) — so resolving the contention rejects a block *whole*, and a
    /// user deploy that rode in on it loses everything, including effects that touched no contended
    /// slot at all.
    ///
    /// **The fixture is the incident's shape, not a hand-written overlap**: four blocks at the boundary
    /// height, three of them empty and one carrying a user deploy, which is exactly the round the live
    /// chain ran at height 100 (`spec/audit/evidence/n280-merge-loses-a-write-results.md`). The block
    /// that carries the deploy gets a **second chain** for it, the way a real block does, and the
    /// question is whether that chain survives a contention it took no part in.
    ///
    /// **The assertion is the deploy's own effect, not a chain id**, because a chain id is the fix's
    /// vocabulary and an effect is the protocol's: `pre_charge` moves 500 out of the payer's vault, so
    /// at the merged root the payer is out exactly 500 — however the boundary contention was resolved,
    /// and whichever of the four blocks carried the deploy (which host the DAG keeps is hash-determined
    /// rather than value-determined, so one ordering could pass by luck).
    #[tokio::test]
    async fn a_boundary_round_does_not_lose_a_contained_deploys_charge() {
        let (repo, base) = genesis().await;
        let balance = |state: Blake2b256Hash| {
            let address = payer(8).1.clone();
            let repo = &repo;
            async move {
                let native = NativeSystemState::new(Arc::new(InMemNativeStore::new(
                    repo.get_native_reader(state).await,
                )));
                i64::from(native.vault_balance(&address).await.unwrap().unwrap())
            }
        };
        let base_balance = balance(base).await;
        assert_eq!(base_balance, 1000, "the fixture funds the payer");

        for deploying in [0xa0u8, 0xb0, 0xc0, 0xd0] {
            let mut blocks = Vec::new();
            for n in [0xa0u8, 0xb0, 0xc0, 0xd0] {
                let body = if n == deploying {
                    Body::Deploy(8)
                } else {
                    Body::Nothing
                };
                let played = play(&repo, base, EPOCH, 1, body).await;
                blocks.push(if n == deploying {
                    // The deploy's own chain, beside the boundary's: a real block has one per deploy,
                    // and **it is the chain that carries no contended slot** — which is the whole of
                    // why the fix works and why the fixture has to get it right rather than clone the
                    // boundary chain.
                    index_with_deploy(n, base, played, n + 1)
                } else {
                    index(n, base, played)
                });
            }

            let (merged, rejected) = merge_by_cost(&repo, base, blocks)
                .await
                .unwrap_or_else(|e| {
                    panic!("the boundary round merges (deploy on {deploying:#x}): {e}")
                });
            assert_eq!(
                balance(merged).await,
                base_balance - 500,
                "the deploy's charge must be applied at the merged root whichever block carried it \
                 (deploy on {deploying:#x}; the merge rejected {rejected:?})"
            );
        }
    }
}

#[cfg(test)]
mod index_stats_tests {
    use super::{BlockIndex, IndexStats};

    /// The progress line must name every counter, because that line is the only place the numbers
    /// become visible (#60) - a summary that silently dropped one would hide it again.
    #[test]
    fn the_summary_names_every_counter() {
        let stats = IndexStats {
            calls: 3,
            replay_fallbacks: 1,
            replay_millis: 42,
            replay_save_failures: 1,
            cache_len: 3,
            cache_pruned: 2,
            cache_capacity_evicted: 4,
        };
        let line = stats.summary();
        for needle in [
            "3 blocks indexed",
            "1 replay fallbacks",
            "42 ms",
            "1 not persisted",
            "index cache 3 entries",
            "2 pruned",
            "4 capacity evicted",
        ] {
            assert!(
                line.contains(needle),
                "summary {line:?} must name {needle:?}"
            );
        }
    }

    /// `read` is a snapshot of process-wide counters: a second read agrees with the first, which is
    /// the property the node's progress line relies on.
    #[test]
    fn read_reports_the_live_counters() {
        let before = IndexStats::read().calls;
        // `prune_cache` on an empty list removes nothing, so it must not move the counters.
        BlockIndex::prune_cache(&[]);
        let after = IndexStats::read();
        assert_eq!(after.calls, before);
        assert!(after.summary().contains("blocks indexed"));
    }

    /// Regress a local-view-dependent merge scope after a partition.
    ///
    /// The same parent/merge fringe has the same causal ancestors on both nodes,
    /// but one node may hold a later sibling child of the final fringe that is
    /// absent from the parent's ancestry. That unrelated child can have an older
    /// fringe. Reading the global child map without a causal bound changes
    /// prune_fringe and, for a nonempty seen closure, final_scope itself.
    /// This fixture gives every message a real seen closure (including itself);
    /// empty seen sets masked the divergence in the earlier version of this test.
    ///
    /// The raw global prune boundaries below MUST differ for the fixture to be
    /// meaningful. Scope calculation must ignore the noncausal child and agree.
    #[test]
    fn the_merge_scope_does_not_depend_on_which_children_this_node_holds() {
        use std::collections::{BTreeMap, BTreeSet};
        use std::sync::Arc;

        use rchain_block_storage::dag::finalizer::Message;
        use rchain_models::block_hash::BlockHash;
        use rchain_models::validator::Validator;
        use rchain_shared::refined::{BlockHeight, SeqNum};

        use crate::merging::MergeScope;

        let v = |n: u8| Validator::new([n; 65]);
        let h = |n: u8| BlockHash::new([n; 32]);
        let msg = |id: BlockHash,
                   sender: Validator,
                   height: i64,
                   seq: i64,
                   parents: Vec<BlockHash>,
                   fringe: Vec<BlockHash>,
                   seen: Vec<BlockHash>| Message {
            id,
            height: BlockHeight::try_from(height).unwrap(),
            sender,
            sender_seq: SeqNum::try_from(seq).unwrap(),
            bonds_map: BTreeMap::new(),
            parents: parents.into_iter().collect(),
            fringe: fringe.into_iter().collect(),
            seen: Arc::new(seen.into_iter().collect()),
        };

        // G is the genesis-ish base; F is a block both nodes have just finalised; C1 and C2 are two
        // blocks that justify F — the unfinalised tip. C1 carries F in its fringe; C2 is further back.
        let g = msg(h(1), v(1), 0, 0, vec![], vec![h(1)], vec![h(1)]);
        let f = msg(h(2), v(1), 3, 1, vec![h(1)], vec![h(1)], vec![h(1), h(2)]);
        let c1 = msg(
            h(3),
            v(1),
            4,
            2,
            vec![h(2)],
            vec![h(2)],
            vec![h(1), h(2), h(3)],
        );
        let c2 = msg(
            h(4),
            v(2),
            4,
            0,
            vec![h(2)],
            vec![h(1)],
            vec![h(1), h(2), h(4)],
        );

        let dag_data: BTreeMap<BlockHash, Message<BlockHash, Validator>> = [
            (g.id, g.clone()),
            (f.id, f.clone()),
            (c1.id, c1.clone()),
            (c2.id, c2.clone()),
        ]
        .into_iter()
        .collect();

        // Node A has only C1 as a child of F; node B has also received C2.
        let child_map_a: BTreeMap<BlockHash, BTreeSet<BlockHash>> = BTreeMap::from([
            (h(1), BTreeSet::from([h(2)])),
            (h(2), BTreeSet::from([h(3)])),
        ]);
        let child_map_b: BTreeMap<BlockHash, BTreeSet<BlockHash>> = BTreeMap::from([
            (h(1), BTreeSet::from([h(2)])),
            (h(2), BTreeSet::from([h(3), h(4)])),
        ]);

        let merge_fringe = BTreeSet::from([c1.id]);
        let final_fringe = BTreeSet::from([f.id]);

        // **The probe must be able to fail.** If both nodes derive the same prune fringe the test says
        // nothing about the read, so the two fringes are asserted apart first — a check that the
        // construction really does differ in the thing under test.
        let prune_a = rchain_block_storage::dag::message_map::prune_fringe(
            &dag_data,
            &final_fringe,
            &child_map_a,
        );
        let prune_b = rchain_block_storage::dag::message_map::prune_fringe(
            &dag_data,
            &final_fringe,
            &child_map_b,
        );
        assert_ne!(
            prune_a, prune_b,
            "the probe is degenerate: both nodes derive the same prune fringe {prune_a:?}, so it \
             cannot test whether the read matters"
        );

        let (scope_a, _) =
            MergeScope::from_dag(&merge_fringe, &final_fringe, &child_map_a, &dag_data).unwrap();
        let (scope_b, _) =
            MergeScope::from_dag(&merge_fringe, &final_fringe, &child_map_b, &dag_data).unwrap();

        assert_eq!(
            scope_a.final_scope,
            BTreeSet::new(),
            "the causal child keeps the final scope empty"
        );
        assert_eq!(scope_a.conflict_scope, BTreeSet::from([c1.id]));
        assert_eq!(
            scope_a, scope_b,
            "law 17a: the same justifications must give the same merge scope. Node A holds only C1 \
             below F; node B also holds C2. A's scope is {:?}; B's is {:?} — the difference is \
             an unrelated later child must not influence causal pruning.",
            scope_a, scope_b
        );
    }
}
