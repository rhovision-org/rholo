//! Block DAG key-value storage (port of `casper/dag/BlockDagKeyValueStorage.scala`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use rchain_block_storage::dag::dag_storage::{
    BlockDagStorage, DeployId, DeployIdPermit, DeployPermit, DeployerLookup, HaltRecord,
    HealthCounters, ResetMarker, HALT_RECORD_KEY, HEALTH_COUNTERS_KEY, RESET_MARKER_KEY,
};

use rchain_block_storage::dag::finalizer::Message;
use rchain_block_storage::dag::message_map;
use rchain_block_storage::dag::message_state::DagMessageState;
use rchain_block_storage::dag::representation::DagRepresentation;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::casper::protocol::casper_message::{BlockMessage, SignedDeployData};
use rchain_models::validator::Validator;
use rchain_shared::lock::Unpoison;
use rchain_shared::metrics::{Metrics, MetricsNop, Source};
use rchain_shared::rate_limiter::KeyedRateLimiter;
use rchain_shared::refined::{BlockHeight, SeqNum};
use rchain_shared::typed_store::KeyValueTypedStore;

use crate::block_metadata_store::BlockMetadataStore;
use crate::merging::BlockIndex;

/// Upper bound on the number of distinct deploys held in the pending pool. Reached by the
/// `add_deploy` chokepoint so a remote flood cannot exhaust the deploy store (documented Scala
/// deviation — the Scala pool is unbounded).
pub const MAX_POOLED_DEPLOYS: usize = 10_000;

/// **Deploys admitted into the pool per deployer key per second.**
///
/// The pool bound above is a property of the *node*; this one is a property of the *submitter*, and
/// they answer different attacks. With only the node-wide count, one identity's burst fills the pool
/// and every other identity's deploy is refused — the pool was correctly bounded and the wrong
/// counterparty paid for it. The subject here is the deployer public key, which is the only identity
/// the node can actually hold a submitter to: the deploy carries it, and every ingress — the external
/// gRPC service, the HTTP routes, the faucet — funnels through `add_deploy`.
///
/// Documented deviation, as above: the Scala node bounds neither.
pub const MAX_DEPLOYS_PER_DEPLOYER_PER_SEC: u64 = 100;

/// Distinct deployer keys the admission limiter will track at once. The map is itself a resource an
/// unauthenticated caller would otherwise size, so it is capped; at the cap a new key is refused
/// rather than admitted (`KeyedRateLimiter`).
pub const MAX_TRACKED_DEPLOYERS: usize = 4_096;

/// **How many of the pool's slots one deployer's deploys may hold at once.**
///
/// The rate bound above says how *fast* a deployer may be admitted; this says how much of the pool
/// it may end up owning. They answer different attacks, and the pool's own cap answers neither on its
/// own: with only `MAX_POOLED_DEPLOYS`, one identity that paces itself under the rate limit fills all
/// ten thousand slots in a hundred seconds and every other identity's deploy is then refused by a
/// full pool — the bound held, and the wrong counterparty paid for it (W-N2-04, W-N2-10). A tenth of
/// the pool is no limit on any honest deployer, whose deploys leave the pool as blocks include them.
///
/// A deployer's deploys leave the pool on expiry (`Self::expire_deploys`) and on inclusion in a block
/// (`Self::insert`), and both decrement the share — a share that never came back would be a permanent
/// lockout, which is the shape this is most likely to be wrong in.
pub const MAX_POOLED_DEPLOYS_PER_DEPLOYER: usize = 1_000;

/// **The pool's term-byte budget** (B36, the same row as the count bound above).
///
/// The count bound treats a 1 KiB deploy and a 64 KiB deploy as one place each; the host pays for
/// bytes. Ten thousand maximal terms is 640 MiB — more than the 1 GiB host this programme's top event
/// names should ever spend on a pending pool at once — and the ingress is unauthenticated, so the
/// count alone lets a caller choose the pool's memory by sending large terms. 64 MiB is reached only
/// when the average pooled term is above ~6.4 KiB, well above ordinary traffic, so the byte bound
/// bites on the shape the attack uses and not on volume alone.
///
/// Term bytes are the proxy, and the bound says so: they are the work the ingress asks the node to
/// hold and parse. The stored row, the signature and the pool's index carry a bounded overhead on
/// top of the term, so this is not an RSS figure.
pub const MAX_POOLED_DEPLOY_BYTES: usize = 64 * 1024 * 1024;

/// Build a `Message` from a `BlockMetadata` given the current message map (port of
/// `BlockDagKeyValueStorage.messageFromBlockMetadata`). Returns `None` when a justification is
/// absent from `msg_map` (the Scala `Map.apply` throws).
pub fn message_from_block_metadata(
    block: &BlockMetadata,
    msg_map: &BTreeMap<BlockHash, Message<BlockHash, Validator>>,
) -> Option<Message<BlockHash, Validator>> {
    let mut seen: BTreeSet<BlockHash> = BTreeSet::new();
    for p in &block.justifications {
        seen.extend(msg_map.get(p)?.seen.iter().copied());
    }
    seen.insert(block.block_hash);
    Some(Message {
        id: block.block_hash,
        height: block.block_num,
        sender: block.sender,
        sender_seq: block.seq_num,
        bonds_map: block.bonds_map.clone(),
        parents: block.justifications.clone(),
        fringe: block.fringe.clone(),
        seen: Arc::new(seen),
    })
}

/// Count a deploy store by deployer and total its term bytes — the starting point for
/// [`MAX_POOLED_DEPLOYS_PER_DEPLOYER`] and [`MAX_POOLED_DEPLOY_BYTES`].
///
/// A node that restarts with deploys already pooled has to start from what is stored rather than
/// from zero, or every deployer's share would be handed out a second time on top of what it holds —
/// and the byte budget would admit a full pool's worth again on top of the bytes already there. One
/// pass, because the two are read from the same rows and a second scan would be the same work twice.
async fn pooled_accounting(
    deploy_store: &Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>>,
) -> Result<(std::collections::HashMap<Vec<u8>, usize>, usize), String> {
    let mut counts: std::collections::HashMap<Vec<u8>, usize> = std::collections::HashMap::new();
    let mut term_bytes = 0usize;
    for deploy in deploy_store.to_map().await?.into_values() {
        *counts.entry(deploy.deployer).or_insert(0) += 1;
        term_bytes = term_bytes.saturating_add(deploy.data.term.len());
    }
    Ok((counts, term_bytes))
}

/// Return one place to `deployer`'s share, dropping the entry at zero.
///
/// Delegates to the shared implementation, which is also what `DeployPermit`'s `Drop` uses — one
/// implementation, because the failure the accounting has to avoid (a missed release, which is a
/// permanent lockout) is the same in both.
fn release_deployer_slot(
    pooled_by_deployer: &std::sync::Mutex<std::collections::HashMap<Vec<u8>, usize>>,
    deployer: &[u8],
) {
    rchain_block_storage::dag::dag_storage::release_deployer_share(pooled_by_deployer, deployer);
}

/// The concrete block DAG storage (port of `BlockDagKeyValueStorage`). Fringe pruning (the
/// `BlockIndex` cache) and deploy-pool expiry run on finalization.
pub struct BlockDagKeyValueStorage {
    representation: tokio::sync::RwLock<Arc<DagRepresentation>>,
    lock: tokio::sync::Mutex<()>,
    /// **Held across the pool's capacity check and its write** (the C113 class, one bound over).
    ///
    /// `MAX_POOLED_DEPLOYS` is a count of distinct deploys, so it has to be enforced on a count that
    /// cannot change between the read and the insert. As two independent acquisitions of the store's
    /// own lock it could: every concurrent caller read a count below the cap and every one of them
    /// inserted, so the pool overshot the cap by the concurrency width — and this is the
    /// unauthenticated ingress, so the width is whatever the caller makes it. One guard over both
    /// makes the bound the bound it claims to be.
    deploy_admission: tokio::sync::Mutex<()>,
    /// **Deployer key → its own admission bucket** (see [`MAX_DEPLOYS_PER_DEPLOYER_PER_SEC`]).
    ///
    /// One node-wide bucket made the bound's subject the node, so a single identity could spend every
    /// other identity's allowance. The key is the deploy's signer, taken at this chokepoint because
    /// this is the first place it exists: the gRPC interceptor sees a request with no body and so
    /// cannot know who is asking, and each ingress keeps its own limiter, so none of those is the
    /// node-wide per-identity bound.
    deploy_rate: KeyedRateLimiter,
    /// **Deployer key → how many of the pool's slots its deploys occupy** (see
    /// [`MAX_POOLED_DEPLOYS_PER_DEPLOYER`]).
    ///
    /// Counts *stored* deploys, not admissions: incremented where a deploy is written to the pool and
    /// decremented at both places one leaves it. An entry is dropped at zero rather than kept, so the
    /// map cannot grow past the number of deployers currently holding a slot — which is bounded by
    /// the pool itself, not by the caller.
    pooled_by_deployer: Arc<std::sync::Mutex<std::collections::HashMap<Vec<u8>, usize>>>,
    /// **The pool's term bytes in use** (see [`MAX_POOLED_DEPLOY_BYTES`]).
    ///
    /// Rebuilt from the store at boot and charged where a deploy is stored, returned at both places
    /// one leaves the pool — the same three points as the per-deployer share, because a byte bound
    /// that misses a return is a pool that fills and never drains.
    pooled_term_bytes: Arc<std::sync::atomic::AtomicUsize>,
    /// **Deploy ids being admitted right now** (see [`BlockDagStorage::reserve_deploy_id`]).
    ///
    /// In memory only: the durable record of a pooled deploy is its own row, and a restart rebuilds
    /// the pool's accounting from the store. Two callers that both hold a reservation cannot both
    /// reach the parse, which is the work this exists to refuse.
    deploy_reservations: Arc<std::sync::Mutex<std::collections::HashSet<DeployId>>>,
    block_metadata_store: Arc<BlockMetadataStore>,
    deploy_index: Arc<dyn KeyValueTypedStore<DeployId, BlockHash>>,
    deploy_store: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>>,
    /// Where the DAG's own gauges go (defaults to the no-op sink; the node wires its
    /// `MetricsRegistry` in). This is the attribution instrument for the structure's cost, in place
    /// of the profiler this environment cannot run (AUDIT C56's owed paragraph).
    metrics: Arc<dyn Metrics + Send + Sync>,
    /// **Equivocating blocks this node has refused, keyed by the sender** (AUDIT C200).
    ///
    /// The H-1 gate below refuses such a block *before any write*, so this is the only place it exists
    /// — and without it a proposer could never show the one offence that is unambiguous. In memory, so a
    /// restart forgets: the *evidence* is what has to be checkable, and it is checked by the receiver
    /// against its own DAG, not against a store this node happens to keep.
    equivocations: tokio::sync::Mutex<BTreeMap<Validator, Vec<u8>>>,
    /// **Deployer public key (hashed) → a block containing a deploy it signed** (the first one this node
    /// inserted). Attached by [`Self::with_deployer_index`]; `None` in tests and tools that never
    /// ask, and then `lookup_by_deployer` refuses. Serves the wallet's quantum key-hygiene check
    /// ("has this key ever signed?"), which without an index has to read every block of the chain.
    deployer_index: Option<Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>>>,
    /// The height at and above which every inserted block has been indexed (see
    /// [`DeployerLookup::indexed_from`]); persisted under [`DEPLOYER_INDEXED_FROM_KEY`].
    deployer_indexed_from: AtomicI64,
    /// **A devnet-only injection, now inert** (default `0` = off): it perturbed the `fringe-data`
    /// record, which retired with the store (Law 66/68; C250's residue, C270). Kept so the devnet
    /// CLI still parses, and reported as armed-but-inert on every insert. See
    /// [`Self::with_merge_divergence_injection`].
    merge_divergence_injection: u8,
    /// **The shard's durable records** — the store a reset writes itself into (F-U5-01, register C274),
    /// the one an autopropose halt writes itself into (W-N10-01, F-U10-02), and the one this shard's
    /// monotone health counters write into (W-N10-02). One store, one reserved key each:
    /// [`RESET_MARKER_KEY`], [`HALT_RECORD_KEY`] and [`HEALTH_COUNTERS_KEY`], all literals so none can
    /// collide with a block entry or with the others.
    ///
    /// Attached by [`Self::with_reset_marker`], whose name records which record needed it first; `None`
    /// in fixtures that use none of them, and then every write refuses — which is what stops a reset
    /// from running with no way for a crash to be repaired, and makes a halt's durability a fact the
    /// surface states rather than assumes. See [`ResetMarker`] for why that record is two phases,
    /// [`HaltRecord`] for why this one is not, and [`HealthCounters`] for why its three counters are
    /// persisted on a transition rather than on every observation.
    reset_marker: Option<Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>>>,
}

/// The deployer index's one non-key entry: the `indexed_from` height, big-endian `i64`. Deployer
/// entries are keyed by a 32-byte hash, so this 12-byte key cannot collide with one (and LMDB
/// refuses an empty key, which is why it is not the empty string).
pub const DEPLOYER_INDEXED_FROM_KEY: &[u8] = b"indexed-from";

/// The deployer index's **second** non-key entry: the store's own record count at the moment the
/// backfill declared the index complete, big-endian `u64` (W-N11-02).
///
/// **Because `0` alone is a claim about a store the writer need not have looked at.** The cursor says
/// "the index is complete", and a host that writes `0` into it suppresses the whole backfill: the node
/// then reports the index complete while every block below the pre-upgrade tip was never indexed, and
/// its deployer lookups answer empty. Recording *what* the completion covered makes the claim
/// checkable in one `count()` against the store it describes — a completion whose count does not match
/// is not believed, and the walk runs. 12 bytes for the same reason the cursor uses 12: deployer
/// entries are keyed by a 32-byte hash and cannot collide with it.
pub const DEPLOYER_INDEX_COUNT_KEY: &[u8] = b"indexed-count";

/// **Whether the recorded completion describes the index it claims to describe** (W-N11-02).
///
/// Three answers, and two of them mean "walk": a completion with no recorded count (a store from a
/// build before this one) cannot be vouched for, and one whose count disagrees with the store is
/// describing something else. Only a completion that matches is trusted. The check is one `get` and
/// one `count` — the codec-backed store answers `count` from `num_records`, so this is not a scan.
async fn completion_matches_the_index(
    index: &dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>,
) -> Result<bool, String> {
    let Some(bytes) = index
        .get(&[DEPLOYER_INDEX_COUNT_KEY.to_vec()])
        .await?
        .into_iter()
        .next()
        .flatten()
    else {
        return Ok(false);
    };
    let recorded = decode_count(&bytes)?;
    Ok(index.count().await? as u64 == recorded)
}

/// The record count a completion was written with. Refused rather than guessed, like
/// [`decode_height`].
fn decode_count(bytes: &[u8]) -> Result<u64, String> {
    let arr: [u8; 8] = bytes.try_into().map_err(|_| {
        format!(
            "deployer index: malformed indexed-count entry ({} bytes)",
            bytes.len()
        )
    })?;
    Ok(u64::from_be_bytes(arr))
}

impl BlockDagKeyValueStorage {
    /// Rebuild the in-memory DAG representation from the stores (port of `BlockDagKeyValueStorage.create`).
    pub async fn create(
        block_metadata_store: Arc<BlockMetadataStore>,
        deploy_index: Arc<dyn KeyValueTypedStore<DeployId, BlockHash>>,
        deploy_store: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>>,
    ) -> Result<Self, String> {
        let representation = Self::rebuild_representation(&block_metadata_store).await?;
        // **A restarted node's pool is not empty**, and both the share and the byte budget have to
        // start from what is actually stored rather than from zero — a node that comes back with a
        // full pool would otherwise admit a fresh ten thousand deploys per deployer, and 64 MiB of
        // terms, on top of what it holds.
        let (pooled_by_deployer, pooled_term_bytes) = pooled_accounting(&deploy_store).await?;
        Ok(BlockDagKeyValueStorage {
            representation: tokio::sync::RwLock::new(Arc::new(representation)),
            lock: tokio::sync::Mutex::new(()),
            deploy_admission: tokio::sync::Mutex::new(()),
            deploy_rate: KeyedRateLimiter::new(
                MAX_DEPLOYS_PER_DEPLOYER_PER_SEC,
                MAX_TRACKED_DEPLOYERS,
            ),
            pooled_by_deployer: Arc::new(std::sync::Mutex::new(pooled_by_deployer)),
            pooled_term_bytes: Arc::new(std::sync::atomic::AtomicUsize::new(pooled_term_bytes)),
            deploy_reservations: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            block_metadata_store,
            deploy_index,
            deploy_store,
            metrics: Arc::new(MetricsNop),
            equivocations: tokio::sync::Mutex::new(BTreeMap::new()),
            deployer_index: None,
            deployer_indexed_from: AtomicI64::new(0),
            merge_divergence_injection: 0,
            reset_marker: None,
        })
    }

    /// Fold the stores into an in-memory DAG representation.
    ///
    /// A function of its own because [`Self::create`] is not the only caller any more: a reset
    /// ([`Self::drop_above`]) rewinds the metadata and then has to produce the *same* representation
    /// this produced at boot. Two implementations of that fold would be two chances for the rebuilt
    /// view to differ from the booted one, and the view is what every dependency check reads.
    ///
    /// **The `fringe-data` store is not folded any more** (Law 66/68; C250's residue, C270). The fold
    /// reads block *metadata* only — the message map, the index maps, and the per-block claims
    /// (`fringe_state_hash`, `fringe`, `member_of_fringe`) the readers now derive from. A store that
    /// predates this change still holds its `fringe-data` rows on disk; they are simply never read.
    async fn rebuild_representation(
        block_metadata_store: &Arc<BlockMetadataStore>,
    ) -> Result<DagRepresentation, String> {
        let dag_set = block_metadata_store.dag_set().await;
        let child_map = block_metadata_store.child_map_data().await;
        let height_map = block_metadata_store.height_map().await;

        let mut dag_msg_state = DagMessageState::<BlockHash, Validator>::empty();
        // H-1's gate, re-applied to a *restored* store. `insert` refuses a second message by the same
        // sender reusing a `seq_num`, but this fold rebuilds the message map from persisted metadata
        // without that check, and the persisted layer never looks at `(sender, seq_num)` at all
        // (`validate_dag_state` reads the height map's contiguity). So a store that already holds a
        // fork — written by a build that predates the gate, or by any path that writes metadata
        // directly — used to restore straight into the equivocation the gate exists to keep out, and
        // the H-1 stall it prevents was live again with nothing saying so. Tracked as a map from
        // `(sender, seq_num)` to the block that claimed it rather than `insert`'s scan over
        // `msg_map`, so the restore stays O(N) (AUDIT C55).
        let mut claimed_by: BTreeMap<(Validator, SeqNum), BlockHash> = BTreeMap::new();

        for hash in height_map.values().flatten() {
            if dag_msg_state.msg_map.contains_key(hash) {
                continue;
            }
            let block = block_metadata_store.get_unchecked(hash).await?;
            let msg = message_from_block_metadata(&block, &dag_msg_state.msg_map)
                .ok_or_else(|| "justification not present in message map".to_string())?;
            if let Some(previous) = claimed_by.insert((msg.sender, msg.sender_seq), msg.id) {
                return Err(format!(
                    "{EQUIVOCATION_PREFIX} in the stored DAG: sender {} reuses sequence number {} \
                     for blocks {} and {}",
                    rchain_shared::base16::encode(msg.sender.as_bytes()),
                    i64::from(msg.sender_seq),
                    previous.to_hex(),
                    msg.id.to_hex()
                ));
            }
            // In place: this loop folds the whole stored chain through the state, and the
            // persistent-shaped `insert_msg` clones the map (and every message's `seen` set) on
            // each step — Θ(N³) over a stored chain, which is what made a 5,844-block restart take
            // longer than the devnet waits for one (AUDIT C55).
            dag_msg_state.insert_msg_mut(&msg);
        }

        Ok(DagRepresentation {
            dag_set,
            child_map,
            height_map,
            dag_message_state: dag_msg_state,
        })
    }

    /// **Rewind the DAG to `height`** — the node-side reset (C249's third rung, #287).
    ///
    /// A node that cannot locally heal has exactly one way back today: an operator stops it and deletes
    /// the shard data dir by hand, which exists as a comment and nowhere as code. This is that path as
    /// code: everything above `height` goes, the representation is rebuilt from what survives, and the
    /// node replays from the anchor.
    ///
    /// **The order is metadata, representation, indices**, and each step is there for a reason. The
    /// metadata store's own rewind is store-first (AUDIT C172's shape): a failed delete leaves the
    /// in-memory index alone, and the index is rebuilt from what the store actually holds rather than
    /// edited down. The representation is then rebuilt by the *same* fold `create` uses, so a rewound
    /// node sees what a rebooted one would. The derived indices are cleaned last because they are
    /// derived: an entry pointing above the anchor answers "this deployer signed in block X" for a block
    /// the node no longer has, and it is found by value, since both indices are keyed by deploy/deployer
    /// rather than by block.
    ///
    /// **Held under the insert lock**, so a concurrent `insert` cannot land a block above the anchor
    /// while the rewind is in flight. Without it the insert and the rebuild race, and the representation
    /// keeps a block the store has just dropped — a node serving a block it cannot replay.
    ///
    /// **The one new invariant the per-block shape introduces** (Law 66/68; C250's residue, C270):
    /// **an orphaned member-of-fringe mark is cleared.**
    ///
    /// A surviving block's `member_of_fringe` names the fringe it belongs to, and a fringe *is*
    /// `X.fringe` for the block(s) X that finalised it — so the mark resolves against a DAG only while
    /// some surviving block still declares that fringe. A rewind above the anchor drops the finalisers;
    /// a surviving block whose mark names a fringe no surviving block declares is left holding a claim
    /// that names nothing. The old design could not have this: the store's record was read *by* the
    /// key, and a key with no finalisers simply was not consulted. With the mark travelling on the
    /// block, the orphan would still be found by a reader that resolves it — a rewound claim re-entering
    /// the view. So it is cleared, here, before the rebuild.
    ///
    /// The dropped blocks' own marks need no clearing: they are rows above the anchor and the store's
    /// rewind takes them whole. What this pass fixes is the *survivor* — the anchor, whose finalisers
    /// were the blocks just dropped.
    ///
    /// **The derived indexes are repaired against what survives, not against what this call dropped**
    /// (B39, HAZOP F-U17-11). `dropped` is only what *this* call removed: a crash that dropped metadata
    /// and died before the cleanup leaves rows behind, and a later rewind that drops nothing — or a boot
    /// with no rewind at all — must still remove them. A stale deploy-index row is not inert: the
    /// proposer's replay guard (`casper/src/blocks/proposer/proposer.rs:697`) treats "in the DAG" as
    /// "already included" and silently skips the pooled deploy. The repair is idempotent, so it runs on
    /// every rewind rather than behind a "did the drop return anything" branch.
    ///
    /// **What this does not drop, stated rather than implied.** The block *messages* live in
    /// `BlockStore`, which this type does not own, so the bodies stay on disk. `deployer_indexed_from`
    /// also stays as it is: its claim is "complete from this height **up to the tip**", and with the tip
    /// now at the anchor that claim is still true of the shorter chain. The deploy-pool accounting
    /// (`pooled_by_deployer`, `pooled_term_bytes`) is not touched: the pool holds only *un-included*
    /// deploys, and a deploy that was included had its share returned when it left the pool.
    pub async fn drop_above(&self, height: BlockHeight) -> Result<Vec<BlockHash>, String> {
        let _guard = self.lock.lock().await;

        let dropped = self.block_metadata_store.drop_above(height).await?;

        // Clear the survivours' orphaned marks (see the doc above). One pass over what the store now
        // holds: the fringes a surviving block *declares* are the names that are still resolvable, and
        // any mark naming a hash outside that set has lost its finalisers.
        let surviving: Vec<BlockHash> = self
            .block_metadata_store
            .dag_set()
            .await
            .iter()
            .copied()
            .collect();
        let mut metas: BTreeMap<BlockHash, BlockMetadata> = BTreeMap::new();
        for h in &surviving {
            metas.insert(*h, self.block_metadata_store.get_unchecked(h).await?);
        }
        let declared: BTreeSet<Blake2b256Hash> = metas
            .values()
            .map(|m| rchain_models::block_metadata::fringe_hash_of(&m.fringe))
            .collect();
        for (h, m) in &metas {
            if let Some(f) = m.member_of_fringe {
                if !declared.contains(&f) {
                    eprintln!(
                        "[rewind] block {}: cleared its member-of-fringe mark {f:?} — no surviving \
                         block declares that fringe, so the mark names a claim the rewind removed",
                        h.to_hex()
                    );
                    self.block_metadata_store
                        .add(BlockMetadata {
                            member_of_fringe: None,
                            ..m.clone()
                        })
                        .await?;
                }
            }
        }

        let rebuilt = Self::rebuild_representation(&self.block_metadata_store).await?;
        *self.representation.write().await = Arc::new(rebuilt);

        let surviving_set: BTreeSet<Vec<u8>> =
            surviving.iter().map(|h| h.as_bytes().to_vec()).collect();
        let (deploy_rows, deployer_rows) = self.repair_derived_index_rows(&surviving_set).await?;
        if deploy_rows + deployer_rows > 0 {
            eprintln!(
                "[rewind] repaired {} deploy-index and {} deployer-index row(s) that named a block \
                 this DAG no longer holds",
                deploy_rows, deployer_rows
            );
        }
        Ok(dropped)
    }

    /// The hashes this DAG currently holds, as bytes — the survivor set both the rewind's orphan-mark
    /// pass and its derived-index repair are judged against.
    async fn surviving_block_hashes(&self) -> BTreeSet<Vec<u8>> {
        self.block_metadata_store
            .dag_set()
            .await
            .iter()
            .map(|h| h.as_bytes().to_vec())
            .collect()
    }

    /// Remove every derived-index row whose block is not in `surviving`, returning
    /// `(deploy rows removed, deployer rows removed)`.
    ///
    /// Idempotent: a second call with the same survivor set removes nothing. It derives the stale rows
    /// from the **survivors**, not from a dropped list, which is what lets it repair a state an earlier
    /// crash left behind (`drop_above`'s early return was the shape that skipped this).
    async fn repair_derived_index_rows(
        &self,
        surviving: &BTreeSet<Vec<u8>>,
    ) -> Result<(usize, usize), String> {
        let deploys = self.deploy_index.to_map().await?;
        let stale: Vec<DeployId> = deploys
            .into_iter()
            .filter(|(_, hash)| !surviving.contains(hash.as_bytes().as_slice()))
            .map(|(id, _)| id)
            .collect();
        let deploy_rows = stale.len();
        if !stale.is_empty() {
            self.deploy_index.delete(&stale).await?;
        }
        let mut deployer_rows = 0usize;
        if let Some(index) = &self.deployer_index {
            let entries = index.to_map().await?;
            // The store carries the backfill cursor and its count beside the rows. They are not
            // derived-index rows: the cursor's claim ("complete from this height up to the tip") is
            // still true of a shorter chain after a rewind, and removing it would make the node
            // re-backfill from scratch. The 8-byte values could never equal a 32-byte block hash, so
            // filtering by value alone would already spare them — naming them says why.
            let stale: Vec<Vec<u8>> = entries
                .into_iter()
                .filter(|(key, value)| {
                    key.as_slice() != DEPLOYER_INDEXED_FROM_KEY
                        && key.as_slice() != DEPLOYER_INDEX_COUNT_KEY
                        && !surviving.contains(value)
                })
                .map(|(key, _)| key)
                .collect();
            deployer_rows = stale.len();
            if !stale.is_empty() {
                index.delete(&stale).await?;
            }
        }
        Ok((deploy_rows, deployer_rows))
    }

    /// Attach the deployer index. From here on every inserted block is indexed. A store that has
    /// never been used records the current chain height as `indexed_from`: on a fresh node that is
    /// 0 and the index is complete from genesis; on a node upgraded onto an existing chain the blocks
    /// below it are filled in by [`Self::backfill_deployer_index`].
    pub async fn with_deployer_index(
        mut self,
        index: Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>>,
    ) -> Result<Self, String> {
        let stored = index
            .get(&[DEPLOYER_INDEXED_FROM_KEY.to_vec()])
            .await?
            .into_iter()
            .next()
            .flatten();
        let from = match stored {
            Some(bytes) => decode_height(&bytes)?,
            None => {
                let from = self.representation.read().await.latest_block_number();
                index
                    .put(&[(
                        DEPLOYER_INDEXED_FROM_KEY.to_vec(),
                        from.to_be_bytes().to_vec(),
                    )])
                    .await?;
                from
            }
        };
        self.deployer_indexed_from = AtomicI64::new(from);
        self.deployer_index = Some(index);
        Ok(self)
    }

    /// Attach the store a reset records itself in (`reset-marker`). From here on
    /// `write_reset_marker` works and a restart can repair a reset that crashed half-way; without it the
    /// write refuses, so a reset cannot run with no way for its crash window to be closed.
    ///
    /// Nothing is read at attach time — unlike the deployer index, this record has no "how much is
    /// indexed" claim to seed — so this is a plain setter returning `Self`.
    pub fn with_reset_marker(
        mut self,
        store: Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>>,
    ) -> Self {
        self.reset_marker = Some(store);
        self
    }

    /// Index every block already in the DAG, then record the index as complete (`0`). Run once in
    /// the background after an upgrade; a no-op when the index is already complete or not attached.
    /// A crash part-way leaves the marker where it was, so the next start runs it again. Takes the insert lock one block at a time, so the node keeps inserting meanwhile.
    /// Returns the number of blocks read.
    pub async fn backfill_deployer_index(
        &self,
        block_store: &Arc<dyn KeyValueTypedStore<BlockHash, BlockMessage>>,
    ) -> Result<usize, String> {
        let Some(index) = &self.deployer_index else {
            return Ok(0);
        };
        let upto = self.deployer_indexed_from.load(Ordering::SeqCst);
        // **`0` is a claim, so it is checked against the store before it is believed** (W-N11-02).
        // The row: a host that sets the cursor to `0` — the "index complete" value — makes the backfill
        // return here, so the node reports an index that was never built and answers deployer lookups
        // with nothing. A completion that does not describe this store is walked instead.
        if upto <= 0 && completion_matches_the_index(index.as_ref()).await? {
            return Ok(0);
        }
        // Every block in the DAG, not the height map: the height map leaves out validation-failed
        // blocks, and `insert` indexes those too, so walking it would leave an upgraded node short of
        // a fresh one (review of #277). Blocks already indexed cost a read and write nothing.
        let hashes: Vec<BlockHash> = {
            let repr = self.get_representation().await;
            repr.dag_set.iter().copied().collect()
        };
        for hash in &hashes {
            let block = block_store
                .get(&[*hash])
                .await?
                .into_iter()
                .next()
                .flatten()
                .ok_or_else(|| {
                    format!(
                        "deployer index backfill: block {} is not in the block store",
                        hash.to_hex()
                    )
                })?;
            let _guard = self.lock.lock().await;
            index_deployers(index.as_ref(), &block).await?;
        }
        let _guard = self.lock.lock().await;
        // **The completion is written with what it covers** (W-N11-02): the store's own record count,
        // measured with both marker entries present so the number is stable across boots (the marker
        // rows are records too, and a count taken before the second one existed would disagree with
        // every later count of the same store).
        index
            .put(&[(
                DEPLOYER_INDEX_COUNT_KEY.to_vec(),
                0u64.to_be_bytes().to_vec(),
            )])
            .await?;
        let total = index.count().await? as u64;
        index
            .put(&[
                (
                    DEPLOYER_INDEXED_FROM_KEY.to_vec(),
                    0i64.to_be_bytes().to_vec(),
                ),
                (
                    DEPLOYER_INDEX_COUNT_KEY.to_vec(),
                    total.to_be_bytes().to_vec(),
                ),
            ])
            .await?;
        self.deployer_indexed_from.store(0, Ordering::SeqCst);
        Ok(hashes.len())
    }

    /// Point the DAG's gauges at a sink (the node's `MetricsRegistry`) and publish what the
    /// structure already holds. Defaults to `MetricsNop`, so a test or a tool that never attaches a
    /// sink pays nothing.
    ///
    /// The publish is on attach as well as per insert: a restart has a whole chain to report before
    /// it accepts its first block, and `/metrics` should say so.
    /// **A retired devnet instrument, kept so the flag still parses.** It perturbed this node's own
    /// `fringe-data` record — one synthetic rejected-deploy id per record, so four nodes with four
    /// values held four different records at any shared fringe key and the merge (which read that
    /// record) answered differently. The record retired with the store (Law 66/68; C250's residue,
    /// C270): the claim a merge reads is now the block's own metadata, and a block's metadata is not a
    /// node-local derived value, so there is nothing left to perturb without also changing a block's
    /// content — which would not be the same instrument. The flag is therefore accepted and *reported
    /// as armed but inert* on every insert (see `insert`): a node asked to do this cannot be mistaken
    /// for one that has gone wrong on its own, and an operator cannot be misled into believing an
    /// injection happened. The divergence rig it staged is `up --validators 4 --fresh`, which needs no
    /// injection (`spec/audit/evidence/n-anchor-drill/self-reject-stall.txt`).
    ///
    /// The node still refuses to arm it without `--dev-mode` (see `node/src/configuration`), so the
    /// flag's own guard is unchanged even though its effect is gone.
    pub fn with_merge_divergence_injection(mut self, injection: u8) -> Self {
        self.merge_divergence_injection = injection;
        self
    }

    /// Replace the per-deployer admission bound ([`MAX_DEPLOYS_PER_DEPLOYER_PER_SEC`]).
    ///
    /// A pool that is legitimately filled from one key — a fixture, or a node whose operator
    /// pre-loads a batch — needs the pool's count bound without the submitter's rate bound. The
    /// node leaves this at its default; it is here so the two bounds can be told apart in a test
    /// rather than conflated.
    pub fn with_deploy_rate(mut self, max_per_sec: u64, max_keys: usize) -> Self {
        self.deploy_rate = KeyedRateLimiter::new(max_per_sec, max_keys);
        self
    }

    pub fn with_metrics(mut self, metrics: Arc<dyn Metrics + Send + Sync>) -> Self {
        self.metrics = metrics;
        // Synchronous, and no guard is held here — `try_read` rather than an await, because this is
        // the constructor path. A caller that attached the sink while a writer held the lock would
        // simply publish nothing until its next insert.
        if let Ok(representation) = self.representation.try_read() {
            self.set_gauges(
                representation.message_count(),
                representation.seen_entries(),
                representation.index_entries(),
                representation.logical_bytes(),
            );
            self.set_merge_shape_gauges();
            self.set_soft_checkpoint_gauges();
            self.set_read_path_gauges();
        }
        self
    }

    /// Push the four gauges (no lock: the caller has the values).
    ///
    /// **Four, not five.** The `fringe_states` gauge counted entries in the retired map (Law 66/68;
    /// C250's residue, C270): with the record gone there is no such set to size, and the per-block
    /// claims are already counted by `index_entries` (the block metadata store's own index).
    fn set_gauges(
        &self,
        messages: usize,
        seen_entries: usize,
        index_entries: usize,
        logical_bytes: usize,
    ) {
        let source = Source::base().sub("dag");
        let count = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
        self.metrics.set_gauge(&source, "messages", count(messages));
        self.metrics
            .set_gauge(&source, "seen_entries", count(seen_entries));
        self.metrics
            .set_gauge(&source, "index_entries", count(index_entries));
        self.metrics
            .set_gauge(&source, "logical_bytes", count(logical_bytes));
    }

    /// Push the merge search's shape: the envelope as gauges, and the **width distribution** as a
    /// histogram. Published from the same two places as the DAG gauges — on attach and per insert —
    /// because a merge is what moves the DAG and this is the node's observability tick.
    ///
    /// **Why a distribution and not only the maxima** (register row C182, #127's change record,
    /// Stage 1): `search_census` carries the *worst* scope seen, and a bound on this class is chosen
    /// from how *often* a scope is wide — a threshold and a price are set from a distribution. The
    /// counts are taken as deltas (`search_census::take_width_deltas`) because the registry's `record`
    /// accumulates, so a running total would double-count.
    fn set_merge_shape_gauges(&self) {
        use crate::merging::search_census;
        use std::sync::atomic::Ordering;

        let source = Source::base().sub("merge");
        let count = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
        let load = |a: &std::sync::atomic::AtomicUsize| count(a.load(Ordering::Relaxed));

        self.metrics.set_gauge(
            &source,
            "searches",
            i64::try_from(search_census::MERGES.load(Ordering::Relaxed)).unwrap_or(i64::MAX),
        );
        self.metrics
            .set_gauge(&source, "max_scope_width", load(&search_census::MAX_KEYS));
        self.metrics.set_gauge(
            &source,
            "max_conflict_pairs",
            load(&search_census::MAX_CONFLICTS),
        );
        self.metrics.set_gauge(
            &source,
            "max_asymmetric_pairs",
            load(&search_census::MAX_ASYMMETRIC),
        );
        self.metrics.set_gauge(
            &source,
            "max_states_expanded",
            load(&search_census::MAX_EXPANDED),
        );
        // The mean's numerator: `scope_width_total / searches` is the **exact** mean scope width, where
        // the histogram below can only offer a mean of bucket edges (C182's second defect).
        self.metrics.set_gauge(
            &source,
            "scope_width_total",
            i64::try_from(search_census::TOTAL_WIDTH.load(Ordering::Relaxed)).unwrap_or(i64::MAX),
        );
        // The distribution: one histogram sample per bucket, valued at its edge. The counts are exact
        // and that is what the histogram is read for; `_sum`/`_min`/`_max` over edges are meaningless
        // by construction, and the open-ended bucket is valued at the last edge (`usize::MAX` would
        // overflow the wire).
        self.metrics.set_gauge(
            &source,
            "states_expanded_total",
            i64::try_from(search_census::TOTAL_EXPANDED.load(Ordering::Relaxed))
                .unwrap_or(i64::MAX),
        );
        // The two distributions, one sample per bucket, valued at its edge: the counts are exact and
        // that is what a threshold is read off; `_sum`/`_min`/`_max` over edges are meaningless by
        // construction, which is why the exact means are published as `*_total` gauges above, and the
        // open-ended bucket is valued at the last edge (`usize::MAX` would overflow the wire).
        let open = |edges: &[usize]| edges[edges.len() - 1];
        for (edge, delta) in search_census::take_width_deltas() {
            if delta > 0 {
                let edge = if edge == usize::MAX {
                    open(&search_census::WIDTH_EDGES)
                } else {
                    edge
                };
                self.metrics
                    .record(&source, "scope_width", count(edge), count(delta));
            }
        }
        for (edge, delta) in search_census::take_expanded_deltas() {
            if delta > 0 {
                let edge = if edge == usize::MAX {
                    open(&search_census::EXPANDED_EDGES)
                } else {
                    edge
                };
                self.metrics
                    .record(&source, "states_expanded", count(edge), count(delta));
            }
        }
    }

    /// Publish #144's Stage-1 play-path soft-checkpoint census. These are monotone process counters,
    /// not verdict inputs: the node computes exactly the same state and block with or without a
    /// metrics sink. A campaign reads deltas between two `/metrics` scrapes so earlier activity in
    /// the process cannot be mistaken for work done by the measured arm.
    fn set_soft_checkpoint_gauges(&self) {
        use crate::runtime_manager::soft_checkpoint_census::{self, Site};

        let source = Source::base().sub("runtime");
        let count = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        let census = soft_checkpoint_census::snapshot();

        self.metrics
            .set_gauge(&source, "deploy_units", count(census.deploy_units));
        self.metrics.set_gauge(
            &source,
            "deploy_unit_total_ns",
            count(census.deploy_unit_total_ns),
        );
        self.metrics.set_gauge(
            &source,
            "deploy_unit_max_ns",
            count(census.deploy_unit_max_ns),
        );
        for site in Site::ALL {
            let sample = census.sites[site.index()];
            self.metrics.set_gauge(
                &source,
                &format!("soft_checkpoint_{}_count", site.metric_stem()),
                count(sample.count),
            );
            self.metrics.set_gauge(
                &source,
                &format!("soft_checkpoint_{}_total_ns", site.metric_stem()),
                count(sample.total_ns),
            );
            self.metrics.set_gauge(
                &source,
                &format!("soft_checkpoint_{}_max_ns", site.metric_stem()),
                count(sample.max_ns),
            );
        }
    }

    /// Publish the read-path census (issue #364): where a read's time goes.
    ///
    /// Monotone process counters like the census it publishes — the node computes the same state and
    /// returns the same reply with or without a metrics sink. A campaign reads **deltas between two
    /// scrapes**, so earlier activity in the process cannot be mistaken for the measured arm's work.
    ///
    /// Three modules publish from this one tick because they are three views of one request: the
    /// casper census times the stages, `rchain_rholang`'s separates the install leg out of `reset`,
    /// and `rchain_rspace`'s counts the substrate reads that no stage boundary can see.
    fn set_read_path_gauges(&self) {
        use crate::runtime_manager::read_path_census::{self, Stage};

        let source = Source::base().sub("runtime");
        let count = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        let census = read_path_census::snapshot();

        self.metrics
            .set_gauge(&source, "read_requests", count(census.reads));
        self.metrics
            .set_gauge(&source, "read_total_ns", count(census.total_ns));
        self.metrics
            .set_gauge(&source, "read_max_ns", count(census.max_ns));
        self.metrics.set_gauge(
            &source,
            "read_reply_first_private_name",
            count(census.reply_first_private_name),
        );
        self.metrics
            .set_gauge(&source, "read_reply_out", count(census.reply_out));
        self.metrics
            .set_gauge(&source, "read_reply_none", count(census.reply_none));
        for stage in Stage::ALL {
            let sample = census.stages[stage.index()];
            self.metrics.set_gauge(
                &source,
                &format!("read_{}_count", stage.metric_stem()),
                count(sample.count),
            );
            self.metrics.set_gauge(
                &source,
                &format!("read_{}_total_ns", stage.metric_stem()),
                count(sample.total_ns),
            );
            self.metrics.set_gauge(
                &source,
                &format!("read_{}_max_ns", stage.metric_stem()),
                count(sample.max_ns),
            );
        }

        let installs = rchain_rholang::read_path_census::snapshot();
        self.metrics.set_gauge(
            &source,
            "read_install_passes",
            count(installs.installs.calls),
        );
        self.metrics.set_gauge(
            &source,
            "read_install_total_ns",
            count(installs.installs.total_ns),
        );
        self.metrics.set_gauge(
            &source,
            "read_definitions_installed",
            count(installs.installs.installed_processes),
        );
        self.metrics.set_gauge(
            &source,
            "read_reset_install_calls",
            count(installs.reset_installs.calls),
        );
        self.metrics.set_gauge(
            &source,
            "read_reset_install_total_ns",
            count(installs.reset_installs.total_ns),
        );

        let substrate = rchain_rspace::read_path_census::snapshot();
        self.metrics.set_gauge(
            &source,
            "read_history_reads",
            count(substrate.history_reads),
        );
        self.metrics.set_gauge(
            &source,
            "read_history_read_misses",
            count(substrate.history_read_misses),
        );
        self.metrics.set_gauge(
            &source,
            "read_trie_cache_hits",
            count(substrate.trie_cache_hits),
        );
        self.metrics.set_gauge(
            &source,
            "read_trie_cache_misses",
            count(substrate.trie_cache_misses),
        );
        self.metrics.set_gauge(
            &source,
            "read_trie_store_loads",
            count(substrate.trie_store_loads),
        );
        self.metrics.set_gauge(
            &source,
            "read_space_reset_calls",
            count(substrate.space_reset_calls),
        );
        self.metrics.set_gauge(
            &source,
            "read_space_reset_total_ns",
            count(substrate.space_reset_total_ns),
        );
    }

    /// Expire deploys the current tip already rejects (`valid_after <= latest - lifespan`).
    /// The proposer applies the same predicate at the *next* candidate height, so a deploy may remain
    /// in the pool for one final height while still being excluded from a block that would reject it.
    /// Keeping the pool GC conservative avoids deleting a deploy before the validator considers it
    /// expired; proposer selection is the actual next-block gate.
    async fn expire_deploys(&self, latest_block_number: i64) -> Result<(), String> {
        let pooled = self.deploy_store.to_map().await?;
        let expired: Vec<(DeployId, Vec<u8>)> = pooled
            .iter()
            .filter(|(_, d)| {
                crate::validate::deploy_expired_at_block(
                    d.data.valid_after_block_number,
                    latest_block_number,
                    crate::multi_parent_casper::DEPLOY_LIFESPAN,
                )
            })
            .map(|(id, d)| (id.clone(), d.deployer.clone()))
            .collect();
        if !expired.is_empty() {
            let ids: Vec<DeployId> = expired.iter().map(|(id, _)| id.clone()).collect();
            let expired_bytes: usize = expired
                .iter()
                .filter_map(|(id, _)| pooled.get(id))
                .map(|d| d.data.term.len())
                .sum();
            self.deploy_store.delete(&ids).await?;
            self.return_pooled_bytes(expired_bytes);
            // Return each deployer's share of the pool. An expired deploy has left it, so holding
            // the share would lock its deployer out for a deploy the node no longer has.
            for (_, deployer) in &expired {
                release_deployer_slot(&self.pooled_by_deployer, deployer);
            }
        }
        Ok(())
    }

    /// Return `bytes` to the pool's term budget, saturating at zero.
    ///
    /// Two removal paths can see the same deploy, exactly as they can for the per-deployer share; a
    /// second return must leave the budget at zero rather than wrap it to a huge number, which would
    /// refuse every subsequent deploy on a node that had done nothing wrong.
    fn return_pooled_bytes(&self, bytes: usize) {
        let _ = self.pooled_term_bytes.fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |in_use| Some(in_use.saturating_sub(bytes)),
        );
    }

    /// The pool's term bytes in use — the reading a status or metrics surface publishes, and the
    /// value a test asserts did not move when a duplicate was refused.
    pub fn pooled_term_bytes(&self) -> usize {
        self.pooled_term_bytes
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// The shared prefix of the two equivocation rejections — the H-1 gate on `insert`, and the
/// stored-DAG restore check. The proposer matches it to tell a benign stale-snapshot
/// self-equivocation (§48) from a genuine insert failure: the block is internally consistent, the DAG
/// just advanced under it. Defined beside the rejections so the string and the check cannot drift.
pub const EQUIVOCATION_PREFIX: &str = "equivocation detected";

/// Record `block` against each deployer it carries that the index does not have yet. The caller
/// holds the insert lock, so the read-then-write cannot race another insert.
async fn index_deployers(
    index: &dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>,
    block: &BlockMessage,
) -> Result<(), String> {
    let deployers: BTreeSet<Vec<u8>> = block
        .state
        .deploys
        .iter()
        .map(|d| deployer_index_key(&d.deploy.deployer))
        .collect();
    if deployers.is_empty() {
        return Ok(());
    }
    let keys: Vec<Vec<u8>> = deployers.into_iter().collect();
    let existing = index.get(&keys).await?;
    let fresh: Vec<(Vec<u8>, Vec<u8>)> = keys
        .into_iter()
        .zip(existing)
        .filter(|(_, e)| e.is_none())
        .map(|(k, _)| (k, block.block_hash.as_bytes().to_vec()))
        .collect();
    if !fresh.is_empty() {
        index.put(&fresh).await?;
    }
    Ok(())
}

/// The index is keyed by `blake2b256(public key)` rather than the 65-byte key: a third smaller, and
/// it lets a lookup ask by the hash, so checking a key that has never signed does not hand that key
/// to the node (`lookup_by_deployer` takes this hash).
pub fn deployer_index_key(deployer: &[u8]) -> Vec<u8> {
    Blake2b256Hash::create(deployer).as_bytes().to_vec()
}

fn decode_height(bytes: &[u8]) -> Result<i64, String> {
    let arr: [u8; 8] = bytes.try_into().map_err(|_| {
        format!(
            "deployer index: malformed indexed-from entry ({} bytes)",
            bytes.len()
        )
    })?;
    Ok(i64::from_be_bytes(arr))
}

#[async_trait]
impl BlockDagStorage for BlockDagKeyValueStorage {
    async fn get_representation(&self) -> Arc<DagRepresentation> {
        // A pointer copy under a lock held for nanoseconds — this used to be a full Θ(N²) deep copy
        // with the read lock held throughout (AUDIT C56's owed paragraph).
        self.representation.read().await.clone()
    }

    /// **The forward, and it is not decorative.** The body lives in this type's *inherent* `drop_above`
    /// (it predates the trait method); without this, a call through `&dyn BlockDagStorage` — which is how
    /// the engine holds its DAG — resolves the **trait's refusing default** while a call on the concrete
    /// type takes the real one. That difference is invisible in every existing test and in every
    /// direct caller, and the reset's durable-half test found it on its first run: the engine would have
    /// refused to rewind on exactly the node that needed it.
    async fn drop_above(&self, height: BlockHeight) -> Result<Vec<BlockHash>, String> {
        BlockDagKeyValueStorage::drop_above(self, height).await
    }

    /// **The forward, for the same reason as `drop_above`'s.** The boot guard and the reset bracket hold
    /// the DAG as `&dyn BlockDagStorage`; the trait's default answers `(0, 0)` (honest for a backend
    /// with no indexes), and without this forward a `dyn` caller would repair nothing on the one storage
    /// that has rows to repair.
    async fn repair_derived_indexes(&self) -> Result<(usize, usize), String> {
        let surviving = self.surviving_block_hashes().await;
        self.repair_derived_index_rows(&surviving).await
    }

    /// **The reset's record, from the storage that can keep one** (F-U5-01, register C274). Three
    /// forwards rather than three defaults: the default write *refuses*, so a `dyn BlockDagStorage`
    /// caller that kept the default would make every reset fail on exactly the node that needed one —
    /// the same shape as `drop_above`'s forward two methods up, and the reason these are written the
    /// same way. The store is absent only in a fixture that never attached one, and then the refusing
    /// default is the honest answer.
    async fn write_reset_marker(&self, marker: ResetMarker) -> Result<(), String> {
        let Some(store) = &self.reset_marker else {
            return Err(
                "this DAG storage has no reset-marker store attached, so a reset it performs could \
                 not be repaired after a crash — refusing to run one"
                    .to_string(),
            );
        };
        let result = store
            .put(&[(RESET_MARKER_KEY.to_vec(), marker.encode())])
            .await;
        // The surface follows the store (F-U12-02, §111): every writer reports what it wrote, so a marker
        // standing on a node that cannot catch up is visible to a probe rather than only in a log line.
        if result.is_ok() {
            crate::interpreter_util::note_reset_marker(Some(marker));
        }
        result
    }

    async fn read_reset_marker(&self) -> Result<Option<ResetMarker>, String> {
        let Some(store) = &self.reset_marker else {
            return Ok(None);
        };
        let raw = store
            .get(&[RESET_MARKER_KEY.to_vec()])
            .await?
            .into_iter()
            .next()
            .flatten();
        let marker = raw.map(|bytes| ResetMarker::decode(&bytes)).transpose()?;
        crate::interpreter_util::note_reset_marker(marker.clone());
        Ok(marker)
    }

    async fn clear_reset_marker(&self) -> Result<(), String> {
        let Some(store) = &self.reset_marker else {
            return Ok(());
        };
        let result = store.delete(&[RESET_MARKER_KEY.to_vec()]).await.map(|_| ());
        if result.is_ok() {
            crate::interpreter_util::note_reset_marker(None);
        }
        result
    }

    /// **The same store, a second reserved key** — see [`HaltRecord`] for why the halt needs a durable
    /// record at all, and why this one is obeyed rather than merely reported.
    ///
    /// A store that is absent refuses, like the reset marker's write and for the same reason: a backend
    /// that cannot record the halt must not look like one that did. Unlike that write, the caller here
    /// has already decided to stop — so the refusal becomes `durable: false` on the reading the surface
    /// serves rather than an aborted halt.
    async fn write_halt_record(&self, record: HaltRecord) -> Result<(), String> {
        let Some(store) = &self.reset_marker else {
            return Err(
                "this DAG storage has no durable-records store attached, so this shard's autopropose \
                 halt would be honoured by this process and forgotten by the next"
                    .to_string(),
            );
        };
        store
            .put(&[(HALT_RECORD_KEY.to_vec(), record.encode())])
            .await
    }

    async fn read_halt_record(&self) -> Result<Option<HaltRecord>, String> {
        // **Refused, not `Ok(None)`, and the difference is the whole row.** `Ok(None)` would say "no
        // halt on record", which is a *claim about the chain's history* that a backend with nowhere to
        // keep the record cannot make. A refusal leaves the boot seed at `durable: false` — "unknown" —
        // where an empty answer would read as "never halted", the two states W-N10-01 exists to keep
        // apart.
        let Some(store) = &self.reset_marker else {
            return Err(
                "this DAG storage has no durable-records store attached, so this shard's autopropose \
                 halt history cannot be read — unknown, not empty"
                    .to_string(),
            );
        };
        let raw = store
            .get(&[HALT_RECORD_KEY.to_vec()])
            .await?
            .into_iter()
            .next()
            .flatten();
        raw.map(|bytes| HaltRecord::decode(&bytes)).transpose()
    }

    async fn clear_halt_record(&self) -> Result<(), String> {
        let Some(store) = &self.reset_marker else {
            return Ok(());
        };
        store.delete(&[HALT_RECORD_KEY.to_vec()]).await.map(|_| ())
    }

    /// **The shard's monotone health counters, in the same store under a third reserved key** (see
    /// [`HealthCounters`]). The write that stops a restart from understating them (W-N10-02).
    async fn write_health_counters(&self, counters: HealthCounters) -> Result<(), String> {
        let Some(store) = &self.reset_marker else {
            return Err(
                "this DAG storage has no durable-records store attached, so this shard's health \
                 counters would be forgotten by the next process"
                    .to_string(),
            );
        };
        store
            .put(&[(HEALTH_COUNTERS_KEY.to_vec(), counters.encode())])
            .await
    }

    /// The counters this shard last recorded. **Refused when the store is absent**, like
    /// `read_halt_record` and for the same reason: "no counters recorded" is a claim about the shard's
    /// history that a backend with nowhere to keep them cannot make, and an empty answer would read as
    /// "never stalled" rather than "unknown".
    async fn read_health_counters(&self) -> Result<Option<HealthCounters>, String> {
        let Some(store) = &self.reset_marker else {
            return Err(
                "this DAG storage has no durable-records store attached, so this shard's health \
                 counters cannot be read"
                    .to_string(),
            );
        };
        let raw = store
            .get(&[HEALTH_COUNTERS_KEY.to_vec()])
            .await?
            .into_iter()
            .next()
            .flatten();
        raw.map(|bytes| HealthCounters::decode(&bytes)).transpose()
    }

    /// The equivocations the H-1 gate recorded (AUDIT C200) — see the field's own note for why they
    /// live here and only here.
    async fn recorded_equivocations(&self) -> Vec<(Validator, Vec<u8>)> {
        self.equivocations
            .lock()
            .await
            .iter()
            .map(|(sender, bytes)| (*sender, bytes.clone()))
            .collect()
    }

    async fn insert(
        &self,
        block_metadata: BlockMetadata,
        block: BlockMessage,
    ) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        if self
            .block_metadata_store
            .contains(&block_metadata.block_hash)
            .await
        {
            return Ok(());
        }

        // H-1: equivocation detection — a second, distinct block by the same sender reusing a
        // `seq_num`. Reject it before any partial write so an equivocating validator can neither
        // enter the DAG nor stall finalization.
        {
            let repr = self.representation.read().await;
            let equivocating = repr.dag_message_state.msg_map.values().any(|m| {
                m.sender == block_metadata.sender && m.sender_seq == block_metadata.seq_num
            });
            if equivocating {
                // **Record the refused block's header before refusing it** (AUDIT C200). This is the
                // only copy of the offence that will ever exist — the gate returns before any write —
                // and it is what a proposer attaches as evidence so that every other node can check the
                // offence against its own DAG. The header and not the block: the payload lands in a
                // block's *state*, and a refused block's size is whatever its sender chose. A second
                // offence by the same sender replaces the first — one piece of evidence takes the bond,
                // and holding more would be unbounded.
                self.equivocations.lock().await.insert(
                    block_metadata.sender,
                    rchain_models::casper::protocol::casper_message::EquivocationEvidence::from_block(
                        &block,
                    )
                    .encode(),
                );
                return Err(format!(
                    "{EQUIVOCATION_PREFIX}: sender produced two blocks with the same sequence number"
                ));
            }
        }

        // The fringe a block belongs to — the key its `member_of_fringe` mark uses, and the name the
        // readers find a fringe's blocks by (Law 66/68; C250's residue, C270).
        let fringe_hash = rchain_models::block_metadata::fringe_hash_of(&block_metadata.fringe);

        // One read of the representation, and the guard is a *block expression* rather than a
        // binding: everything below is synchronous, so the copy this used to take
        // (`dag_message_state.clone()`, Θ(N²) in the messages' `seen` sets, once per block) bought
        // nothing except releasing the lock — and a binding left in scope would hold the read lock
        // across the metadata write at the end of this function, making that write wait on itself.
        // The borrow checker cannot catch a stale guard; the block can.
        let fringe_diff: BTreeSet<BlockHash> = {
            let repr = self.representation.read().await;
            let msg_map = &repr.dag_message_state.msg_map;
            let mut justifications: BTreeSet<Message<BlockHash, Validator>> = BTreeSet::new();
            for j in &block_metadata.justifications {
                let msg = msg_map
                    .get(j)
                    .ok_or_else(|| "justification not present in message map".to_string())?;
                justifications.insert(msg.clone());
            }
            let prev_fringe = message_map::latest_fringe(msg_map, &justifications);
            let mut fringe_seen: BTreeSet<BlockHash> = BTreeSet::new();
            for f in &block_metadata.fringe {
                let msg = msg_map
                    .get(f)
                    .ok_or_else(|| "fringe block not present in message map".to_string())?;
                fringe_seen.extend(msg.seen.iter().copied());
            }
            let prev_seen: BTreeSet<BlockHash> = prev_fringe
                .iter()
                .flat_map(|m| m.seen.iter().copied())
                .collect();
            fringe_seen.difference(&prev_seen).copied().collect()
        };

        // **The `fringe-data` record is not written any more.** `insert` used to build a `FringeData`
        // from this block's claim and *join* it with whatever a sibling had written at the same fringe
        // key — `join_fringe_records`, with a `min` tie-break on the state when two writers disagreed —
        // and the store returned it last-write-first. That was the cache the ambiguity Law 66 describes
        // came from: one key, many bases, so the value a reader saw depended on the arrival order
        // (C215/C270). The claim is a per-block fact now — this block's `fringe_state_hash`, its
        // `fringe`, and its own `rejected_deploys` — written once under the block's hash, and `insert`
        // short-circuits on a known block, so the write is append-only (Law 68's positive half: a read
        // names a *block*, a version that names a value). With the key gone there is no join left to be
        // a semigroup (Law 67 retired), and the `[fringe-divergence]` report moved to the reader that
        // sees two claims of one fringe disagree
        // (`multi_parent_casper.rs::get_pre_state_for_parents`), where the disagreement is still
        // visible rather than folded.
        //
        // **The injection, now inert.** It perturbed the record this node wrote; there is no
        // node-local derived record left to perturb without also changing a block's content, so the
        // flag is accepted (the devnet CLI still parses it) and *reports itself as armed but inert* on
        // every insert — an operator must not be able to believe an injection happened when it did
        // not. The divergence rig it staged is `up --validators 4 --fresh`, which needs no injection
        // (`spec/audit/evidence/n-anchor-drill/self-reject-stall.txt`).
        if self.merge_divergence_injection > 0 {
            eprintln!(
                "[merge-divergence-injection {}] armed but inert: the fringe record this instrument \
                 perturbed retired with the `fringe-data` store (Law 66/68; C250's residue, C270), so \
                 there is no node-local derived value left to perturb — the drill it staged is \
                 `up --validators 4 --fresh`, which needs no injection",
                self.merge_divergence_injection
            );
        }

        // The deployer index is data too, so it is written **before** the block becomes known (review
        // of #277). In the other order a crash between the two writes left the block known — and
        // `insert` returns early for a known block, so nothing would ever index it again — while the
        // index reported itself complete. This way a crash leaves at worst an entry for a block that
        // is not in the DAG yet; the block is in the block store (the receiver stores it before it
        // inserts), so that entry is still a true "this key signed in a block this node holds", and
        // the re-insert finds it already present.
        if let Some(index) = &self.deployer_index {
            index_deployers(index.as_ref(), &block).await?;
        }

        // Make the block known, with its claim on it. This single write *is* the claim's write now
        // (the F-5 order the record's own write used to need — data before the pointer — has nothing
        // left to order against: the block's metadata carries the claim, so there is no second write
        // a crash could tear it from).
        self.block_metadata_store
            .add(block_metadata.clone())
            .await?;

        // Index each deploy to this block, and remove it from the pending pool so
        // `pooled_deploys` no longer lists it (a deploy leaves the pool once included).
        let deploy_hashes: Vec<DeployId> = block
            .state
            .deploys
            .iter()
            .map(|d| d.deploy.sig.clone())
            .collect();
        if !deploy_hashes.is_empty() {
            // **Which of these were in *this* node's pool.** A block routinely carries deploys this
            // node never held — it was not the proposer, or the deploy expired here first — and
            // returning a slot for one of those would credit a deployer for a slot it never spent
            // (W-N2-04). One batched read, bounded by the block's own deploy count.
            let pooled_here = self.deploy_store.get(&deploy_hashes).await?;
            let released: Vec<Vec<u8>> = pooled_here
                .iter()
                .flatten()
                .map(|d| d.deployer.clone())
                .collect();
            let released_bytes: usize = pooled_here
                .iter()
                .flatten()
                .map(|d| d.data.term.len())
                .sum();

            let pairs: Vec<(DeployId, BlockHash)> = deploy_hashes
                .iter()
                .map(|h| (h.clone(), block.block_hash))
                .collect();
            self.deploy_index.put(&pairs).await?;
            self.deploy_store.delete(&deploy_hashes).await?;
            // The deploy has left the pool, so its share of it goes with it — both the deployer's
            // places and the term bytes, at the same moment the rows are deleted.
            self.return_pooled_bytes(released_bytes);
            for deployer in &released {
                release_deployer_slot(&self.pooled_by_deployer, deployer);
            }
        }

        // Mark the newly-finalized blocks' metadata with their member fringe.
        for h in &fringe_diff {
            let meta = self.block_metadata_store.get_unchecked(h).await?;
            let updated = BlockMetadata {
                member_of_fringe: Some(fringe_hash),
                ..meta
            };
            self.block_metadata_store.add(updated).await?;
        }

        // Update the in-memory DAG representation.
        let dag_set = self.block_metadata_store.dag_set().await;
        let child_map = self.block_metadata_store.child_map_data().await;
        let height_map = self.block_metadata_store.height_map().await;
        let mut prune_cache_ids: Vec<BlockHash> = Vec::new();
        let latest_block_number: i64;
        {
            // The guard is bound so that `Arc::make_mut` has somewhere to borrow the `&mut` from; it
            // is dropped at the end of this block, before the awaits below. `make_mut` copies only
            // when a reader still holds a snapshot, and this function is serialized by `self.lock`,
            // so that is at most one copy per insert — where every reader used to pay one.
            let mut guard = self.representation.write().await;
            let repr = Arc::make_mut(&mut *guard);
            let msg = message_from_block_metadata(&block_metadata, &repr.dag_message_state.msg_map)
                .ok_or_else(|| "justification not present in message map".to_string())?;
            // H-2: a validation-failed block is recorded in the map (for `neglectedInvalidBlock`
            // and justification-regression) but must not become the sender's latest message.
            if block_metadata.validation_failed {
                repr.dag_message_state.insert_msg_without_latest_mut(&msg);
            } else {
                repr.dag_message_state.insert_msg_mut(&msg);
            }
            repr.dag_set = dag_set;
            repr.child_map = child_map;
            repr.height_map = height_map;

            // H-4: when finalization advanced, collect the block-index cache entries prunable
            // below the newly-finalized fringe.
            if !fringe_diff.is_empty() {
                let msg_map = &repr.dag_message_state.msg_map;
                let latest_msgs: BTreeSet<_> = repr
                    .dag_message_state
                    .latest_msgs
                    .values()
                    .cloned()
                    .collect();
                let lowest = message_map::lowest_fringe(msg_map, &latest_msgs);
                let lowest_ids: BTreeSet<BlockHash> = lowest.iter().map(|m| m.id).collect();
                let prunable = message_map::prune_fringe(msg_map, &lowest_ids, &repr.child_map);
                prune_cache_ids = prunable.iter().map(|m| m.id).collect();
            }
            latest_block_number = repr.latest_block_number();
            // Published here, from the representation the write guard already holds — a second read
            // lock would deadlock against the guard this block is inside of.
            self.set_gauges(
                repr.message_count(),
                repr.seen_entries(),
                repr.index_entries(),
                repr.logical_bytes(),
            );
            self.set_merge_shape_gauges();
            self.set_soft_checkpoint_gauges();
            self.set_read_path_gauges();
        }

        BlockIndex::prune_cache(&prune_cache_ids);
        self.expire_deploys(latest_block_number).await?;
        Ok(())
    }

    /// **The update `insert` cannot be** (AUDIT C193) — see the trait's doc for why the restoring rule
    /// needs it. The narrow shape is the point: the block's fringe data is already stored, its hash is
    /// already in `dag_set` and `child_map`, and the *only* thing an update carries is a record whose
    /// `validation_failed` has been cleared. Two structures excluded it while it was failed, and both
    /// have to take it back:
    ///
    /// - **the height map**, which does not count a failed block — re-read from the store after `add`,
    ///   so the store's own contiguity check (Law 18) is the one that decides, rather than a second
    ///   implementation here;
    /// - **`latest_msgs`**, through [`DagMessageState::promote_msg_mut`] — H-2 is what kept the block
    ///   out, and a proposer that never sees it keeps deriving a sequence number it has already spent.
    ///
    /// The H-1 gate is deliberately **not** re-run: this is not an insertion, so there is no second block
    /// claiming the `(sender, seq_num)` — the block was admitted under that rule when it was first
    /// inserted, and the gate's `contains` short-circuit (which made this a no-op) is what the update
    /// exists to step around.
    async fn update_metadata(&self, block_metadata: BlockMetadata) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        let hash = block_metadata.block_hash;
        if !self.block_metadata_store.contains(&hash).await {
            return Err(format!(
                "no block {} to update — `insert` is the call for one that is not there",
                hash.to_hex()
            ));
        }
        // The store first, so a refusal (its contiguity check) leaves the representation untouched —
        // the same order `insert` uses, and for the same reason (AUDIT C172's shape).
        self.block_metadata_store
            .add(block_metadata.clone())
            .await?;

        let mut guard = self.representation.write().await;
        let repr = Arc::make_mut(&mut *guard);
        let msg = message_from_block_metadata(&block_metadata, &repr.dag_message_state.msg_map)
            .ok_or_else(|| "justification not present in message map".to_string())?;
        repr.dag_message_state.promote_msg_mut(&msg);
        // Re-read rather than patch: the store has just applied its own contiguity rule, and reading
        // back is what makes this and `insert` agree by construction rather than by two implementations.
        repr.height_map = self.block_metadata_store.height_map().await;
        Ok(())
    }

    async fn lookup(&self, block_hash: &BlockHash) -> Result<Option<BlockMetadata>, String> {
        self.block_metadata_store.get(block_hash).await
    }

    async fn lookup_by_deploy_id(&self, deploy_id: &DeployId) -> Result<Option<BlockHash>, String> {
        let vals = self.deploy_index.get(&[deploy_id.clone()]).await?;
        Ok(vals.into_iter().next().flatten())
    }

    async fn lookup_by_deployer(&self, deployer_hash: &[u8]) -> Result<DeployerLookup, String> {
        let Some(index) = &self.deployer_index else {
            return Err("this node keeps no deployer index".to_string());
        };
        // Read the coverage first: a backfill that completes between the two reads can only make the
        // answer more complete than the height it is reported with, never less.
        //
        // **Coverage is also bounded by what this node holds.** A node that joined by syncing from a
        // finalized fringe inserts only the blocks above it, so its index — complete for everything
        // it has — knows nothing of the chain below its lowest block. Reporting 0 there would turn
        // "not in my blocks" into "never signed".
        let lowest_held = self
            .get_representation()
            .await
            .height_map
            .keys()
            .next()
            .map_or(0, |h| i64::from(*h));
        let indexed_from = self
            .deployer_indexed_from
            .load(Ordering::SeqCst)
            .max(lowest_held);
        let block = match index
            .get(&[deployer_hash.to_vec()])
            .await?
            .into_iter()
            .next()
            .flatten()
        {
            Some(bytes) => Some(BlockHash::try_from(bytes.as_slice()).map_err(|e| e.to_string())?),
            None => None,
        };
        Ok(DeployerLookup {
            block,
            indexed_from,
        })
    }

    async fn add_deploy(&self, deploy: SignedDeployData) -> Result<(), String> {
        // **The two halves, in one call, for a caller with nothing to do in between.** `deploy` is
        // the caller that has something in between — it parses the term — and it calls the halves
        // separately so the submitter's bounds refuse a flood *before* the node pays to parse it, and
        // so a parse that fails returns the place rather than spending it (W-N2-05, W-N12-03).
        let permit = self.admit_deploy(&deploy.deployer).await?;
        self.commit_deploy(permit, deploy).await
    }

    /// **The submitter's bounds, charged before the term is parsed.**
    ///
    /// The rate is the one property of the submitter the node can hold it to, and the share is the
    /// pool's `MAX_POOLED_DEPLOYS_PER_DEPLOYER` (W-N2-04). Both are claimed here; `Drop for
    /// DeployPermit` gives the share back if the deploy never arrives, which is what makes a refused
    /// parse free rather than a permanently spent place.
    ///
    /// The pool's own capacity is checked here **as well as** at [`Self::commit_deploy`], and the two
    /// halves have different jobs: this one refuses a deploy the node was always going to refuse,
    /// before the parse; that one is under the admission lock, where the count cannot move between the
    /// read and the write and the bound is therefore exact. Making the early check the only one would
    /// reintroduce the C113 overshoot the lock exists to close.
    async fn admit_deploy(&self, deployer: &[u8]) -> Result<DeployPermit, String> {
        if !self.deploy_rate.allow(deployer) {
            return Err("deploy rate limit exceeded for this deployer".to_string());
        }

        // The check and the claim are one operation under the map's own lock, so two concurrent
        // admits for one deployer cannot both see the last place.
        {
            let mut pooled = self.pooled_by_deployer.lock().unpoison();
            let held = pooled.get(deployer).copied().unwrap_or(0);
            if held >= MAX_POOLED_DEPLOYS_PER_DEPLOYER {
                return Err("this deployer holds its share of the deploy pool".to_string());
            }
            pooled.insert(deployer.to_vec(), held + 1);
        }

        if self.deploy_store.count().await? >= MAX_POOLED_DEPLOYS {
            release_deployer_slot(&self.pooled_by_deployer, deployer);
            return Err("deploy pool is full".to_string());
        }
        Ok(DeployPermit::holding(
            self.pooled_by_deployer.clone(),
            deployer.to_vec(),
        ))
    }

    /// Store an admitted deploy, handing the reservation over to it.
    ///
    /// **The count and the insert are ONE commit point.** Held apart — as they were — N concurrent
    /// callers all read a count below the cap and all insert, and the pool exceeds its own bound by
    /// the concurrency width on the node's unauthenticated ingress. `deploy_admission` is that commit
    /// point; the store's lock cannot be it, because it is taken and released by each of `count` and
    /// `put` separately.
    ///
    /// Every failure path here drops the permit, which returns the submitter's place: a store fault is
    /// not the deployer's, and a place that was never taken yet never returned is a permanent lockout.
    async fn commit_deploy(
        &self,
        mut permit: DeployPermit,
        deploy: SignedDeployData,
    ) -> Result<(), String> {
        let _admitted = self.deploy_admission.lock().await;
        // **A duplicate is refused, not overwritten** (B36, W-N2-05). The early reservation in
        // `multi_parent_casper::deploy` keeps a resubmission from reaching the parse, but
        // `add_deploy` is also a direct ingress: without this check the duplicate's `put` overwrote
        // its own row while `permit.commit()` kept the second per-deployer place and the second term
        // bytes, and the single eventual release returned one of each — a permanent lockout of the
        // deployer's share and a byte budget that never drains.
        if self.contains_deploy_in_pool(&deploy.sig).await? {
            return Err(format!(
                "deploy {} is already in the pool",
                rchain_shared::base16::encode(&deploy.sig)
            ));
        }
        if self.deploy_store.count().await? >= MAX_POOLED_DEPLOYS {
            return Err("deploy pool is full".to_string());
        }
        let term_bytes = deploy.data.term.len();
        let in_use = self
            .pooled_term_bytes
            .load(std::sync::atomic::Ordering::Relaxed);
        if in_use.saturating_add(term_bytes) > MAX_POOLED_DEPLOY_BYTES {
            return Err(format!(
                "deploy pool is full: {in_use} of {MAX_POOLED_DEPLOY_BYTES} term bytes in use"
            ));
        }
        self.deploy_store
            .put(&[(deploy.sig.clone(), deploy)])
            .await?;
        self.pooled_term_bytes
            .fetch_add(term_bytes, std::sync::atomic::Ordering::Relaxed);
        // The place is the stored deploy's now: dropping the permit must not return it.
        permit.commit();
        Ok(())
    }

    async fn reserve_deploy_id(&self, deploy_id: &DeployId) -> Result<DeployIdPermit, String> {
        // The store first: a deploy that is already pooled is a duplicate whatever is in flight.
        if self.contains_deploy_in_pool(deploy_id).await? {
            return Err(format!(
                "deploy {} is already in the pool",
                rchain_shared::base16::encode(deploy_id)
            ));
        }
        let mut reservations = self.deploy_reservations.lock().unpoison();
        if !reservations.insert(deploy_id.clone()) {
            return Err(format!(
                "deploy {} is already being admitted",
                rchain_shared::base16::encode(deploy_id)
            ));
        }
        drop(reservations);
        Ok(DeployIdPermit::holding(
            self.deploy_reservations.clone(),
            deploy_id.clone(),
        ))
    }

    async fn pooled_deploys(&self) -> Result<BTreeMap<DeployId, SignedDeployData>, String> {
        self.deploy_store.to_map().await
    }

    async fn contains_deploy_in_pool(&self, deploy_id: &DeployId) -> Result<bool, String> {
        let vals = self.deploy_store.contains(&[deploy_id.clone()]).await?;
        Ok(vals.into_iter().next().unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multi_parent_casper::MAX_DEPLOY_TERM_BYTES;
    use rchain_block_storage::dag::codecs::{
        BlockHashCodec, BlockMetadataCodec, SignedDeployDataCodec,
    };
    use rchain_models::block::state_hash::StateHash;
    use rchain_models::block_metadata::SlashSeverity;
    use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};
    use rchain_shared::store::{InMemoryKeyValueStore, KeyValueStore};
    use rchain_shared::typed_store::{BytesCodec, KeyValueTypedStoreCodec};

    type Shared = Arc<tokio::sync::Mutex<Box<dyn KeyValueStore + Send + Sync>>>;

    fn in_memory() -> Shared {
        Arc::new(tokio::sync::Mutex::new(Box::new(
            InMemoryKeyValueStore::default(),
        )))
    }

    fn hash(byte: u8) -> BlockHash {
        let mut bytes = [0u8; 32];
        bytes[0] = byte;
        BlockHash::new(bytes)
    }

    fn meta(hash: BlockHash, parents: &[BlockHash], block_num: i64) -> BlockMetadata {
        BlockMetadata {
            block_hash: hash,
            block_num: BlockHeight::try_from(block_num).unwrap(),
            sender: Validator::new([0u8; 65]),
            seq_num: 0.try_into().unwrap(),
            justifications: parents.iter().copied().collect(),
            bonds_map: BTreeMap::new(),
            validated: true,
            validation_failed: false,
            slashable: false,
            failure_cause: None,
            slash_severity: SlashSeverity::Unspecified,
            restore_attempts: 0,
            fringe: BTreeSet::new(),
            fringe_state_hash: StateHash::new([0u8; 32]),
            member_of_fringe: None,
        }
    }

    fn block(hash: BlockHash) -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: hash,
            block_number: 0.try_into().unwrap(),
            sender: Validator::new([0u8; 65]),
            seq_num: 0.try_into().unwrap(),
            pre_state_hash: StateHash::new([0u8; 32]),
            post_state_hash: StateHash::new([0u8; 32]),
            justifications: vec![],
            bonds: BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: rchain_models::casper::protocol::casper_message::RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![],
            timestamp: 0,
        }
    }

    async fn empty_metadata_store() -> Arc<BlockMetadataStore> {
        Arc::new(
            BlockMetadataStore::create(Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BlockHashCodec),
                Arc::new(BlockMetadataCodec),
            )))
            .await
            .unwrap(),
        )
    }

    async fn build_storage_over(
        metadata_store: Arc<BlockMetadataStore>,
    ) -> Arc<BlockDagKeyValueStorage> {
        Arc::new(build_storage_over_unshared(metadata_store).await)
    }

    /// The same storage, not yet wrapped in an `Arc` — so a test can attach a metrics sink
    /// (`with_metrics` consumes it) the way the node does.
    async fn build_storage_over_unshared(
        metadata_store: Arc<BlockMetadataStore>,
    ) -> BlockDagKeyValueStorage {
        let deploy_index: Arc<dyn KeyValueTypedStore<DeployId, BlockHash>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BytesCodec),
                Arc::new(BlockHashCodec),
            ));
        let deploy_store: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BytesCodec),
                Arc::new(SignedDeployDataCodec),
            ));
        // **The reset marker is attached here because a reset needs somewhere to record itself**
        // (F-U5-01, C274): `rewind_and_purge` refuses when the store is absent, so a fixture that
        // drives one attaches it — the same reason the node does.
        let reset_marker: Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>> = Arc::new(
            KeyValueTypedStoreCodec::new(in_memory(), Arc::new(BytesCodec), Arc::new(BytesCodec)),
        );
        BlockDagKeyValueStorage::create(metadata_store, deploy_index, deploy_store)
            .await
            .unwrap()
            .with_reset_marker(reset_marker)
    }

    async fn build_storage() -> Arc<BlockDagKeyValueStorage> {
        build_storage_over(empty_metadata_store().await).await
    }

    /// **A rewind takes the node back to the anchor, in the view every dependency check reads.**
    ///
    /// The DAG is built the way a *restart* finds one — blocks added to the metadata store, then a
    /// storage created over it — because that is the state a node that cannot heal is actually in, and
    /// because it makes the assertion about the rebuilt representation rather than about the insert
    /// path. The blocks are a chain, so "above the anchor" is unambiguous.
    ///
    /// Falsifier: as with the metadata-store half, the capability is absent on the pre-fix tree and the
    /// test does not build there. What the test then pins is the part a reboot would have caught: after
    /// the rewind the *representation* — not just the store — no longer contains the dropped blocks,
    /// which is what `has_all_deps` and the sync path read.
    #[tokio::test]
    async fn a_rewind_to_the_anchor_takes_the_tip_out_of_the_representation() {
        let metadata = empty_metadata_store().await;
        let mut parents: Vec<BlockHash> = Vec::new();
        for n in 0..=4u8 {
            let h = hash(n);
            // One sender, so the sequence number is what keeps H-1's gate out of the way: a chain is
            // the same proposer's blocks, and `meta`'s `seq_num: 0` would be an equivocation.
            let mut m = meta(h, &parents, i64::from(n));
            m.seq_num = i64::from(n).try_into().unwrap();
            metadata
                .add(m)
                .await
                .expect("the stored chain is contiguous");
            parents = vec![h];
        }
        let storage = build_storage_over(Arc::clone(&metadata)).await;

        assert!(
            storage.get_representation().await.contains(&hash(4)),
            "the node sees the tip to begin with"
        );

        let anchor = BlockHeight::try_from(2).unwrap();
        let dropped = storage.drop_above(anchor).await.unwrap();
        assert_eq!(
            dropped.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([hash(3), hash(4)])
        );

        let repr = storage.get_representation().await;
        assert!(!repr.contains(&hash(4)), "the tip is gone from the view");
        assert!(!repr.contains(&hash(3)));
        assert!(
            repr.contains(&hash(2)),
            "the anchor survives — it is the state replayed from"
        );
        assert_eq!(
            repr.latest_block_number(),
            3,
            "three blocks left, at heights 0, 1 and 2"
        );
        drop(repr);

        assert!(
            metadata.get(&hash(4)).await.unwrap().is_none(),
            "and the store agrees, which is what the next boot reads"
        );
    }

    /// **The reset's durable half: the DAG and the bodies go together, and a fresh view sees it**
    /// (#320).
    ///
    /// The test lives in this module because the fixture that builds a DAG over an in-memory metadata
    /// store is here — the subject is `engine::reset::rewind_and_purge`, which composes the rewind this
    /// module already tests with the *body* purge that `BlockStore` needs and `drop_above` cannot do
    /// (it does not own that store).
    ///
    /// **What it pins, and it is the half that undoes a reset if it is missing.** A body left behind is
    /// read by `block_receiver::not_validated` as "stored but not yet validated" and re-injected by
    /// `send_to_validate`, so the next child of a dropped block arriving re-inserts it above the anchor.
    /// The assertion is therefore about the *store*, not about the DAG: after the reset the bodies are
    /// gone, so there is nothing for a peer to be served and nothing for the receiver to re-inject.
    ///
    /// The durability half is the second assertion: a **fresh** storage built over the same metadata
    /// store sees the rewound chain. An in-memory-only rewind would be undone by the next boot, which is
    /// the shape `drop_above`'s own test establishes for the DAG alone.
    #[tokio::test]
    async fn the_reset_rewinds_the_dag_and_removes_the_bodies_it_dropped() {
        let metadata = empty_metadata_store().await;
        let mut parents: Vec<BlockHash> = Vec::new();
        for n in 0..5u8 {
            let h = hash(n);
            let store = build_storage_over(Arc::clone(&metadata)).await;
            // The sequence number climbs with the height: `insert`'s H-1 gate refuses two blocks from
            // one sender at one sequence number, and a chain is not an equivocation.
            let mut m = meta(h, &parents, i64::from(n));
            m.seq_num = i64::from(n).try_into().unwrap();
            store.insert(m, block(h)).await.expect("the chain inserts");
            parents = vec![h];
        }
        let storage = build_storage_over(Arc::clone(&metadata)).await;

        // The bodies, as a serving node holds them — through the same store the receiver reads.
        let manager = rchain_shared::store_manager::InMemoryStoreManager::default();
        let block_store = rchain_block_storage::block_store::create(&manager)
            .await
            .expect("an in-memory block store");
        for n in 0..5u8 {
            block_store
                .put(&[(hash(n), block(hash(n)))])
                .await
                .expect("the body is stored");
        }

        let anchor = BlockHeight::try_from(2).unwrap();
        let dag: Arc<dyn BlockDagStorage> = storage.clone();
        let (rewound, removed) = crate::engine::reset::rewind_and_purge(&dag, &block_store, anchor)
            .await
            .expect("the reset's durable half succeeds over healthy stores");

        assert_eq!(
            (rewound, removed),
            (2, 2),
            "heights 3 and 4 go, both halves"
        );
        assert!(
            !storage.get_representation().await.contains(&hash(4)),
            "the view is at the anchor"
        );
        assert!(
            storage.get_representation().await.contains(&hash(2)),
            "and the anchor survives — it is the state replayed from"
        );
        for n in 3..5u8 {
            assert!(
                !block_store
                    .contains(&[hash(n)])
                    .await
                    .expect("the store answers")
                    .first()
                    .copied()
                    .unwrap_or(false),
                "the body of height {n} is gone, so it can be neither served nor re-injected"
            );
        }
        assert!(
            block_store
                .contains(&[hash(2)])
                .await
                .expect("the store answers")
                .first()
                .copied()
                .unwrap_or(false),
            "the anchor's own body stays — the reset keeps what it rewinds to"
        );

        // The next boot reads the metadata store, so a rewind that lived only in memory would be
        // undone by it. This is the assertion that makes the test about the reset rather than about a
        // flag that was set.
        let rebooted = build_storage_over(Arc::clone(&metadata)).await;
        let repr = rebooted.get_representation().await;
        assert!(
            !repr.contains(&hash(4)),
            "a fresh view sees the rewound chain"
        );
        assert!(
            repr.contains(&hash(2)),
            "down to the anchor, which survives"
        );
    }

    /// **A rewind clears a survivor's orphaned `member_of_fringe` mark** — the one new invariant the
    /// per-block shape introduces (Law 66/68; C250's residue, C270).
    ///
    /// A block's mark names the fringe it belongs to, and a fringe *is* `X.fringe` for the block(s) X
    /// that finalised it. Rewinding above the anchor drops those finalisers, so a survivor whose mark
    /// names a fringe no surviving block declares would be holding a claim the rewind removed — and
    /// because the mark travels on the block, a reader resolving it would *find* that orphan and read
    /// a claim out of a DAG that no longer has it. The old store could not do this: a record was read
    /// *by* its key, and a key with no finalisers was simply never consulted.
    ///
    /// The chain is `g → a → b`, each block declaring the previous as its fringe (so `insert`'s
    /// `fringe_diff` marks each finalised block with the fringe's own key). Rewinding to `a` drops only
    /// `b`, which declared the fringe `{a}` that `a`'s mark names — so `a`'s mark is the orphan and is
    /// cleared, while `g`'s mark names `{g}`, still declared by the surviving `a`, and is the control
    /// that the pass clears orphans rather than everything.
    #[tokio::test]
    async fn a_rewind_clears_a_survivors_orphaned_member_of_fringe_mark() {
        let storage = build_storage().await;
        let (g, a, b) = (hash(0), hash(1), hash(2));
        let mark = |h: BlockHash| {
            Some(rchain_models::block_metadata::fringe_hash_of(
                &[h].into_iter().collect(),
            ))
        };

        // g: no parents, empty fringe; a: justifies g, declares fringe {g}; b: justifies a, declares
        // fringe {a}. Sequence numbers climb so H-1's gate (same sender, same seq) stays out of it.
        storage.insert(meta(g, &[], 0), block(g)).await.unwrap();
        let mut a_meta = meta(a, &[g], 1);
        a_meta.seq_num = 1.try_into().unwrap();
        a_meta.fringe = [g].into_iter().collect();
        storage.insert(a_meta, block(a)).await.unwrap();
        let mut b_meta = meta(b, &[a], 2);
        b_meta.seq_num = 2.try_into().unwrap();
        b_meta.fringe = [a].into_iter().collect();
        storage.insert(b_meta, block(b)).await.unwrap();

        // Before the rewind: `a` is marked with the fringe it belongs to (`{a}`, declared by `b`) and
        // `g` with `{g}` (declared by `a`).
        assert_eq!(
            storage.lookup(&a).await.unwrap().unwrap().member_of_fringe,
            mark(a)
        );
        assert_eq!(
            storage.lookup(&g).await.unwrap().unwrap().member_of_fringe,
            mark(g)
        );

        // Rewind to the anchor `a`: height 2 goes, so `b` — the sole declarer of `{a}` — goes with it.
        let dropped = storage
            .drop_above(BlockHeight::try_from(1).unwrap())
            .await
            .unwrap();
        assert_eq!(dropped, vec![b], "only the tip is above the anchor");

        assert_eq!(
            storage.lookup(&a).await.unwrap().unwrap().member_of_fringe,
            None,
            "the orphaned mark is cleared — a rewound block's claim must not re-enter the view"
        );
        assert_eq!(
            storage.lookup(&g).await.unwrap().unwrap().member_of_fringe,
            mark(g),
            "and a mark whose fringe a survivor still declares is kept — the pass clears orphans, not \
             everything"
        );
    }

    #[tokio::test]
    async fn insert_and_lookup_round_trip() {
        let storage = build_storage().await;
        let genesis_hash = hash(0);
        storage
            .insert(meta(genesis_hash, &[], 0), block(genesis_hash))
            .await
            .unwrap();

        assert_eq!(
            storage
                .lookup(&genesis_hash)
                .await
                .unwrap()
                .unwrap()
                .block_hash,
            genesis_hash
        );
        assert!(storage.lookup(&hash(9)).await.unwrap().is_none());
        assert!(storage.get_representation().await.contains(&genesis_hash));
    }

    #[tokio::test]
    async fn insert_rejects_equivocation_same_seq_num() {
        let storage = build_storage().await;
        let first = hash(1);
        let second = hash(2);

        storage
            .insert(meta(first, &[], 0), block(first))
            .await
            .unwrap();

        // A second, distinct block by the same sender reusing `seq_num` is rejected (H-1), before
        // any partial write.
        let err = storage.insert(meta(second, &[], 0), block(second)).await;
        assert!(err.is_err(), "equivocation must be rejected");
        assert!(
            err.unwrap_err().contains("equivocation"),
            "error should mention equivocation"
        );

        // The first block's metadata is still present and unchanged.
        let stored = storage.lookup(&first).await.unwrap().unwrap();
        assert_eq!(stored.block_hash, first);

        // **And the refused block's header is recorded as evidence** (AUDIT C200). This gate is the only
        // place the offence will ever exist — it returned before any write — and it is what a proposer
        // attaches so that every *other* node can re-check the offence against its own DAG. Red before
        // this: nothing was kept, so an equivocation could be refused and then never punished.
        let recorded = storage.recorded_equivocations().await;
        assert_eq!(recorded.len(), 1, "the refused block is kept as evidence");
        let (sender, bytes) = &recorded[0];
        assert_eq!(
            *sender,
            meta(second, &[], 0).sender,
            "keyed by the validator that equivocated"
        );
        let decoded =
            rchain_models::casper::protocol::casper_message::EquivocationEvidence::decode(bytes)
                .expect("the recorded evidence decodes");
        assert_eq!(
            decoded.block_hash, second,
            "and it is the *refused* block's own hash, not the one the DAG holds"
        );
    }

    #[tokio::test]
    async fn deploy_pool_round_trip() {
        let storage = build_storage().await;
        let deploy = SignedDeployData {
            data: rchain_models::casper::protocol::casper_message::DeployData {
                attachments: Vec::new(),
                term: "Nil".to_string(),
                timestamp: 0,
                phlo_price: 1,
                phlo_limit: 1,
                valid_after_block_number: 0,
                shard_id: "root".to_string(),
            },
            deployer: vec![1, 2, 3],
            sig: vec![9, 9, 9],
            sig_algorithm: "secp256k1".to_string(),
        };
        storage.add_deploy(deploy.clone()).await.unwrap();
        assert!(storage.contains_deploy_in_pool(&deploy.sig).await.unwrap());
        assert_eq!(
            storage.lookup_by_deploy_id(&deploy.sig).await.unwrap(),
            None
        );

        let pooled = storage.pooled_deploys().await.unwrap();
        assert_eq!(pooled[&deploy.sig], deploy);
    }

    #[tokio::test]
    async fn insert_removes_included_deploy_from_pool() {
        let storage = build_storage().await;
        let signed = deploy_with_id(7);
        storage.add_deploy(signed.clone()).await.unwrap();
        assert!(storage.contains_deploy_in_pool(&signed.sig).await.unwrap());
        assert_eq!(storage.pooled_term_bytes(), signed.data.term.len());

        let processed = rchain_models::casper::protocol::casper_message::ProcessedDeploy {
            deploy: signed.clone(),
            cost: rchain_models::casper::protocol::casper_message::PCost { cost: 0 },
            deploy_log: vec![],
            is_failed: false,
            system_deploy_error: None,
        };
        let block_hash = hash(1);
        let mut b = block(block_hash);
        b.state.deploys = vec![processed];
        storage.insert(meta(block_hash, &[], 1), b).await.unwrap();

        assert!(
            !storage.contains_deploy_in_pool(&signed.sig).await.unwrap(),
            "an included deploy must leave the pool"
        );
        assert_eq!(
            storage.lookup_by_deploy_id(&signed.sig).await.unwrap(),
            Some(block_hash)
        );
        assert!(storage.pooled_deploys().await.unwrap().is_empty());
        assert_eq!(
            storage.pooled_term_bytes(),
            0,
            "the included deploy's term bytes leave the budget with its row"
        );
    }

    /// **A rewind repairs the derived indexes against the surviving block set, not the dropped list**
    /// (B39, HAZOP F-U17-11).
    ///
    /// A crash between the metadata drop and the index cleanup leaves a row naming a block the DAG no
    /// longer holds. The row is not inert: the proposer's replay guard (`select_deploys`) reads
    /// `lookup_by_deploy_id`, treats "in the DAG" as "already included", and silently skips the pooled
    /// deploy. The repair must remove it even when the rewind itself drops nothing — which is the shape
    /// the old early return missed.
    ///
    /// Falsifier: keep the early return and the second call removes nothing; the deploy stays
    /// unselectable.
    #[tokio::test]
    async fn repair_removes_stale_entries_and_releases_the_pool_slot() {
        let storage = build_storage_over_unshared(empty_metadata_store().await).await;
        let deploy = deploy_by(0, 7);
        storage.add_deploy(deploy.clone()).await.unwrap();
        storage
            .deploy_index
            .put(&[(deploy.sig.clone(), hash(42))])
            .await
            .unwrap();
        assert_eq!(
            storage.lookup_by_deploy_id(&deploy.sig).await.unwrap(),
            Some(hash(42)),
            "the fixture is the row a crash after the metadata drop leaves"
        );

        let dropped = storage
            .drop_above(BlockHeight::try_from(0).unwrap())
            .await
            .unwrap();
        assert!(dropped.is_empty(), "nothing is above the anchor");
        assert_eq!(
            storage.lookup_by_deploy_id(&deploy.sig).await.unwrap(),
            None,
            "the stale row is gone, so the replay guard no longer skips the pooled deploy"
        );
        assert!(
            storage
                .pooled_deploys()
                .await
                .unwrap()
                .contains_key(&deploy.sig),
            "the deploy was never included, so its pool place is still its own"
        );
        assert_eq!(
            storage.repair_derived_indexes().await.unwrap(),
            (0, 0),
            "the repair is idempotent"
        );
    }

    /// **The crash the early return left behind**: metadata dropped directly (the pre-fix shape — the
    /// derived cleanup never ran), so the index still names a block the DAG does not hold. The audit
    /// removes it, which is what the reset bracket's `Rewinding` branch and the boot hook call.
    #[tokio::test]
    async fn a_crash_before_the_index_cleanup_is_repaired_by_the_audit() {
        let storage = build_storage_over_unshared(empty_metadata_store().await).await;
        let deploy = deploy_by(0, 11);
        storage.add_deploy(deploy.clone()).await.unwrap();
        let h = hash(3);
        let processed = rchain_models::casper::protocol::casper_message::ProcessedDeploy {
            deploy: deploy.clone(),
            cost: rchain_models::casper::protocol::casper_message::PCost { cost: 0 },
            deploy_log: vec![],
            is_failed: false,
            system_deploy_error: None,
        };
        let mut b = block(h);
        b.state.deploys = vec![processed];
        storage.insert(meta(h, &[], 3), b).await.unwrap();
        assert_eq!(
            storage.lookup_by_deploy_id(&deploy.sig).await.unwrap(),
            Some(h)
        );

        // The crash window: the metadata drop ran, the index cleanup did not.
        let dropped = storage
            .block_metadata_store
            .drop_above(BlockHeight::try_from(0).unwrap())
            .await
            .unwrap();
        assert_eq!(dropped, vec![h]);
        assert_eq!(
            storage.lookup_by_deploy_id(&deploy.sig).await.unwrap(),
            Some(h),
            "the stale row is the crash's residue"
        );
        assert_eq!(storage.repair_derived_indexes().await.unwrap(), (1, 0));
        assert_eq!(
            storage.lookup_by_deploy_id(&deploy.sig).await.unwrap(),
            None,
            "the audit repairs the row the crash left"
        );
    }

    /// The audit's subject is the surviving set: a row for a block this DAG never held is removed, and
    /// the deployer index is repaired by the same rule.
    #[tokio::test]
    async fn a_rewind_derives_cleanup_from_survivors_not_the_dropped_list() {
        let deployer_index: Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>> = Arc::new(
            KeyValueTypedStoreCodec::new(in_memory(), Arc::new(BytesCodec), Arc::new(BytesCodec)),
        );
        let seed = deployer_index.clone();
        let storage = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deployer_index(deployer_index)
            .await
            .expect("the deployer index attaches");
        let deploy = deploy_by(1, 9);
        storage
            .deploy_index
            .put(&[(deploy.sig.clone(), hash(77))])
            .await
            .unwrap();
        seed.put(&[(vec![9u8], hash(77).as_bytes().to_vec())])
            .await
            .unwrap();

        assert_eq!(storage.repair_derived_indexes().await.unwrap(), (1, 1));
        assert_eq!(storage.repair_derived_indexes().await.unwrap(), (0, 0));
    }

    fn deploy_with_id(id: usize) -> SignedDeployData {
        SignedDeployData {
            data: rchain_models::casper::protocol::casper_message::DeployData {
                attachments: Vec::new(),
                term: "Nil".to_string(),
                timestamp: 0,
                phlo_price: 1,
                phlo_limit: 1,
                valid_after_block_number: 0,
                shard_id: "root".to_string(),
            },
            deployer: vec![0],
            sig: id.to_le_bytes().to_vec(),
            sig_algorithm: "secp256k1".to_string(),
        }
    }

    #[tokio::test]
    async fn pool_drops_a_deploy_before_the_first_block_that_would_reject_it() {
        let storage = build_storage().await;
        let mut deploy = deploy_with_id(1);
        deploy.data.valid_after_block_number = 1;
        let id = deploy.sig.clone();
        storage.add_deploy(deploy).await.unwrap();

        storage.expire_deploys(50).await.unwrap();
        assert!(
            storage.contains_deploy_in_pool(&id).await.unwrap(),
            "the deploy is still valid for block 50"
        );

        storage.expire_deploys(51).await.unwrap();
        assert!(
            !storage.contains_deploy_in_pool(&id).await.unwrap(),
            "block 51 validation rejects valid_after=1 at lifespan 50, so the proposer pool must remove it before block 51 can select it"
        );
    }

    /// **The pool is expired on the insert path, with no finalisation driven** (W-N2-07).
    ///
    /// The test above drives `expire_deploys` directly, so it pins the function and not its placement.
    /// The row's falsifier asks for the case where *nothing finalises*: on a chain whose fringe has not
    /// moved — a quiet net, a validator that is the only one speaking — nothing goes final, so a pool
    /// that expired deploys only when a block was finalised would keep handing the proposer work the
    /// next block must reject, and hold it for the process lifetime. `insert` expires the pool on every
    /// insert (`dag.rs`'s `self.expire_deploys(latest_block_number)`), and this is the assertion that it
    /// is *there*: delete the call and this fails while the direct test above still passes.
    #[tokio::test]
    async fn inserting_a_block_expires_the_pool_with_no_finalisation() {
        let storage = build_storage().await;
        let mut deploy = deploy_with_id(1);
        deploy.data.valid_after_block_number = 1;
        let id = deploy.sig.clone();
        storage.add_deploy(deploy).await.unwrap();
        assert!(
            storage.contains_deploy_in_pool(&id).await.unwrap(),
            "the deploy is pooled and nothing has happened yet"
        );

        // One block at height 51, inserted. No finalisation is driven anywhere in this test.
        let h = hash(51);
        storage
            .insert(meta(h, &[], 51), block(h))
            .await
            .expect("the insert path accepts a block");
        assert!(
            !storage.contains_deploy_in_pool(&id).await.unwrap(),
            "inserting past the lifespan must expire the pool: block 51 rejects valid_after=1, and \
             nothing finalised to tell the pool so"
        );
    }

    #[tokio::test]
    async fn add_deploy_rejects_when_pool_full() {
        // The pool's COUNT bound, told apart from the submitter's RATE bound **and from its share**.
        // It used to say "one key fills it", and that was the defect: the line below now needs ten
        // distinct deployers to fill ten thousand slots, which is the bound doing its job — see
        // `one_deployer_cannot_hold_the_whole_pool` for the other half.
        let storage = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deploy_rate(u64::MAX, MAX_TRACKED_DEPLOYERS);
        let distinct_deployers = MAX_POOLED_DEPLOYS.div_ceil(MAX_POOLED_DEPLOYS_PER_DEPLOYER);
        for id in 0..MAX_POOLED_DEPLOYS {
            storage
                .add_deploy(deploy_by((id / MAX_POOLED_DEPLOYS_PER_DEPLOYER) as u8, id))
                .await
                .unwrap();
        }
        assert!(
            distinct_deployers >= 2,
            "if one deployer could fill the pool this test would not be testing the pool's bound"
        );
        let err = storage
            .add_deploy(deploy_by(distinct_deployers as u8, MAX_POOLED_DEPLOYS))
            .await;
        assert!(err.is_err(), "deploy pool must reject once full");
        assert!(err.unwrap_err().contains("full"));
    }

    /// **One deployer cannot hold the whole pool** (W-N2-04, W-N2-10).
    ///
    /// The pool's cap was a node-wide count with a node-wide refusal, so one identity that paced
    /// itself under the rate limit owned every slot and every other identity was then refused by a
    /// *full pool* — the bound held, and the wrong counterparty paid for it. The share is per
    /// deployer, and this pins both halves of that: the deployer that reaches its share is refused,
    /// and a different deployer is admitted while the pool as a whole is barely used.
    #[tokio::test]
    async fn one_deployer_cannot_hold_the_whole_pool() {
        let storage = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deploy_rate(u64::MAX, MAX_TRACKED_DEPLOYERS);

        for id in 0..MAX_POOLED_DEPLOYS_PER_DEPLOYER {
            storage.add_deploy(deploy_by(0, id)).await.unwrap();
        }
        let err = storage
            .add_deploy(deploy_by(0, MAX_POOLED_DEPLOYS_PER_DEPLOYER))
            .await
            .expect_err("a deployer at its share must be refused");
        assert!(
            err.contains("share"),
            "and the refusal must name the share rather than the pool — the pool is 90% empty: {err}"
        );

        // The pool is nearly empty *and* another deployer is untouched: both are what makes this a
        // per-submitter bound rather than a node-wide one.
        assert!(
            storage.deploy_store.count().await.unwrap() < MAX_POOLED_DEPLOYS,
            "the share must refuse long before the pool's own cap"
        );
        storage.add_deploy(deploy_by(1, 999_999)).await.unwrap();
    }

    /// **The share comes back when the deploy leaves the pool** (W-N2-04).
    ///
    /// The failure this guards is the one that turns a quota into a permanent lockout: a share that
    /// is claimed on the way in and never returned. Both removal paths return it — expiry here, and
    /// inclusion in a block in the test below — and the deployer must be admitted again afterwards.
    #[tokio::test]
    async fn a_deployers_share_returns_when_its_deploys_leave_the_pool() {
        let storage = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deploy_rate(u64::MAX, MAX_TRACKED_DEPLOYERS);

        for id in 0..MAX_POOLED_DEPLOYS_PER_DEPLOYER {
            storage.add_deploy(deploy_by(0, id)).await.unwrap();
        }
        assert!(storage
            .add_deploy(deploy_by(0, MAX_POOLED_DEPLOYS_PER_DEPLOYER + 1))
            .await
            .is_err());

        // Every pooled deploy expires at this height (`valid_after_block_number` is 0 and the
        // lifespan is 50), so the whole share returns at once.
        storage.expire_deploys(1_000).await.unwrap();

        storage
            .add_deploy(deploy_by(0, 999_999))
            .await
            .expect("a deployer whose deploys have left the pool must be admitted again");
    }

    /// A deploy store that answers `count` and *then* suspends, so the capacity check and the insert
    /// are separated by a real suspension point.
    ///
    /// The suspension is after the read, deliberately: a store that suspends before reading hides the
    /// race, because the callers re-read a count that has already moved. Any store with I/O behind it
    /// (LMDB through the node's typed-store layer) has the other shape — the read is taken, the task
    /// yields, and the count it is holding is stale by the time the insert runs.
    struct YieldingCountStore(Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>>);

    #[async_trait]
    impl KeyValueTypedStore<DeployId, SignedDeployData> for YieldingCountStore {
        async fn get(&self, keys: &[DeployId]) -> Result<Vec<Option<SignedDeployData>>, String> {
            self.0.get(keys).await
        }
        async fn put(&self, pairs: &[(DeployId, SignedDeployData)]) -> Result<(), String> {
            self.0.put(pairs).await
        }
        async fn delete(&self, keys: &[DeployId]) -> Result<usize, String> {
            self.0.delete(keys).await
        }
        async fn contains(&self, keys: &[DeployId]) -> Result<Vec<bool>, String> {
            self.0.contains(keys).await
        }
        async fn to_map(&self) -> Result<BTreeMap<DeployId, SignedDeployData>, String> {
            self.0.to_map().await
        }
        async fn count(&self) -> Result<usize, String> {
            let n = self.0.count().await?;
            tokio::task::yield_now().await;
            Ok(n)
        }
    }

    /// **The pool's bound is a bound under concurrency** (the C113 class, one bound over).
    ///
    /// The capacity check and the insert were two independent acquisitions of the store's lock, so
    /// every caller in flight read a count below the cap and every one of them inserted — the pool
    /// overshot `MAX_POOLED_DEPLOYS` by the concurrency width, on the node's *unauthenticated*
    /// ingress. Eight callers against a pool one below the cap must admit exactly one, and the pool
    /// must land on its cap rather than above it. Remove the `deploy_admission` guard and this test
    /// admits eight.
    #[tokio::test]
    async fn add_deploy_admits_exactly_one_when_the_pool_is_one_below_the_cap() {
        let inner: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BytesCodec),
                Arc::new(SignedDeployDataCodec),
            ));
        let deploy_store: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>> =
            Arc::new(YieldingCountStore(inner.clone()));
        let deploy_index: Arc<dyn KeyValueTypedStore<DeployId, BlockHash>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BytesCodec),
                Arc::new(BlockHashCodec),
            ));
        let storage = BlockDagKeyValueStorage::create(
            empty_metadata_store().await,
            deploy_index,
            deploy_store,
        )
        .await
        .unwrap()
        .with_deploy_rate(u64::MAX, MAX_TRACKED_DEPLOYERS);

        // **Filled by ten distinct deployers, and the concurrent callers are eight more.** One key
        // cannot fill this pool any more (see `one_deployer_cannot_hold_the_whole_pool`), so filling
        // it takes ten — and the callers below must be fresh deployers, or the *share* bound would
        // refuse them and this would stop testing the pool's.
        for id in 0..MAX_POOLED_DEPLOYS - 1 {
            storage
                .add_deploy(deploy_by((id / MAX_POOLED_DEPLOYS_PER_DEPLOYER) as u8, id))
                .await
                .unwrap();
        }

        let base = MAX_POOLED_DEPLOYS;
        let (r1, r2, r3, r4, r5, r6, r7, r8) = tokio::join!(
            storage.add_deploy(deploy_by(10, base)),
            storage.add_deploy(deploy_by(11, base + 1)),
            storage.add_deploy(deploy_by(12, base + 2)),
            storage.add_deploy(deploy_by(13, base + 3)),
            storage.add_deploy(deploy_by(14, base + 4)),
            storage.add_deploy(deploy_by(15, base + 5)),
            storage.add_deploy(deploy_by(16, base + 6)),
            storage.add_deploy(deploy_by(17, base + 7)),
        );
        let admitted = [r1, r2, r3, r4, r5, r6, r7, r8]
            .iter()
            .filter(|r| r.is_ok())
            .count();
        assert_eq!(
            admitted, 1,
            "eight concurrent callers against a pool one below the cap must admit exactly one"
        );
        assert_eq!(
            inner.count().await.unwrap(),
            MAX_POOLED_DEPLOYS,
            "the pool must land exactly on its cap, not above it"
        );
    }

    /// The same deploy from a named deployer, so the admission bound's *subject* can be varied.
    /// The same deploy with a chosen term, so a test can hand the parser something it must not reach.
    fn deploy_saying(deployer: u8, term: &str) -> SignedDeployData {
        let mut deploy = deploy_by(deployer, 0);
        deploy.data.term = term.to_string();
        deploy
    }

    /// **A deploy the submitter's bounds refuse is never parsed** (W-N2-05, W-N12-03).
    ///
    /// The ingress used to parse and fully normalize the term *first* and only then ask whether the
    /// deploy could be admitted — so every caller, including one the node was always going to refuse,
    /// bought the parse. On an unauthenticated ingress that is the amplification: the work happens
    /// before the decision that would have avoided it.
    ///
    /// The second deploy is refused by the *rate* and also carries a term that cannot parse, so the
    /// two orderings produce different errors and the assertion can tell them apart. Falsified by
    /// restoring the parse-first order: the error becomes `Error in parsing term`, which is exactly
    /// the work this rules out.
    ///
    /// It lives in this module's tests because the storage builders do; the function under test is
    /// `multi_parent_casper::deploy`, which is the one caller that has something between the halves.
    #[tokio::test]
    async fn a_deploy_the_admission_refuses_is_never_parsed() {
        let storage = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deploy_rate(1, MAX_TRACKED_DEPLOYERS);

        crate::multi_parent_casper::deploy(&storage, &deploy_saying(0, "Nil"))
            .await
            .expect("the first deploy is inside the rate");

        // A *different* deploy: same key, different id, so the duplicate reservation does not answer
        // and the rate bound is what refuses it (B36's reservation would otherwise mask this test).
        let mut second = deploy_by(0, 1);
        second.data.term = "(((".to_string();
        let err = crate::multi_parent_casper::deploy(&storage, &second)
            .await
            .expect_err("the second is over the rate");
        assert!(
            err.0.contains("rate limit"),
            "the refusal must be the admission's: {}",
            err.0
        );
        assert!(
            !err.0.contains("parsing term"),
            "and the term must never have been parsed: {}",
            err.0
        );
    }

    /// **The pool's byte budget is exact and refuses before the store** (B36).
    ///
    /// The count bound treats a 1 KiB deploy and a 64 KiB deploy alike; the host does not. Filling
    /// the budget with maximal terms — each one *inside* the term bound, so nothing here is refused
    /// for size — leaves exactly [`MAX_POOLED_DEPLOY_BYTES`], and the next byte is refused. Two
    /// deployers are used because one may hold at most `MAX_POOLED_DEPLOYS_PER_DEPLOYER` places, and
    /// 64 MiB of maximal terms is 1,024 deploys: fewer than the pool's count bound, which is the
    /// point of having a second bound.
    ///
    /// Falsifier: remove the byte check in `commit_deploy` and the last `add_deploy` succeeds.
    #[tokio::test]
    async fn a_pool_at_its_byte_cap_refuses_before_the_store() {
        let storage = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deploy_rate(u64::MAX, MAX_TRACKED_DEPLOYERS);
        let term = " ".repeat(MAX_DEPLOY_TERM_BYTES);
        let maximal_deploys = MAX_POOLED_DEPLOY_BYTES / MAX_DEPLOY_TERM_BYTES;
        for id in 0..maximal_deploys {
            let deployer = u8::try_from(id / MAX_POOLED_DEPLOYS_PER_DEPLOYER).unwrap();
            let mut deploy = deploy_by(deployer, id);
            deploy.data.term = term.clone();
            storage.add_deploy(deploy).await.unwrap();
        }
        assert_eq!(storage.pooled_term_bytes(), MAX_POOLED_DEPLOY_BYTES);

        let mut extra = deploy_by(9, maximal_deploys);
        extra.data.term = "x".to_string();
        let err = storage
            .add_deploy(extra.clone())
            .await
            .expect_err("one byte over the budget must be refused");
        assert!(err.contains("term bytes in use"), "{err}");
        assert!(
            !storage.contains_deploy_in_pool(&extra.sig).await.unwrap(),
            "a refused deploy must not reach the store"
        );
    }

    /// **An expired deploy returns its term bytes** (B36): a budget that only charges is a pool
    /// that fills and never drains.
    #[tokio::test]
    async fn an_expired_deploy_returns_its_term_bytes() {
        let storage = build_storage().await;
        let mut deploy = deploy_by(0, 1);
        deploy.data.term = "x".repeat(1_024);
        deploy.data.valid_after_block_number = 1;
        let id = deploy.sig.clone();
        storage.add_deploy(deploy).await.unwrap();
        assert_eq!(storage.pooled_term_bytes(), 1_024);

        storage.expire_deploys(51).await.unwrap();
        assert!(!storage.contains_deploy_in_pool(&id).await.unwrap());
        assert_eq!(storage.pooled_term_bytes(), 0);
    }

    /// **A duplicate is refused at the storage's own chokepoint too** — `add_deploy` is a direct
    /// ingress, and an overwrite would charge the share and the bytes twice while the pool stored one
    /// row. The refused copy must move neither.
    #[tokio::test]
    async fn adding_the_same_deploy_twice_is_refused_and_charged_once() {
        let storage = build_storage().await;
        let deploy = deploy_by(0, 1);
        storage.add_deploy(deploy.clone()).await.unwrap();
        let err = storage
            .add_deploy(deploy.clone())
            .await
            .expect_err("a byte-identical deploy is already in the pool");
        assert!(err.contains("already in the pool"), "{err}");
        assert_eq!(storage.pooled_deploys().await.unwrap().len(), 1);
        assert_eq!(storage.pooled_term_bytes(), deploy.data.term.len());
    }

    fn deploy_by(deployer: u8, id: usize) -> SignedDeployData {
        SignedDeployData {
            deployer: vec![deployer],
            ..deploy_with_id(id)
        }
    }

    /// **One deployer cannot spend another deployer's allowance** (W-N2-01).
    ///
    /// The admission bound was one token bucket for the whole node, so its subject was the node and
    /// not the caller: one identity's burst exhausted the allowance and refused every other
    /// identity's deploys. The pool was bounded, correctly, and the wrong counterparty paid. Exhaust
    /// one deployer completely; a second must be unaffected, and the refusal must name the *rate*
    /// rather than the pool so the two bounds cannot be confused.
    #[tokio::test]
    async fn one_deployer_cannot_spend_another_deployers_allowance() {
        let storage = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deploy_rate(3, MAX_TRACKED_DEPLOYERS);

        for id in 0..3 {
            storage.add_deploy(deploy_by(1, id)).await.unwrap();
        }
        let err = storage.add_deploy(deploy_by(1, 999)).await;
        assert!(
            err.as_ref().is_err_and(|e| e.contains("rate limit")),
            "the fourth deploy from one key must be refused by its own rate bound, got {err:?}"
        );

        // A deploy the first key has not submitted: the id (the signature) is the pool's identity,
        // so reusing id 0 here would be the duplicate reservation answering, not this bound.
        assert!(
            storage.add_deploy(deploy_by(2, 500)).await.is_ok(),
            "a second deployer's allowance is its own; the first deployer's flood must not refuse it"
        );
    }

    // --- Deployer index (`GET /api/v1/deployer/{pubkey}`) ---

    fn deployer_index_store() -> Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>> {
        Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(BytesCodec),
            Arc::new(BytesCodec),
        ))
    }

    /// `meta` with the sequence number following the height, so a chain of blocks by the one test
    /// sender is not refused as an equivocation.
    fn meta_seq(hash: BlockHash, parents: &[BlockHash], block_num: i64) -> BlockMetadata {
        BlockMetadata {
            seq_num: block_num.try_into().unwrap(),
            ..meta(hash, parents, block_num)
        }
    }

    /// A block carrying one deploy per key in `deployers`.
    fn block_signed_by(h: BlockHash, deployers: &[u8]) -> BlockMessage {
        let mut b = block(h);
        b.state.deploys = deployers
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let mut d = deploy_with_id(1000 * usize::from(h.as_bytes()[0]) + i);
                d.deployer = vec![*k; 65];
                rchain_models::casper::protocol::casper_message::ProcessedDeploy {
                    deploy: d,
                    cost: rchain_models::casper::protocol::casper_message::PCost { cost: 0 },
                    deploy_log: vec![],
                    is_failed: false,
                    system_deploy_error: None,
                }
            })
            .collect();
        b
    }

    #[tokio::test]
    async fn deployer_index_records_the_first_block_a_key_signed_in() {
        let storage = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deployer_index(deployer_index_store())
            .await
            .unwrap();
        // An empty chain: the index is complete from genesis.
        let unseen = storage
            .lookup_by_deployer(&deployer_index_key(&[7u8; 65]))
            .await
            .unwrap();
        assert_eq!(
            unseen,
            DeployerLookup {
                block: None,
                indexed_from: 0
            }
        );

        let (h1, h2) = (hash(1), hash(2));
        storage
            .insert(meta_seq(h1, &[], 0), block_signed_by(h1, &[7, 8]))
            .await
            .unwrap();
        storage
            .insert(meta_seq(h2, &[h1], 1), block_signed_by(h2, &[7]))
            .await
            .unwrap();

        assert_eq!(
            storage
                .lookup_by_deployer(&deployer_index_key(&[7u8; 65]))
                .await
                .unwrap()
                .block,
            Some(h1),
            "the first block a key signed in is kept, not overwritten by a later one"
        );
        assert_eq!(
            storage
                .lookup_by_deployer(&deployer_index_key(&[8u8; 65]))
                .await
                .unwrap()
                .block,
            Some(h1)
        );
        assert_eq!(
            storage
                .lookup_by_deployer(&deployer_index_key(&[9u8; 65]))
                .await
                .unwrap()
                .block,
            None,
            "a key that never signed is not in the index"
        );
    }

    #[tokio::test]
    async fn deployer_index_is_refused_when_not_attached() {
        let storage = build_storage().await;
        assert!(
            storage
                .lookup_by_deployer(&deployer_index_key(&[7u8; 65]))
                .await
                .is_err(),
            "a storage without the index must not answer \"not seen\""
        );
    }

    /// The upgrade path: a node whose chain predates the index reports how far down it reaches, and
    /// the backfill reads the stored blocks below that and then reports the index complete.
    #[tokio::test]
    async fn deployer_index_backfills_a_chain_that_predates_it() {
        let metadata_store = empty_metadata_store().await;
        let block_store: Arc<dyn KeyValueTypedStore<BlockHash, BlockMessage>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BlockHashCodec),
                Arc::new(rchain_block_storage::dag::codecs::BlockMessageCodec),
            ));
        let (h1, h2, h3) = (hash(1), hash(2), hash(3));
        {
            // The old node: no index. `h3` failed validation, so the height map leaves it out —
            // but `insert` indexes a failed block's deployers, so the backfill must too.
            let old = build_storage_over(metadata_store.clone()).await;
            for (h, parents, height, keys, failed) in [
                (h1, vec![], 0, vec![7u8], false),
                (h2, vec![h1], 1, vec![8u8], false),
                (h3, vec![h2], 2, vec![9u8], true),
            ] {
                let b = block_signed_by(h, &keys);
                block_store.put(&[(h, b.clone())]).await.unwrap();
                let m = BlockMetadata {
                    validation_failed: failed,
                    ..meta_seq(h, &parents, height)
                };
                old.insert(m, b).await.unwrap();
            }
        }

        let index = deployer_index_store();
        let upgraded = build_storage_over_unshared(metadata_store)
            .await
            .with_deployer_index(index.clone())
            .await
            .unwrap();
        let before = upgraded
            .lookup_by_deployer(&deployer_index_key(&[7u8; 65]))
            .await
            .unwrap();
        assert_eq!(
            before,
            DeployerLookup {
                block: None,
                indexed_from: 2
            },
            "before the backfill the answer says the heights below 2 are unread"
        );

        assert_eq!(
            upgraded
                .backfill_deployer_index(&block_store)
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            upgraded
                .lookup_by_deployer(&deployer_index_key(&[7u8; 65]))
                .await
                .unwrap(),
            DeployerLookup {
                block: Some(h1),
                indexed_from: 0
            }
        );
        assert_eq!(
            upgraded
                .lookup_by_deployer(&deployer_index_key(&[8u8; 65]))
                .await
                .unwrap()
                .block,
            Some(h2)
        );
        assert_eq!(
            upgraded
                .lookup_by_deployer(&deployer_index_key(&[9u8; 65]))
                .await
                .unwrap()
                .block,
            Some(h3),
            "a validation-failed block's deployer is backfilled, as a live insert would index it"
        );

        // Persisted: a restart over the same index store is complete without another backfill.
        let restarted = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deployer_index(index)
            .await
            .unwrap();
        assert_eq!(
            restarted
                .lookup_by_deployer(&deployer_index_key(&[7u8; 65]))
                .await
                .unwrap(),
            DeployerLookup {
                block: Some(h1),
                indexed_from: 0
            }
        );
        assert_eq!(
            restarted
                .backfill_deployer_index(&block_store)
                .await
                .unwrap(),
            0
        );
    }

    /// A metadata store whose writes fail: what `insert` leaves behind when it is interrupted at
    /// the point that makes a block known.
    struct FailingPutStore(InMemoryKeyValueStore);

    impl KeyValueStore for FailingPutStore {
        fn get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>, String> {
            self.0.get(keys)
        }
        fn put(&mut self, _pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<(), String> {
            Err("interrupted".to_string())
        }
        fn delete(&mut self, keys: &[Vec<u8>]) -> Result<usize, String> {
            self.0.delete(keys)
        }
        fn entries(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
            self.0.entries()
        }
    }

    /// Review of #277: the index is written before the block becomes known, so an insert interrupted
    /// there leaves no known block with a missing signer, and the retry after a restart completes it.
    #[tokio::test]
    async fn deployer_index_survives_an_insert_interrupted_before_the_block_is_known() {
        let failing: Shared = Arc::new(tokio::sync::Mutex::new(Box::new(FailingPutStore(
            InMemoryKeyValueStore::default(),
        ))));
        let failing_metadata = Arc::new(
            BlockMetadataStore::create(Arc::new(KeyValueTypedStoreCodec::new(
                failing,
                Arc::new(BlockHashCodec),
                Arc::new(BlockMetadataCodec),
            )))
            .await
            .unwrap(),
        );
        let index = deployer_index_store();
        let h1 = hash(1);
        let interrupted = build_storage_over_unshared(failing_metadata)
            .await
            .with_deployer_index(index.clone())
            .await
            .unwrap();
        assert!(interrupted
            .insert(meta_seq(h1, &[], 0), block_signed_by(h1, &[7]))
            .await
            .is_err());
        assert_eq!(
            interrupted
                .lookup_by_deployer(&deployer_index_key(&[7u8; 65]))
                .await
                .unwrap()
                .block,
            Some(h1),
            "the signer was indexed before the write that makes the block known"
        );

        // Restart over a healthy store: the block is not known, so its re-insert runs in full.
        let restarted = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deployer_index(index)
            .await
            .unwrap();
        restarted
            .insert(meta_seq(h1, &[], 0), block_signed_by(h1, &[7]))
            .await
            .unwrap();
        assert_eq!(
            restarted
                .lookup_by_deployer(&deployer_index_key(&[7u8; 65]))
                .await
                .unwrap(),
            DeployerLookup {
                block: Some(h1),
                indexed_from: 0
            }
        );
    }

    /// A node that joined by syncing from a fringe holds no blocks below it, so its coverage starts
    /// at its lowest block — never 0 — however complete its index is for the blocks it has.
    #[tokio::test]
    async fn deployer_index_coverage_starts_at_the_lowest_block_held() {
        let storage = build_storage_over_unshared(empty_metadata_store().await)
            .await
            .with_deployer_index(deployer_index_store())
            .await
            .unwrap();
        let h = hash(5);
        storage
            .insert(meta_seq(h, &[], 40), block_signed_by(h, &[7]))
            .await
            .unwrap();
        assert_eq!(
            storage
                .lookup_by_deployer(&deployer_index_key(&[9u8; 65]))
                .await
                .unwrap(),
            DeployerLookup {
                block: None,
                indexed_from: 40
            }
        );
    }

    fn chain_hash(i: usize) -> BlockHash {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
        BlockHash::new(bytes)
    }

    /// Store a chain of `n` blocks: block `i` sits at height `i` and justifies block `i - 1`, so
    /// its `seen` set is its whole ancestry — which is what a restore has to fold.
    async fn store_chain(store: &BlockMetadataStore, n: usize) {
        let mut hashes: Vec<BlockHash> = Vec::with_capacity(n);
        for i in 0..n {
            let h = chain_hash(i);
            let parents: Vec<BlockHash> = if i == 0 {
                Vec::new()
            } else {
                vec![hashes[i - 1]]
            };
            // One sender, a strictly increasing `seq_num` — the shape H-1 and `sequence_number`
            // require of a chain a validator actually produced. The plain `meta` helper reuses
            // `seq_num 0` for every block, which is a *fork* by those rules: legal to store (the
            // persisted layer checks only that the height map is contiguous) and exactly what
            // `create`'s H-1 re-check refuses on restore — which is the point of that re-check, so
            // this fixture had to become a state the port admits.
            store
                .add(meta_by(0, i as i64, h, &parents, i as i64))
                .await
                .unwrap();
            hashes.push(h);
        }
    }

    /// AUDIT C56's owed paragraph, first half — **reading** the DAG must not copy it.
    ///
    /// `get_representation` returned `DagRepresentation` **by value**, so every caller paid a deep
    /// copy of `msg_map`: N messages, each carrying its whole ancestor set behind `seen`, i.e.
    /// Σ|seen| × 32 B ≈ 23 MB *per read* at N=1,200 and ≈ 550 MB at the 5,844-block devnet chain —
    /// with the read lock held for the whole copy. Roughly 35 production sites read it, including
    /// every per-block path and every peer-request handler, which is what grew a peer-serving node
    /// by ~0.86 GiB per block. A reader now takes a refcount (`Arc<DagRepresentation>`).
    ///
    /// A cost bound, not a semantic one: the value is identical either way, which is why no test of
    /// behaviour could see it.
    ///
    /// **Two instruments, because one did not survive its own falsification (2026-09-24).** The
    /// timing bound alone was calibrated against the *pre-Stage-3* tree — the doc-comment figure of
    /// 27.3 s is that artifact, where `Message.seen` was still a plain `BTreeSet`. With `seen` shared
    /// the same restoration costs **459 ms**, which a 5 s bound clears, so the tripwire passed with
    /// the copy put back. It now asserts the *mechanism* as well: every read must hand back the same
    /// allocation (`Arc::ptr_eq`), which is exactly "the DAG was not rebuilt" and is deterministic —
    /// immune to machine load, and 0.17 ms measured against 459 ms, where a timing bound separating
    /// them by 2,700× would have to sit close enough to the floor to be flaky. The 5 s bound stays
    /// for the slower regression: the *whole* pre-pass shape (copying and unshared `seen`) is the
    /// 27.3 s case, which both instruments catch. `--nocapture` prints the window.
    #[tokio::test]
    async fn reading_the_dag_representation_does_not_copy_the_message_state() {
        const N: usize = 1200;
        const READS: usize = 200;
        const BOUND: std::time::Duration = std::time::Duration::from_secs(5);

        let metadata_store = empty_metadata_store().await;
        store_chain(&metadata_store, N).await;
        let storage = build_storage_over(metadata_store).await;

        // No insert runs here, so the stored `Arc` cannot be replaced under us: every read of an
        // unchanged DAG must return that same allocation.
        let first = storage.get_representation().await;
        let started = std::time::Instant::now();
        for _ in 0..READS {
            let repr = storage.get_representation().await;
            // The read did happen, and returned the whole DAG — so the tripwire cannot pass by
            // reading something smaller.
            assert_eq!(repr.dag_message_state.msg_map.len(), N);
            assert!(
                Arc::ptr_eq(&first, &repr),
                "a read of a {N}-block DAG returned a different allocation than the previous one: \
                 `get_representation` is rebuilding the representation instead of sharing it"
            );
        }
        let elapsed = started.elapsed();
        println!("{READS} representation reads at N={N}: {elapsed:?}");

        assert!(
            elapsed < BOUND,
            "{READS} representation reads of a {N}-block DAG took {elapsed:?} (bound {BOUND:?}): the \
             DAG is being copied per read instead of shared"
        );
    }

    /// AUDIT C56's owed paragraph, second half — **adding** a block must not copy the DAG either.
    ///
    /// `insert` cloned `dag_message_state` once per block (again Θ(N) messages, though no longer
    /// Θ(N) each) purely to avoid holding the read guard across the awaits further down; everything
    /// it needed from the state is synchronous, so the copy bought nothing. The guard is now scoped
    /// to the synchronous region and the write path takes `Arc::make_mut`, which copies only while a
    /// reader holds a snapshot — at most once per insert, serialized by the storage's own lock.
    ///
    /// **What this test can and cannot see (measured 2026-09-24).** The transient copy — HEAD's
    /// `self.representation.read().await.dag_message_state.clone()`, restored verbatim — costs
    /// **2.2 ms per insert** at N=1,200 against the insert's own 4.6 ms: 50 inserts take **339 ms
    /// copying against 230 ms fixed**. A 1.47× ratio cannot carry a timing bound, and the 3 s bound
    /// it used to carry did not catch it. The ratio is small *because* Stage 3 landed: sharing
    /// `seen` turned the copy into N refcount bumps, so the doc-comment figure of 7.44 s describes
    /// the pre-Stage-3 tree rather than this falsifier. And with no allocation counter available (a
    /// `#[global_allocator]` needs `unsafe`, which this crate graph does not admit) that transient
    /// copy leaves no trace a test can read — only its cost, which nothing here measures.
    ///
    /// So the assertion is the mechanism's *persistent* half, which is deterministic: with no reader
    /// holding a snapshot, `insert` must take `Arc::make_mut`'s no-copy path, leaving the
    /// representation at the same allocation. An unconditional `Arc::new((**guard).clone())` — the
    /// plausible way to reintroduce the per-block copy — fails here. The 5 s bound remains as a
    /// gross-regression smoke bound (the full pre-pass shape lands in the seconds, and C55's own
    /// tripwire covers the cubic one); it is deliberately not tight enough to be the copy detector,
    /// and is not claimed to be. The per-*read* half is pinned deterministically by
    /// `reading_the_dag_representation_does_not_copy_the_message_state`.
    #[tokio::test]
    async fn adding_a_block_does_not_copy_the_message_state() {
        const N: usize = 1200;
        const K: usize = 50;
        const BOUND: std::time::Duration = std::time::Duration::from_secs(5);

        let metadata_store = empty_metadata_store().await;
        store_chain(&metadata_store, N).await;
        let storage = build_storage_over(metadata_store).await;

        // `as_ptr` off a temporary, so the read does not itself hold a snapshot across the inserts
        // (which would force `make_mut` to copy, legitimately).
        let before = Arc::as_ptr(&storage.get_representation().await);

        // A distinct `seq_num` per block, continuing the stored chain's sequence: `store_chain` now
        // gives its blocks sender `[0; 65]` with seq `0..N`, and a second block from the same sender
        // at the same seq is an equivocation (H-1) — refused at ingress and on restore alike.
        let mut tip = chain_hash(N - 1);
        let started = std::time::Instant::now();
        for k in 0..K {
            let next = chain_hash(N + k);
            let mut m = meta(next, &[tip], (N + k) as i64);
            m.seq_num = ((N + k) as i64).try_into().unwrap();
            storage.insert(m, block(next)).await.unwrap();
            tip = next;
        }
        let elapsed = started.elapsed();
        println!("{K} inserts at N={N}: {elapsed:?}");

        let repr = storage.get_representation().await;
        assert_eq!(repr.dag_message_state.msg_map.len(), N + K);
        assert_eq!(
            before,
            Arc::as_ptr(&repr),
            "inserting into an unread DAG replaced the representation: the message state is being \
             copied per block instead of extended in place"
        );

        assert!(
            elapsed < BOUND,
            "{K} inserts into a {N}-block DAG took {elapsed:?} (bound {BOUND:?})"
        );
    }

    /// A metrics sink that records what the DAG publishes, so the gauges can be asserted rather than
    /// eyeballed on a live `/metrics`.
    #[derive(Default)]
    struct RecordingMetrics {
        gauges: std::sync::Mutex<BTreeMap<(String, String), i64>>,
    }

    impl RecordingMetrics {
        fn gauge(&self, source: &str, name: &str) -> Option<i64> {
            self.gauges
                .lock()
                .unwrap()
                .get(&(source.to_string(), name.to_string()))
                .copied()
        }
    }

    impl rchain_shared::metrics::Metrics for RecordingMetrics {
        fn increment_counter(&self, _s: &rchain_shared::metrics::Source, _n: &str, _d: i64) {}
        fn increment_sampler(&self, _s: &rchain_shared::metrics::Source, _n: &str, _d: i64) {}
        fn sample(&self, _s: &rchain_shared::metrics::Source, _n: &str) {}
        fn set_gauge(&self, source: &rchain_shared::metrics::Source, name: &str, value: i64) {
            self.gauges
                .lock()
                .unwrap()
                .insert((source.0.clone(), name.to_string()), value);
        }
        fn increment_gauge(&self, _s: &rchain_shared::metrics::Source, _n: &str, _d: i64) {}
        fn decrement_gauge(&self, _s: &rchain_shared::metrics::Source, _n: &str, _d: i64) {}
        fn record(&self, _s: &rchain_shared::metrics::Source, _n: &str, _v: i64, _c: i64) {}
    }

    /// AUDIT C56's owed paragraph, third part — the DAG index is held **once**: the representation's
    /// `dag_set`/`child_map`/`height_map` are the store's own allocations.
    ///
    /// `BlockMetadataStore`'s accessors used to hand out a full clone per call, and `insert` called
    /// all three on every block, so the index lived twice and was rebuilt per insert. `Arc`-sharing
    /// it is a *value-identical* change (the digest test above pins that), so the assertion is the
    /// mechanism: pointer identity between the store's index and the representation's, which is
    /// exactly "one index, one update point".
    ///
    /// Falsified against this tree (2026-09-24): returning a fresh `Arc::new((*map).clone())` from the
    /// accessors — the plausible way to reintroduce the second copy while keeping the type — fails
    /// the first `ptr_eq`. The old shape (a plain `BTreeSet` field) cannot even be compared this way,
    /// which is why the mechanism, not the bound, is the tripwire.
    #[tokio::test]
    async fn the_index_is_shared_with_the_representation_not_copied() {
        let metadata_store = empty_metadata_store().await;
        store_chain(&metadata_store, 8).await;
        let storage = build_storage_over(metadata_store.clone()).await;
        let repr = storage.get_representation().await;

        assert!(
            Arc::ptr_eq(&metadata_store.dag_set().await, &repr.dag_set),
            "the representation's dag_set is the store's allocation, not a copy of it"
        );
        assert!(
            Arc::ptr_eq(&metadata_store.child_map_data().await, &repr.child_map),
            "the representation's child_map is the store's allocation"
        );
        assert!(
            Arc::ptr_eq(&metadata_store.height_map().await, &repr.height_map),
            "the representation's height_map is the store's allocation"
        );
        // …and it is the same *index*, not merely a shared empty one.
        assert_eq!(repr.dag_set.len(), 8);
        assert_eq!(repr.child_map.len(), 8);
        assert_eq!(repr.height_map.len(), 8);

        // An insert keeps them shared: the store's new index is the representation's, while the
        // snapshot taken above keeps the old one (copy-on-write, which is what makes the read path
        // cheap and the writer's copy at most one per insert).
        let mut m = meta(chain_hash(8), &[chain_hash(7)], 8);
        m.seq_num = 8.try_into().unwrap();
        storage.insert(m, block(chain_hash(8))).await.unwrap();
        let after = storage.get_representation().await;
        assert!(
            Arc::ptr_eq(&metadata_store.dag_set().await, &after.dag_set),
            "an insert moves the pointer, it does not replace the index"
        );
        assert_eq!(after.dag_set.len(), 9);
        assert_eq!(
            repr.dag_set.len(),
            8,
            "the snapshot taken before the insert keeps the index it read"
        );
    }

    /// Stage 6's observability claim — the DAG publishes its own gauges, and `logical_bytes` is the
    /// number the residency claims are stated in.
    ///
    /// The assertion is the published gauge *equals* the representation's own accounting (so the
    /// instrument cannot report a number the structure does not have) and that it moves when the DAG
    /// does (so a frozen gauge is not mistaken for a flat cost). Falsified against this tree: with the
    /// `set_gauges` call removed — the shape before this stage, where nothing published anything and
    /// `/metrics` served the reporter's placeholder — every `gauge(..)` below is `None`.
    #[tokio::test]
    async fn the_dag_publishes_its_own_gauges() {
        let metrics = Arc::new(RecordingMetrics::default());
        let metadata_store = empty_metadata_store().await;
        store_chain(&metadata_store, 4).await;
        let storage = Arc::new(
            build_storage_over_unshared(metadata_store)
                .await
                .with_metrics(metrics.clone()),
        );

        // Attaching the sink published what `create` rebuilt — a restart reports its whole chain
        // before it accepts its first block.
        assert_eq!(metrics.gauge("rchain.dag", "messages"), Some(4));

        // `seq` continues the stored chain's sender sequence (see `store_chain`), and `i` the height.
        for (seq, i) in [(4i64, 4usize), (5, 5)] {
            let mut m = meta(chain_hash(i), &[chain_hash(i - 1)], i as i64);
            m.seq_num = seq.try_into().unwrap();
            storage.insert(m, block(chain_hash(i))).await.unwrap();
        }

        let repr = storage.get_representation().await;
        let expected = i64::try_from(repr.logical_bytes()).expect("fits i64");
        assert_eq!(metrics.gauge("rchain.dag", "messages"), Some(6));
        assert_eq!(
            metrics.gauge("rchain.dag", "seen_entries"),
            Some(i64::try_from(repr.seen_entries()).expect("fits i64")),
            "the published seen-entry count is the representation's own"
        );
        assert_eq!(
            metrics.gauge("rchain.dag", "logical_bytes"),
            Some(expected),
            "the published accounting is the representation's own"
        );
        assert_eq!(
            metrics.gauge("rchain.dag", "index_entries"),
            Some(i64::try_from(repr.index_entries()).expect("fits i64"))
        );
        // **Four gauges, not five** (Law 66/68; C250's residue, C270): the `fringe_states` gauge and
        // the map it sized retired with the `fringe-data` store. There is no set of records left to
        // count, so the published set is exactly these four.
        assert_eq!(metrics.gauge("rchain.dag", "fringe_states"), None);
        assert!(
            expected > 0 && repr.seen_entries() > 0,
            "a non-empty DAG has a non-zero account"
        );
        // The negative control for a representation-only change, in exact units: this chain's account
        // is a fixed number. The retired `fringe_states` map, the `Arc`-ing of the index and this
        // stage's own gauges must not move it — the accounting reads the *messages*, so a change
        // that moved this value would be a change to the DAG rather than to how it is held.
        assert_eq!(
            repr.logical_bytes(),
            2032,
            "the chain's logical bytes are a fixed value, not a function of how the DAG is held"
        );
        assert_eq!(
            repr.seen_entries(),
            21,
            "and so is its seen-entry count (Σ|seen| = 21 for this chain)"
        );

        // It moves: one more block, and `logical_bytes` and `seen_entries` grow (a chain's `seen` is
        // its whole ancestry). `seq 6` continues the same sender's sequence (stored `0..4`, inserted
        // `4`, `5`), rather than reusing a number the chain already spent.
        let mut m = meta(chain_hash(6), &[chain_hash(5)], 6);
        m.seq_num = 6.try_into().unwrap();
        storage.insert(m, block(chain_hash(6))).await.unwrap();
        assert!(
            metrics
                .gauge("rchain.dag", "logical_bytes")
                .expect("published")
                > expected,
            "the account grows with the DAG"
        );
        assert_eq!(metrics.gauge("rchain.dag", "messages"), Some(7));
    }

    /// A canonical digest of the representation's **value**: every message and the three index-map
    /// sizes, independent of how the maps are keyed or whether they are shared.
    ///
    /// **The fringe records are not digested any more** (Law 66/68; C250's residue, C270): the map
    /// they came from retired with the `fringe-data` store, and with it the only part of this digest
    /// that had to be *sorted* before hashing. The digest is therefore simpler and pins less — it is
    /// the messages and the index, exactly what the representation still is — and it remains
    /// falsifiable in both directions: it *moves* when the value moves (below), so a constant digest
    /// is evidence rather than a tautology.
    fn representation_digest(repr: &Arc<DagRepresentation>) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (id, m) in &repr.dag_message_state.msg_map {
            parts.push(format!(
                "m {} h{} s{} p{} f{} seen{}",
                id.to_hex(),
                i64::from(m.height),
                i64::from(m.sender_seq),
                m.parents.len(),
                m.fringe.len(),
                m.seen.len()
            ));
        }
        parts.push(format!(
            "index {} {} {}",
            repr.dag_set.len(),
            repr.child_map.len(),
            repr.height_map.len()
        ));
        let refs: Vec<&[u8]> = parts.iter().map(|p| p.as_bytes()).collect();
        Blake2b256Hash::create_many(&refs).to_hex()
    }

    /// The digest of a stored chain is a fixed value, and it moves when the DAG does — the negative
    /// control for a representation-only change (AUDIT C56's owed paragraph).
    ///
    /// `the_representation_digest` is computed from a two-block chain; the assert on the second half
    /// is what makes the first half evidence: inserting a block changes the digest, so a digest that
    /// did not move between two trees would mean the DAG's value really is unchanged.
    #[tokio::test]
    async fn the_representation_digest_pins_the_value_and_moves_with_it() {
        let metadata_store = empty_metadata_store().await;
        store_chain(&metadata_store, 2).await;
        let storage = build_storage_over(metadata_store).await;
        // Continuing the stored chain's one sender: `store_chain` gives each of its blocks a strictly
        // increasing `seq_num`, so the continuation continues *that* sequence and takes the next
        // height (`seq`, i.e. parent + 1), rather than restarting at `seq 1` on top of a fixture that
        // used to reuse `seq 0` throughout.
        for (seq, h) in [(2i64, chain_hash(2)), (3, chain_hash(3))] {
            let mut m = meta(h, &[chain_hash(usize::try_from(seq - 1).unwrap())], seq);
            m.seq_num = seq.try_into().unwrap();
            storage.insert(m, block(h)).await.unwrap();
        }

        let before = representation_digest(&storage.get_representation().await);
        assert_eq!(
            before, "555361e149afda515e025bb54e3d7732b236c687de82521b241002bcc26b8459",
            "the representation's value for a 4-block chain — recomputed when the fringe records \
             left the digest as the `fringe-data` store retired (Law 66/68; C250's residue, C270)"
        );

        let mut m = meta(chain_hash(4), &[chain_hash(3)], 4);
        m.seq_num = 4.try_into().unwrap();
        storage.insert(m, block(chain_hash(4))).await.unwrap();
        let after = representation_digest(&storage.get_representation().await);
        assert_ne!(before, after, "the digest moves when the DAG does");
    }

    /// **The fringe claim travels with the block, and the mark names the fringe by its own key.**
    ///
    /// This test replaces `fringe_states_are_keyed_by_the_stores_own_key`. That one asserted the
    /// in-memory `fringe_states` map was keyed by the fringe store's own key
    /// (`FringeData::fringe_hash_of(fringe)`), so the map was a faithful cache of the store rather
    /// than a second index with a different identity. **Both the map and the store retired** (Law
    /// 66/68; C250's residue, C270), so that assertion is unstatable — there is no map to key — and
    /// it is *re-expressed* around what is still checkable and still load-bearing: the single fact
    /// both the old map and the readers now agree on, which is that a fringe is *named* by
    /// `fringe_hash_of(its sorted set)`, and that a block which belongs to one records that name.
    ///
    /// The old test's key claim survives in this form: `member_of_fringe` is `fringe_hash_of(fringe)`
    /// — a hash over the sorted set (Law 18: fringe identity is order-independent), not the set
    /// itself — and it is written on the blocks that were *finalised by* the declaring block, which
    /// is exactly the index the new readers (`multi_parent_casper.rs::get_pre_state_for_parents`,
    /// `merging.rs`'s rejection derivation) walk. The claim half is now per-block metadata: a block's
    /// own `fringe` and `fringe_state_hash`, written once under the block's hash.
    #[tokio::test]
    async fn the_fringe_claim_travels_with_the_block_it_marks() {
        let storage = build_storage().await;
        let (genesis, left, right, tip) = (hash(0), hash(1), hash(2), hash(3));

        // Two children of genesis, then a block that justifies only one of them but declares both as
        // its fringe — so the fringe diff is non-empty and the *finalised* blocks are marked with
        // their member fringe (`insert`'s `member_of_fringe`), which is what this test reads.
        for (seq, h, parents) in [
            (0i64, genesis, vec![]),
            (1, left, vec![genesis]),
            (2, right, vec![genesis]),
        ] {
            let mut m = meta(h, &parents, seq);
            m.seq_num = seq.try_into().unwrap();
            storage.insert(m, block(h)).await.unwrap();
        }
        let fringe: BTreeSet<BlockHash> = [left, right].into_iter().collect();
        let mut m = meta(tip, &[left], 3);
        m.seq_num = 3.try_into().unwrap();
        m.fringe = fringe.clone();
        // A distinct state, so the claim assertion below is about the value the block carried rather
        // than about a fixture default.
        m.fringe_state_hash = StateHash::new([0x7au8; 32]);
        storage.insert(m, block(tip)).await.unwrap();

        // **The name.** A fringe's key is the hash of its *sorted set* — the mark below is that value,
        // and building the set in either order gives it, which is Law 18 on the type that replaces
        // the retired `FringeData::fringe_hash_of`.
        let key = rchain_models::block_metadata::fringe_hash_of(&fringe);
        let reversed_key = rchain_models::block_metadata::fringe_hash_of(
            &[right, left].into_iter().collect::<BTreeSet<_>>(),
        );
        assert_eq!(key, reversed_key, "fringe identity is order-independent");

        // **The mark.** Both finalised blocks record that key as their member fringe — the index is
        // the fringe's own hash, exactly as the retired map's key was (the re-expressed half of the
        // old test's claim).
        for h in [left, right] {
            assert_eq!(
                storage.lookup(&h).await.unwrap().unwrap().member_of_fringe,
                Some(key),
                "a block finalised by the fringe is marked with the fringe's own key"
            );
        }

        // **The claim.** It is the declaring block's *own* metadata — its `fringe` and the state it
        // derived for that fringe — written once under its hash. The old test read this same value
        // out of the store's record; a reader names the block now (Law 68's version), so the block's
        // metadata is where the value lives. There is no second place (no map) that could disagree.
        let tip_meta = storage.lookup(&tip).await.unwrap().unwrap();
        assert_eq!(
            tip_meta.fringe, fringe,
            "the block declares the fringe it finalised"
        );
        assert_eq!(
            tip_meta.fringe_state_hash,
            StateHash::new([0x7au8; 32]),
            "and carries the state it derived for it — the claim, on the block"
        );
    }

    /// AUDIT C55 — restoring a stored chain must not rebuild the message state once per block.
    ///
    /// `BlockDagKeyValueStorage::create` folds every stored block through the message state. The
    /// oracle folds through a *persistent* map — `DagMessageState.scala`'s `msgMap + (k -> v)` over
    /// Scala's immutable `Map`, structurally shared, O(log N). A `BTreeMap` has no structural
    /// sharing, so the same-shaped `insert_msg` deep-copies the map per block, and every entry's
    /// own `seen` set with it, making the restore Θ(N³) in copies and Θ(N²) in memory. On
    /// 2026-09-24 a 5,844-block devnet bootstrap spent longer inside this loop than
    /// `tools/devnet.sh` waits for `/api/v1/status`, at ~100% of one core and with no further log
    /// line — which read as a hang and was reported as one.
    ///
    /// The bound is a cost bound, not a semantic one: the rebuild produces the same state either
    /// way, which is exactly why nothing caught it. Measured on the authoring machine at N=1500 in
    /// a debug test build: 5.1 s in place, 108.6 s copying — a 21× gap, widening with N (the one is
    /// Θ(N²), the other Θ(N³)). The constants below sit at N=1200, where that gap is 3.3 s against
    /// 55.6 s: the bound leaves the fixed fold 4.6× of headroom (so a CI runner several times
    /// slower does not trip it) and the copying fold 3.7× over it. Recalibrate from those two
    /// numbers if either side ever changes.
    #[tokio::test]
    async fn restoring_a_stored_chain_is_not_cubic_in_the_message_state() {
        const N: usize = 1200;
        const BOUND: std::time::Duration = std::time::Duration::from_secs(15);

        let metadata_store = empty_metadata_store().await;
        store_chain(&metadata_store, N).await;

        let started = std::time::Instant::now();
        let storage = build_storage_over(metadata_store).await;
        let elapsed = started.elapsed();

        // The fold did happen — so the tripwire cannot pass by never doing the work.
        let repr = storage.get_representation().await;
        assert_eq!(repr.dag_message_state.msg_map.len(), N);

        assert!(
            elapsed < BOUND,
            "restoring a {N}-block chain took {elapsed:?} (bound {BOUND:?}): the message state is \
             being rebuilt per block instead of extended in place"
        );
    }

    /// `meta`, with the sender and sequence number chosen by the caller: H1b needs a failed block by
    /// one validator and a child by another.
    fn meta_by(
        sender_byte: u8,
        seq: i64,
        hash: BlockHash,
        parents: &[BlockHash],
        block_num: i64,
    ) -> BlockMetadata {
        BlockMetadata {
            sender: Validator::new([sender_byte; 65]),
            seq_num: SeqNum::try_from(seq).unwrap(),
            ..meta(hash, parents, block_num)
        }
    }

    /// `block`, with the fields a validator reads.
    fn block_with(
        hash: BlockHash,
        block_number: i64,
        sender_byte: u8,
        seq: i64,
        justifications: &[BlockHash],
        bonds: &[(u8, i64)],
    ) -> BlockMessage {
        BlockMessage {
            block_number: BlockHeight::try_from(block_number).unwrap(),
            sender: Validator::new([sender_byte; 65]),
            seq_num: SeqNum::try_from(seq).unwrap(),
            justifications: justifications.to_vec(),
            bonds: bonds
                .iter()
                .map(|(b, s)| (Validator::new([*b; 65]), NonNegI64::try_from(*s).unwrap()))
                .collect(),
            ..block(hash)
        }
    }

    /// **H1b, enforced — and it was the reproduction before.** The model requires every resolved
    /// parent to be lower than the message naming it (`Descends`, `spec/Rchain/Casper/Dag.lean:Descends`).
    /// `validate::block_number` now *requires* that of every resolved parent while still skipping
    /// failed ones for the maximum; before that enforcement this test asserted the opposite — that the
    /// violation was **admitted** — because a failed block's recorded height is its **claimed**
    /// `block_num` (`message_from_block_metadata`: `height: block.block_num`) and nothing bounded it.
    /// The child below used to be accepted and inserted, putting a parent claiming 999 above a child
    /// at 1; it is now refused.
    ///
    /// Reachability is unchanged, and it is the block processor's own call
    /// (`blocks/block_processor.rs:55,:132` → `insert`): the failed block is already in the DAG, and a
    /// peer then sends a block naming it. `neglected_invalid_block` is what makes the *bonded* case
    /// closed and this one open — the failed sender here is unbonded (see the test below), so the
    /// refusal below is the descent bound's and nothing else's.
    #[tokio::test]
    async fn h1b_a_failed_parent_above_the_childs_height_is_refused() {
        let storage = build_storage().await;
        let genesis = hash(0);
        storage
            .insert(meta(genesis, &[], 0), block(genesis))
            .await
            .unwrap();

        // A block that fails validation while claiming height 999. H-2 records it in the message map
        // with that claimed number, and keeps it out of `latest_msgs`.
        let failed = hash(1);
        let mut failed_meta = meta_by(1, 0, failed, &[genesis], 999);
        failed_meta.validation_failed = true;
        storage.insert(failed_meta, block(failed)).await.unwrap();

        // The child: proposer `2`, height 1 (genesis 0 + 1 from the unfailed maximum), justifying the
        // failed block. Validator `1` is not in the child's bond map, so the neglect rule does not
        // refuse it — the refusal must come from the descent bound alone.
        let child = hash(2);
        let child_block = block_with(child, 1, 2, 0, &[genesis, failed], &[]);
        assert!(
            matches!(
                crate::validate::neglected_invalid_block(&*storage, &child_block)
                    .await
                    .unwrap(),
                Ok(())
            ),
            "an unbonded failed justification is not neglected, so this refusal is the bound's"
        );
        let verdict = crate::validate::block_number(&*storage, &child_block)
            .await
            .unwrap();
        assert!(
            !matches!(verdict, Ok(())),
            "a resolved parent above the block's number must be refused: {verdict:?}"
        );

        // The control, so the test cannot pass for the wrong reason: the failed parent really is above
        // the child's number — and the refused child is therefore not in the DAG, which is the state
        // the model's premise excludes.
        let repr = storage.get_representation().await;
        let parent = repr.dag_message_state.msg_map.get(&failed).unwrap();
        assert!(
            i64::from(parent.height) > 1,
            "the parent is above the child's number ({})",
            i64::from(parent.height)
        );
        assert!(
            !repr.dag_message_state.msg_map.contains_key(&child),
            "the refused child never enters the DAG"
        );
    }

    /// The route the H1b brief named — a child *forced* to justify a bonded failed block — is not
    /// how the port behaves: `neglected_invalid_block` is faithful to the oracle
    /// (`legacy/casper/.../Validate.scala:340-360`) and **refuses** a child that justifies a failed
    /// block whose sender is still bonded. Nothing here forces such a justification; the rule above
    /// is what a child is refused for. Reachability of the `Descends` violation therefore runs through
    /// the unbonded case, and the bonded one is closed.
    #[tokio::test]
    async fn h1b_a_justified_bonded_failed_block_is_refused_rather_than_forced() {
        let storage = build_storage().await;
        let genesis = hash(0);
        storage
            .insert(meta(genesis, &[], 0), block(genesis))
            .await
            .unwrap();
        let failed = hash(1);
        let mut failed_meta = meta_by(1, 0, failed, &[genesis], 999);
        failed_meta.validation_failed = true;
        storage.insert(failed_meta, block(failed)).await.unwrap();

        // Same child, but validator `1` now holds stake in the child's bond map.
        let child_block = block_with(hash(2), 1, 2, 0, &[genesis, failed], &[(1, 100)]);
        let verdict = crate::validate::neglected_invalid_block(&*storage, &child_block)
            .await
            .unwrap();
        assert!(
            !matches!(verdict, Ok(())),
            "justifying a bonded failed block is refused, not forced: {verdict:?}"
        );
    }

    /// **H1c.** A store that already contains a fork is refused on restore, exactly as `insert`
    /// refuses it at ingress.
    ///
    /// `create` rebuilds the message map from persisted metadata, and the persisted layer checks only
    /// that the height numbers are contiguous (`validate_dag_state`) — it never looks at
    /// `(sender, seq_num)`. So a store holding two distinct blocks by one sender reusing a sequence
    /// number — written by a build that predates H-1's gate, or by any path that writes metadata
    /// directly — restores into a DAG holding the equivocation that gate exists to keep out, and the
    /// H-1 stall it prevents is live again with nothing saying so. This test writes such a store and
    /// asserts the refusal names both blocks; a `create` without the re-check returns `Ok` here and
    /// this assertion fails.
    #[tokio::test]
    async fn h1c_a_store_holding_an_equivocation_is_refused_on_restore() {
        let metadata_store = empty_metadata_store().await;
        let genesis = hash(0);
        metadata_store.add(meta(genesis, &[], 0)).await.unwrap();
        // Two distinct blocks, one sender, one `seq_num`: the fork `insert` refuses (H-1).
        metadata_store
            .add(meta_by(1, 0, hash(1), &[genesis], 1))
            .await
            .unwrap();
        metadata_store
            .add(meta_by(1, 0, hash(2), &[genesis], 1))
            .await
            .unwrap();

        let deploy_index: Arc<dyn KeyValueTypedStore<DeployId, BlockHash>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BytesCodec),
                Arc::new(BlockHashCodec),
            ));
        let deploy_store: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BytesCodec),
                Arc::new(SignedDeployDataCodec),
            ));

        let err = match BlockDagKeyValueStorage::create(metadata_store, deploy_index, deploy_store)
            .await
        {
            Ok(_) => panic!("a store that already contains a fork must not restore silently"),
            Err(e) => e,
        };
        assert!(err.contains("equivocation"), "{err}");
        assert!(err.contains("sequence number"), "{err}");
    }

    /// **The falsifier for C193's fix.** The restoring rule's write must reach the store *and* take the
    /// block back into the two structures H-2 excluded it from — which is what makes the rule a rule.
    ///
    /// Every assertion here is one the old path failed: the store kept `validation_failed: true`, and
    /// `latest_msgs` never gained the block, so a proposer kept deriving the sequence number it had
    /// already spent (C190's wedge, which is why these two rows travel together).
    #[tokio::test]
    async fn an_update_clears_the_failure_record_and_promotes_the_block() {
        let storage = build_storage().await;
        let genesis = hash(0);
        storage
            .insert(meta(genesis, &[], 0), block(genesis))
            .await
            .unwrap();

        let failed = hash(1);
        let mut failed_meta = meta_by(1, 0, failed, &[genesis], 1);
        failed_meta.validation_failed = true;
        storage
            .insert(failed_meta.clone(), block(failed))
            .await
            .unwrap();
        assert!(
            !storage
                .get_representation()
                .await
                .dag_message_state
                .latest_msgs
                .values()
                .any(|m| m.id == failed),
            "the control: H-2 keeps a failed block out of `latest_msgs` to begin with"
        );

        // What the restoring rule now writes on a successful revalidation.
        let cleared = BlockMetadata {
            validation_failed: false,
            failure_cause: None,
            slash_severity: SlashSeverity::Unspecified,
            ..failed_meta.clone()
        };
        storage.update_metadata(cleared).await.unwrap();

        let stored = storage.lookup(&failed).await.unwrap().unwrap();
        assert!(
            !stored.validation_failed,
            "the update must reach the store — the old path returned Ok without writing"
        );
        let after = storage.get_representation().await;
        assert!(
            after
                .dag_message_state
                .latest_msgs
                .values()
                .any(|m| m.id == failed),
            "and the block must be the sender's latest message again: this is what advances the proposer's \
             arithmetic past the sequence number it had spent (C190)"
        );
        assert!(
            after.height_map.values().any(|s| s.contains(&failed)),
            "and it returns to the height map, which does not count a failed block"
        );
    }

    /// **`insert` is not an update — and the restoring rule uses it as one.**
    ///
    /// `restore_divergent_justifications` re-validates a stored justification and, on success, writes the
    /// cleared record back with `dag.insert(record, msg)` (`casper/src/multi_parent_casper.rs:473`). But
    /// `revalidated_record` keeps the block's **hash** (`:394-409`, `..fresh`/`..stored.clone()`), the
    /// failed block is already in the store — `add_block_to_dag_state_mut` puts every hash in `dag_set`
    /// unconditionally (`block-storage/src/dag/metadata_store.rs:71`) — and `insert` short-circuits on a
    /// known hash (`:322-328`). So the write never happens: the store keeps `validation_failed: true`,
    /// the representation is never touched, and the rule logs *"cleared the failure record for … the block
    /// validates"* (`multi_parent_casper.rs:485-490`) while doing nothing at all.
    ///
    /// **This is why C173's clearing branch had never been verified end to end**, and it is invisible to
    /// `casper/tests/restoring_rule.rs` by construction: that file drives a **fake** storage whose
    /// `insert` unconditionally records (`inserts_for`), so the rule looks correct there and the real
    /// short-circuit is never in the path.
    ///
    /// The assertion pins the defect rather than failing on it, so the tree stays honest — invert it (and
    /// the rule's own success test beside it) when the clearing gets a write path that is an update.
    #[tokio::test]
    async fn re_inserting_a_known_block_does_not_clear_its_failure_record() {
        let storage = build_storage().await;
        let genesis = hash(0);
        storage
            .insert(meta(genesis, &[], 0), block(genesis))
            .await
            .unwrap();

        let failed = hash(1);
        let mut failed_meta = meta_by(1, 0, failed, &[genesis], 1);
        failed_meta.validation_failed = true;
        storage
            .insert(failed_meta.clone(), block(failed))
            .await
            .unwrap();
        assert!(
            storage
                .lookup(&failed)
                .await
                .unwrap()
                .unwrap()
                .validation_failed,
            "the control: the failed record is in the store to begin with"
        );

        // Exactly what the restoring rule writes on a successful revalidation.
        let cleared = BlockMetadata {
            validation_failed: false,
            failure_cause: None,
            slash_severity: SlashSeverity::Unspecified,
            ..failed_meta.clone()
        };
        storage.insert(cleared, block(failed)).await.unwrap();

        let stored = storage.lookup(&failed).await.unwrap().unwrap();
        assert!(
            stored.validation_failed,
            "the cleared record did NOT reach the store: `insert` returned Ok without writing, so the \
             restoring rule's success path is a no-op that reports success. If this assertion fails, a \
             real update path landed — invert it (C173's clearing branch, and the self-clearing step a \
             proposer needs when its own record is spent — `casper/tests/finalization.rs`, #156)"
        );
    }

    /// **A completion record that does not describe the index is not believed** (W-N11-02).
    ///
    /// The row: the cursor's "complete" value is a claim *about the store*, and a host that writes it —
    /// a planted data directory, or the key set by hand — makes the backfill return without indexing
    /// anything, so the node reports the index complete while every block below the cut was never
    /// indexed and its deployer lookups answer empty. The completion now carries the record count it
    /// was written with, so the claim is checkable in one `count()`, and one that does not match is
    /// walked instead of believed.
    ///
    /// **Falsified by believing the claim again** — dropping the check, which is what the tree did —
    /// under which the first call below returns 0 blocks read and the lookup still answers `block:
    /// None`. The second call is the other half: once the walk has stamped the store, the completion
    /// *is* believed, so the check costs one walk and not one per boot.
    #[tokio::test]
    async fn a_completion_record_that_does_not_describe_the_index_is_not_believed() {
        let metadata_store = empty_metadata_store().await;
        let block_store: Arc<dyn KeyValueTypedStore<BlockHash, BlockMessage>> =
            Arc::new(KeyValueTypedStoreCodec::new(
                in_memory(),
                Arc::new(BlockHashCodec),
                Arc::new(rchain_block_storage::dag::codecs::BlockMessageCodec),
            ));
        let (h1, h2) = (hash(1), hash(2));
        {
            let old = build_storage_over(metadata_store.clone()).await;
            for (h, parents, height, keys) in
                [(h1, vec![], 0, vec![7u8]), (h2, vec![h1], 1, vec![8u8])]
            {
                let b = block_signed_by(h, &keys);
                block_store.put(&[(h, b.clone())]).await.unwrap();
                old.insert(meta_seq(h, &parents, height), b).await.unwrap();
            }
        }

        let index = deployer_index_store();

        // **The plant, before the index is attached** — which is the order a planted data directory
        // arrives in, and the order that matters: `with_deployer_index` reads the stored cursor, so a
        // `0` planted first is the `0` the node boots believing. (Planting it after the attach instead
        // leaves the in-memory cursor at the tip, and the backfill walks for the ordinary reason — the
        // first draft of this test did that and stayed green under the mutation.)
        index
            .put(&[(
                DEPLOYER_INDEXED_FROM_KEY.to_vec(),
                0i64.to_be_bytes().to_vec(),
            )])
            .await
            .unwrap();

        let upgraded = build_storage_over_unshared(metadata_store)
            .await
            .with_deployer_index(index.clone())
            .await
            .unwrap();

        assert_eq!(
            upgraded
                .backfill_deployer_index(&block_store)
                .await
                .unwrap(),
            2,
            "a completion that does not describe this store is walked, not believed"
        );
        assert_eq!(
            upgraded
                .lookup_by_deployer(&deployer_index_key(&[7u8; 65]))
                .await
                .unwrap()
                .block,
            Some(h1),
            "and the deployers the plant hid are indexed by that walk"
        );

        assert_eq!(
            upgraded
                .backfill_deployer_index(&block_store)
                .await
                .unwrap(),
            0,
            "the completion the walk wrote does describe the store, so the next boot believes it"
        );
    }
}
