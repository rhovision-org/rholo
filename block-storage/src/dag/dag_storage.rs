//! Block DAG storage interface.
//!
//! Mirrors `block-storage/src/main/scala/coop/rchain/blockstorage/dag/BlockDagStorage.scala`. The
//! concrete `BlockDagKeyValueStorage` is casper-owned in Scala, so only the trait is ported here.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;

use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::BlockMetadata;
use rchain_models::casper::protocol::casper_message::{BlockMessage, SignedDeployData};
use rchain_shared::lock::Unpoison;
use rchain_shared::refined::BlockHeight;

use super::representation::DagRepresentation;

/// A deploy id (the Scala `BlockDagStorage.DeployId = ByteString`).
pub type DeployId = Vec<u8>;

/// **What a node must remember, across a restart, about a reset it performed** (F-U5-01's acting half,
/// register C274).
///
/// **Why the node has to remember anything at all.** A reset is two store operations — the DAG's rewind
/// (`drop_above`) and the removal of the bodies it dropped (`BlockStore`) — and a process that dies
/// between them leaves bodies with no DAG entries. That state is not inert: `block_receiver::not_validated`
/// reads a stored body the DAG does not hold as *"stored but not yet validated"*, re-injects it, and the
/// next child of a dropped block **re-inserts the dropped block above the anchor**, undoing the reset. The
/// window cannot be closed by ordering (the hashes to purge are what the rewind returns) and the reverse
/// order is worse (metadata whose bodies are gone is a wedge), so the node has to be able to *name* the
/// state it is in.
///
/// **Two phases, and the phase is load-bearing.** `Rewinding` is written *before* the first destructive
/// operation; `Rewound` once both have returned. A boot that finds `Rewinding` knows only that a rewind
/// was **intended** and re-applies both operations (both are idempotent); a boot that finds `Rewound`
/// knows the stores already agree and must **not** rewind again — by then the node may have caught up and
/// produced above the anchor, and re-running `drop_above` would throw those blocks away. That is the one
/// way this type could do harm, and the reason it is a phase and not a flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetMarker {
    /// The block the node rewound to — kept as the state replayed from, and the root the catch-up walk
    /// is anchored at on the next boot.
    pub anchor: BlockHash,
    /// The anchor's height, so the boot guard can rewind without reading the whole block store.
    pub height: BlockHeight,
    pub phase: ResetPhase,
}

/// Which half of a reset a [`ResetMarker`] records. See the type's own doc for why this is not a flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetPhase {
    /// Written before `drop_above`: the rewind may not have happened, so a boot re-applies it.
    Rewinding,
    /// Written after the body purge returned: the two stores agree, so a boot re-applies nothing.
    Rewound,
}

/// The reset marker's one key: a literal, not a block hash, so it cannot collide with a block entry
/// (LMDB refuses an empty key, which is why it is not the empty string).
pub const RESET_MARKER_KEY: &[u8] = b"last-reset";

impl ResetMarker {
    /// Anchor (32) ++ height (big-endian i64, 8) ++ phase (1). Fixed width, so a decode cannot be
    /// ambiguous and a truncated value is refused rather than guessed at.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(41);
        out.extend_from_slice(self.anchor.as_bytes().as_slice());
        out.extend_from_slice(&i64::from(self.height).to_be_bytes());
        out.push(match self.phase {
            ResetPhase::Rewinding => 0,
            ResetPhase::Rewound => 1,
        });
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 41 {
            return Err(format!(
                "a reset marker is 41 bytes (anchor ++ height ++ phase) and this one is {}",
                bytes.len()
            ));
        }
        let mut anchor = [0u8; 32];
        anchor.copy_from_slice(&bytes[..32]);
        let mut height = [0u8; 8];
        height.copy_from_slice(&bytes[32..40]);
        let height = BlockHeight::try_from(i64::from_be_bytes(height))
            .map_err(|e| format!("the reset marker's height is not a block height: {e}"))?;
        let phase = match bytes[40] {
            0 => ResetPhase::Rewinding,
            1 => ResetPhase::Rewound,
            other => {
                return Err(format!(
                "the reset marker's phase byte is {other}, and only 0 (rewinding) and 1 (rewound) \
                     mean anything"
            ))
            }
        };
        Ok(ResetMarker {
            anchor: BlockHash::new(anchor),
            height,
            phase,
        })
    }
}

/// **What a shard must remember, across a restart, about an autopropose halt it took** (W-N10-01,
/// F-U10-02).
///
/// **Why a flag was not enough.** `autopropose_timer_halted` is an in-memory cell, so a node that
/// halted and was then restarted reports `false` — indistinguishable from a node that never halted.
/// And the operator's *prescribed* remedy for a halt is a restart (`AUTOPROPOSE_MAX_CONSECUTIVE_FAILURES`
/// at `node/src/runtime/node_runtime.rs`), so the remedy itself is what destroyed the record of the
/// fault. This is the record that survives it.
///
/// **It is obeyed, not merely reported** (decided 2026-10-14). A shard whose record stands does not
/// start its autopropose timer, so a restart does not silently resume production on a fault nobody has
/// cleared. There is exactly one way out — `POST /api/v1/autopropose/resume` on the admin listener — and
/// the "exactly one" is the point: a record that halted a node with no way to stop halting is the same
/// class of defect as one that could not clear, which is W-N11-01's class (an on-disk record choosing
/// what the node does).
///
/// **Not a `ResetMarker`.** That record is *two-phased* because its write has a caller that can refuse
/// (`write_reset_marker`'s default does). This one is written by the timer *after* it has decided to
/// stop, so there is nothing left to abort — the write either reaches the store or it does not, and
/// [`HaltRecord::durable`] is how the surface says which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HaltRecord {
    /// When the halt was most recently taken, in milliseconds since the unix epoch. A date is what
    /// separates "halted a minute ago" from "halted last week", which a bare flag cannot.
    pub at_ms: i64,
    /// **Monotone**: how many times this shard's timer has halted. A count cannot reverse, so it is the
    /// half that survives where the flag's `false` cannot.
    pub episodes: u64,
    /// Whether the write that produced this record reached the store.
    ///
    /// `false` means this process is honouring a halt the *next* process will not know about — the
    /// store was absent or the write failed. The surface says so rather than implying a durability it
    /// does not have.
    pub durable: bool,
}

/// The halt record's one key: a literal, so it cannot collide with a block entry or with
/// [`RESET_MARKER_KEY`] in the store they share.
pub const HALT_RECORD_KEY: &[u8] = b"autopropose-halt";

/// **The per-shard health counters that must survive a restart** (W-N10-02).
///
/// **Why these three and not the rest.** The health surface carries counters of two lifetimes, and the
/// distinction is the finding: `stall_episodes`, `non_quiet_merge_reports` and `unrestorable_records`
/// count events **this shard's history** contains, so a restart that zeroes them understates what
/// happened — an operator restarting to clear an alarm is told the shard has never stalled. The others
/// (`poison_recoveries`, the supervisor's `tasks_*`, `undelivered_sends`, `sync.attempts`) are the
/// *process's* own running totals, correctly zero after a restart, and are labelled `SinceProcessStart`
/// on the wire rather than being persisted.
///
/// **Written on the transition, not on the observation.** A block is validated every round and most are
/// refused by the stake gate, so "persist the counter" would be a store write per validated block. The
/// writers call for a persist only when a counter actually moves — an episode begins, a merge reports
/// non-quiet, a record is given up on — which is rare by construction.
///
/// **Not a two-phased record like [`ResetMarker`] or an obeyed one like [`HaltRecord`].** Nothing reads
/// these to decide what the node *does*: they are observations, seeded at boot so the surface reports
/// the shard's history rather than this process's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HealthCounters {
    pub stall_episodes: u64,
    pub non_quiet_merge_reports: u64,
    pub unrestorable_records: u64,
}

/// The health counters' one key, a literal for the same reason as the other two.
pub const HEALTH_COUNTERS_KEY: &[u8] = b"shard-health-counters";

/// **The one-time legacy body scan's completion record** (U19/F-U19-07). A literal in the same
/// durable-records store as the reset/halt/health keys; a value of [`LEGACY_BODY_SCAN_VERSION`]
/// means "this store has been walked for bodies with no disposition row". Written only after the
/// walk, so a crash before it rescans on the next boot.
pub const LEGACY_BODY_SCAN_KEY: &[u8] = b"legacy-body-scan";

/// The scan's meaning. Bump this if a later scan must run again on a store already marked.
///
/// **Version 2 is the move to the block-storage environment** (C350). The ledger's rows moved with
/// `STORE_FORMAT_VERSION` 4, so an upgraded node opens an empty ledger over a populated body store;
/// the scan adopts every body the DAG does not hold — exactly the unsettled set — and writes the new
/// marker. What is lost is a `Faulted` row on an unsettled body: it comes back as `Pending` with its
/// attempt count reset, which retries a body that had failed rather than dropping one.
pub const LEGACY_BODY_SCAN_VERSION: u8 = 2;

impl HealthCounters {
    /// Three big-endian u64s, 24 bytes. Fixed width, so a decode cannot be ambiguous and a truncated
    /// value is refused rather than guessed at — the codec property [`ResetMarker`]'s and
    /// [`HaltRecord`]'s both keep.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24);
        out.extend_from_slice(&self.stall_episodes.to_be_bytes());
        out.extend_from_slice(&self.non_quiet_merge_reports.to_be_bytes());
        out.extend_from_slice(&self.unrestorable_records.to_be_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 24 {
            return Err(format!(
                "health counters are 24 bytes (three u64s) and this value is {}",
                bytes.len()
            ));
        }
        let read = |i: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&bytes[i * 8..i * 8 + 8]);
            u64::from_be_bytes(b)
        };
        Ok(HealthCounters {
            stall_episodes: read(0),
            non_quiet_merge_reports: read(1),
            unrestorable_records: read(2),
        })
    }
}

impl HaltRecord {
    /// `at_ms` (big-endian i64, 8) ++ `episodes` (big-endian u64, 8). Fixed width, so a decode cannot
    /// be ambiguous and a truncated value is refused rather than guessed at — the property
    /// [`ResetMarker`]'s own codec keeps and the reason it is trusted.
    ///
    /// `durable` is **not** encoded: a record that is in the store is durable by construction, and one
    /// that is not is not there to decode. The flag is a fact about the write, not about the bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16);
        out.extend_from_slice(&self.at_ms.to_be_bytes());
        out.extend_from_slice(&self.episodes.to_be_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 16 {
            return Err(format!(
                "a halt record is 16 bytes (at_ms ++ episodes) and this one is {}",
                bytes.len()
            ));
        }
        let mut at = [0u8; 8];
        at.copy_from_slice(&bytes[..8]);
        let mut episodes = [0u8; 8];
        episodes.copy_from_slice(&bytes[8..]);
        Ok(HaltRecord {
            at_ms: i64::from_be_bytes(at),
            episodes: u64::from_be_bytes(episodes),
            durable: true,
        })
    }
}

/// What a node knows about one deployer key: a block that includes a deploy it signed, and the height
/// from which that knowledge is complete.
///
/// `indexed_from` is the lowest height at and above which **every** block this node has inserted was
/// indexed. It is never below the lowest block the node holds: a node that started indexing on an
/// existing chain has not read the blocks below its marker, and a node that joined by last-finalized-
/// state sync never stored the blocks below the fringe. So `block: None` with `indexed_from > 0` means
/// "not in any block from `indexed_from` up" — never "this key has never signed". A node that holds
/// and has indexed every block from genesis reports `0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployerLookup {
    pub block: Option<BlockHash>,
    pub indexed_from: i64,
}

/// The block DAG storage interface (port of `BlockDagStorage[F]`). The concrete implementation is
/// the `casper` crate's `BlockDagKeyValueStorage`.
#[async_trait]
pub trait BlockDagStorage: Send + Sync {
    /// The current DAG representation, **shared rather than copied**.
    ///
    /// A `DagRepresentation` owns every message, and each message's `seen` is its whole ancestry, so
    /// it is Θ(N²) in the number of blocks. Returning it by value made every caller pay that copy —
    /// ~35 call sites, including every per-block path and every peer-request handler, with the read
    /// lock held for the whole copy; on a node serving a syncing peer that was the difference between
    /// a flat curve and ~0.86 GiB per block. Callers read fields through `Deref`, so the change is
    /// invisible to them; the writer takes the copy at most once per insert via `Arc::make_mut`.
    async fn get_representation(&self) -> Arc<DagRepresentation>;

    /// **The equivocating blocks this node has refused**, as serialized blocks keyed by the sender that
    /// equivocated (AUDIT C200).
    ///
    /// The H-1 gate refuses such a block *before any write*, so nothing else in the tree holds it — and
    /// a proposer cannot slash for an equivocation it cannot show. Recording it here gives the evidence
    /// somewhere to live that is **not** consensus state until a block actually carries it, and reading
    /// it is what lets a proposer attach a proof every other node can check for itself.
    ///
    /// The default is empty, and that is a statement rather than a stub: a storage that does not record
    /// them has none, and only the real implementation can.
    async fn recorded_equivocations(&self) -> Vec<(rchain_models::validator::Validator, Vec<u8>)> {
        Vec::new()
    }

    async fn insert(
        &self,
        block_metadata: BlockMetadata,
        block: BlockMessage,
    ) -> Result<(), String>;

    /// **Update the record of a block the DAG already holds** — the write the restoring rule needs, and
    /// `insert` is not it (AUDIT C193).
    ///
    /// `insert`'s contract is to *add* a block, and a re-insert of a hash it already holds is a no-op —
    /// which is right for that contract and wrong for this one. `restore_divergent_justifications`
    /// re-validates a stored justification and writes back the **same block's** record with
    /// `validation_failed` cleared; through `insert` that returned `Ok(())` without writing, so the rule
    /// logged *"cleared the failure record"* while the store kept the failure and the in-memory
    /// representation was never touched. The rule has never cleared anything.
    ///
    /// An update also has to undo the two structures that excluded the block while it was failed: the
    /// height map (which does not count a failed block) and `latest_msgs` (H-2, so a failed block cannot
    /// be a parent — and therefore cannot advance the proposer's arithmetic, which is the sequence-number
    /// wedge that made this visible).
    ///
    /// **The default refuses rather than silently doing nothing.** A no-op default is the defect this
    /// method exists to fix — a write that returns `Ok` and changes nothing is precisely what the
    /// restoring rule has been doing — so a backend that cannot update says so, and the failure is
    /// visible in the log that names the block.
    async fn update_metadata(&self, block_metadata: BlockMetadata) -> Result<(), String> {
        Err(format!(
            "this DAG storage cannot update the record of {} (no `update_metadata` impl)",
            block_metadata.block_hash.to_hex()
        ))
    }

    /// **Rewind this node to `height`, dropping everything above it and keeping the block at it** (the
    /// anchor). Returns the dropped hashes, because the reset is not the only thing that has to be
    /// rewound: the derived deploy and deployer indices point at blocks that no longer exist.
    ///
    /// On the trait rather than only on the key-value backend since #320: a diverged *running* node
    /// re-enters sync by rewinding, and the engine holds a `dyn BlockDagStorage` — the rewind is part
    /// of the storage contract now, not an implementation detail of one backend. **The default
    /// refuses**, which is the honest answer for a backend that cannot rewind (a test stub, or a
    /// storage that keeps only a tip): a silent no-op would leave the node believing it had reset.
    async fn drop_above(&self, _height: BlockHeight) -> Result<Vec<BlockHash>, String> {
        Err("this DAG storage cannot rewind (no `drop_above` impl)".to_string())
    }

    /// **Remove derived-index rows that point at a block this DAG does not hold** (B39, HAZOP
    /// F-U17-11): the deploy index and the deployer index, returning how many rows each lost.
    ///
    /// The repair's subject is the **surviving block set**, not the list a particular rewind happened
    /// to drop. A crash between dropping metadata and cleaning the indexes leaves rows pointing at
    /// blocks that are gone; a later rewind that drops nothing must still remove them, and so must
    /// boot on an upgraded store. The rule is idempotent: a second call finds nothing to remove.
    ///
    /// The default answers `(0, 0)`, which is honest for a backend that keeps no derived indexes —
    /// unlike the refusing defaults above, there is nothing here that a no-op could leave believed.
    async fn repair_derived_indexes(&self) -> Result<(usize, usize), String> {
        Ok((0, 0))
    }

    /// **Record that this node is rewinding to `marker.anchor`, and that it has finished** (see
    /// [`ResetMarker`]) — the write that lets a restart repair a reset that crashed half-way.
    ///
    /// **The default refuses rather than doing nothing.** A reset whose durable half cannot be recorded
    /// is a reset a crash can undo silently, and a no-op write would let the caller believe it had
    /// recorded one — the same reasoning `update_metadata`'s refusing default carries. The node attaches
    /// a store for this (`reset-marker`); a fixture that drives a real reset attaches one too.
    async fn write_reset_marker(&self, _marker: ResetMarker) -> Result<(), String> {
        Err("this DAG storage cannot record a reset (no `write_reset_marker` impl)".to_string())
    }

    /// **Has this node reset itself and not yet caught up?** The default answers *no*, which is honest
    /// for a backend that records nothing: there is no marker, so there is nothing to repair.
    async fn read_reset_marker(&self) -> Result<Option<ResetMarker>, String> {
        Ok(None)
    }

    /// Clear the marker once the node has caught up past its anchor. The default does nothing, for the
    /// same reason the read answers `None`: a backend with no marker has nothing to clear.
    async fn clear_reset_marker(&self) -> Result<(), String> {
        Ok(())
    }

    /// **Has this store been walked for pre-ledger bodies?** The default answers *no*: a backend
    /// with no durable-records store cannot record the scan, and the scan must not run with nowhere
    /// to write its marker.
    async fn read_legacy_body_scan_marker(&self) -> Result<Option<u8>, String> {
        Ok(None)
    }

    /// Record that the legacy body scan completed. The default refuses, like `write_reset_marker`:
    /// a backend that cannot record it must not look like one that did.
    async fn write_legacy_body_scan_marker(&self, _version: u8) -> Result<(), String> {
        Err("this DAG storage cannot record a legacy body scan".to_string())
    }

    /// **Record that this shard's autopropose timer has halted** (see [`HaltRecord`]) — the write that
    /// lets a restart know about a fault it would otherwise forget.
    ///
    /// **The default refuses rather than doing nothing**, like `write_reset_marker`'s and for the same
    /// reason: a backend that cannot record the halt must not look like one that did. The difference is
    /// what the caller does with the refusal — the timer has already decided to stop, so it cannot
    /// abort; it records `durable: false` on the reading the surface serves instead.
    async fn write_halt_record(&self, _record: HaltRecord) -> Result<(), String> {
        Err(
            "this DAG storage cannot record an autopropose halt (no `write_halt_record` impl)"
                .to_string(),
        )
    }

    /// The halt this shard took, if it took one. The default answers *no*, which is honest for a
    /// backend that records nothing.
    async fn read_halt_record(&self) -> Result<Option<HaltRecord>, String> {
        Ok(None)
    }

    /// **Forget the halt** — the one way out, reached by `POST /api/v1/autopropose/resume` on the admin
    /// listener. The default does nothing, for the same reason the read answers `None`.
    async fn clear_halt_record(&self) -> Result<(), String> {
        Ok(())
    }

    /// **Record this shard's monotone health counters** (see [`HealthCounters`]) — the write that stops
    /// a restart from understating them (W-N10-02).
    ///
    /// **The default refuses**, like `write_reset_marker`'s and `write_halt_record`'s: a backend that
    /// cannot record them must not look like one that did. Unlike the halt's write, the caller here is
    /// the *validation path*, which is already past its decision and cannot refuse either — so it is
    /// here that the surface's honesty rule lives: a counter that could not be written is reported on
    /// the reading (the parent's `durable`-style qualification), not silently dropped.
    async fn write_health_counters(&self, _counters: HealthCounters) -> Result<(), String> {
        Err(
            "this DAG storage cannot record health counters (no `write_health_counters` impl)"
                .to_string(),
        )
    }

    /// The health counters this shard last recorded, if any. The default answers `None`, which is
    /// honest for a backend that records nothing — the surface reads it as "since process start".
    async fn read_health_counters(&self) -> Result<Option<HealthCounters>, String> {
        Ok(None)
    }

    async fn lookup(&self, block_hash: &BlockHash) -> Result<Option<BlockMetadata>, String>;

    /// Look up a block hash by the deploy id included in the DAG.
    async fn lookup_by_deploy_id(&self, deploy_id: &DeployId) -> Result<Option<BlockHash>, String>;

    /// A block containing a deploy signed by the key whose `blake2b256` hash is `deployer_hash`, from
    /// the deployer index — the first such block this node inserted. It takes the hash, not the key,
    /// so a wallet checking a key that has never signed need not reveal that key to the node. No Scala counterpart: it serves the wallet's
    /// "has this key been revealed?" check (quantum key hygiene), which otherwise has to scan every
    /// block. The default refuses rather than answering `None`: a backend without the index cannot say
    /// a key is unseen.
    async fn lookup_by_deployer(&self, _deployer_hash: &[u8]) -> Result<DeployerLookup, String> {
        Err("this DAG storage keeps no deployer index".to_string())
    }

    /// Add a deploy to the (unprocessed) deploy pool.
    async fn add_deploy(&self, deploy: SignedDeployData) -> Result<(), String>;

    /// **Reserve the submitter's place in the pool, before its term is parsed** (W-N2-05, W-N12-03).
    ///
    /// The split from [`Self::admit_deploy`]'s other half exists because the term is parsed and
    /// normalized *between* the two: a deploy that the submitter's own bounds would refuse must be
    /// refused before that work, and a deploy whose parse fails must give its place back. `add_deploy`
    /// is both halves at once, which is the right shape when there is nothing in between.
    ///
    /// The default reserves nothing, so a test double that does not track a per-submitter share does
    /// not have to implement this; the concrete storage is where the share lives.
    async fn admit_deploy(&self, _deployer: &[u8]) -> Result<DeployPermit, String> {
        Ok(DeployPermit::default())
    }

    /// **Hold `deploy_id` for this admission, before its term is parsed** (B36, W-N2-05's other half).
    ///
    /// `admit_deploy` bounds the *submitter*; this bounds the *work*. A byte-identical resubmission is
    /// one deploy the pool stores once, but without a reservation the second caller parses the term
    /// again — and two concurrent submissions of the same deploy can both do so, because a store read
    /// before the parse cannot reserve anything. The returned permit removes the id when it drops; the
    /// deploy's own row is the durable record from then on.
    ///
    /// The default reserves nothing, which is honest for a backend that keeps no pool: there is
    /// nothing for a fixture without a deploy store to duplicate.
    async fn reserve_deploy_id(&self, _deploy_id: &DeployId) -> Result<DeployIdPermit, String> {
        Ok(DeployIdPermit::default())
    }

    /// Store a deploy that was admitted, handing over the place [`Self::admit_deploy`] reserved.
    ///
    /// The default is the whole of `add_deploy`, which is the pre-split behaviour: a storage that does
    /// not implement the reservation cannot have one to hand over.
    async fn commit_deploy(
        &self,
        _permit: DeployPermit,
        deploy: SignedDeployData,
    ) -> Result<(), String> {
        self.add_deploy(deploy).await
    }

    async fn pooled_deploys(&self) -> Result<BTreeMap<DeployId, SignedDeployData>, String>;

    async fn contains_deploy_in_pool(&self, deploy_id: &DeployId) -> Result<bool, String>;

    /// The pool's current count and term bytes, for the status surface's capacity answer. A storage
    /// that does not track a pool returns the default (an empty, unbounded-looking pool), which is
    /// what a stub API means by "not the thing that is full". Law 86a: admission depends on the pool's
    /// bounds, and the status surface must read the same bounds.
    async fn deploy_pool_stats(&self) -> Result<(usize, usize), String> {
        Ok((0, 0))
    }
}

/// A reservation of the submitter's place in the deploy pool, from admission until the deploy is
/// stored — or until it is dropped, which returns the place.
///
/// **Opaque, and that is the point:** what it holds is the storage's business, and the caller that
/// parses the term in between has no use for it. `Default` reserves nothing, which is what a storage
/// that does not track a share returns from [`BlockDagStorage::admit_deploy`].
#[derive(Default)]
pub struct DeployPermit {
    /// The share this permit holds: the pool's per-deployer counts, and the key it claimed in them.
    /// `None` for a storage with no such accounting.
    ///
    /// An `Arc` because the permit outlives any borrow of the storage it came from — the term is
    /// parsed between admission and commit, and holding a borrow across that would be a borrow of the
    /// DAG for as long as a caller's parse takes.
    share: Option<(
        Arc<std::sync::Mutex<std::collections::HashMap<Vec<u8>, usize>>>,
        Vec<u8>,
    )>,
    /// Set by [`BlockDagStorage::commit_deploy`], so dropping the permit after a successful commit
    /// does not give back a place the stored deploy now owns.
    committed: bool,
}

impl DeployPermit {
    /// Hand the reservation over to a stored deploy: the place is now the deploy's, and dropping this
    /// must not return it.
    pub fn commit(&mut self) {
        self.committed = true;
    }

    /// A permit that holds a share, for the storage that tracks one.
    pub fn holding(
        share: Arc<std::sync::Mutex<std::collections::HashMap<Vec<u8>, usize>>>,
        deployer: Vec<u8>,
    ) -> Self {
        DeployPermit {
            share: Some((share, deployer)),
            committed: false,
        }
    }
}

impl Drop for DeployPermit {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some((share, deployer)) = &self.share {
            release_deployer_share(share, deployer);
        }
    }
}

/// A claim on one deploy id from admission until the deploy is stored — or until it is dropped, which
/// releases the id so a failed parse or a failed commit does not lock it out.
///
/// Opaque like [`DeployPermit`], and for the same reason: the caller that parses the term has no use
/// for the set it holds, only for the guarantee that nobody else is parsing the same deploy.
#[derive(Default)]
pub struct DeployIdPermit {
    reservation: Option<(Arc<std::sync::Mutex<HashSet<DeployId>>>, DeployId)>,
}

impl DeployIdPermit {
    /// A permit that holds `deploy_id` in the storage's in-flight set.
    pub fn holding(
        reservations: Arc<std::sync::Mutex<HashSet<DeployId>>>,
        deploy_id: DeployId,
    ) -> Self {
        DeployIdPermit {
            reservation: Some((reservations, deploy_id)),
        }
    }
}

impl Drop for DeployIdPermit {
    fn drop(&mut self) {
        if let Some((reservations, deploy_id)) = &self.reservation {
            reservations.lock().unpoison().remove(deploy_id);
        }
    }
}

/// Return one place to `deployer`'s share, dropping the entry at zero.
///
/// An entry at zero carries nothing a missing one does not, and keeping it would let the map outlive
/// the deploys it counts — the rule `KeyedRateLimiter` applies to a bucket that has refilled.
///
/// **A missing entry is not an error.** Two removal paths can see the same deploy — an expiry and a
/// block that includes it — and the second releasing a place the first already returned is the
/// fail-*safe* direction: the share reads one too small, so a deployer may hold one deploy more than
/// its quota until a restart rebuilds the map from the store. The opposite, a *missed* release, is a
/// permanent lockout, and that is why every release happens only after the delete it accompanies has
/// succeeded, and why the lock is taken even when poisoned: this is a return path, and skipping it
/// would leak the place for the life of the process.
pub fn release_deployer_share(
    share: &std::sync::Mutex<std::collections::HashMap<Vec<u8>, usize>>,
    deployer: &[u8],
) {
    let mut counts = share.lock().unpoison();
    match counts.get_mut(deployer) {
        Some(held) if *held > 1 => *held -= 1,
        Some(_) => {
            counts.remove(deployer);
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The marker's wire form is fixed-width, and a decode is strict** (F-U5-01, register C274).
    ///
    /// The record is written before a destructive operation and read by a boot that has to decide
    /// whether to re-apply one, so a decoder that guessed at a malformed value would be deciding what to
    /// do with a store it does not understand. It refuses instead: exactly 41 bytes, and the phase byte
    /// has two meanings and no default.
    #[test]
    fn a_reset_marker_round_trips_and_a_malformed_one_is_refused() {
        for phase in [ResetPhase::Rewinding, ResetPhase::Rewound] {
            let marker = ResetMarker {
                anchor: BlockHash::new([7u8; 32]),
                height: BlockHeight::try_from(4_294_967_296i64).unwrap(),
                phase,
            };
            let bytes = marker.encode();
            assert_eq!(bytes.len(), 41, "anchor (32) ++ height (8) ++ phase (1)");
            assert_eq!(ResetMarker::decode(&bytes).unwrap(), marker);
        }

        // A truncated value, a phase byte with no meaning, and a height that is not a height.
        assert!(ResetMarker::decode(&[0u8; 40]).is_err(), "short");
        assert!(ResetMarker::decode(&[0u8; 42]).is_err(), "long");
        let mut bad_phase = ResetMarker {
            anchor: BlockHash::new([0u8; 32]),
            height: BlockHeight::zero(),
            phase: ResetPhase::Rewound,
        }
        .encode();
        bad_phase[40] = 9;
        assert!(ResetMarker::decode(&bad_phase).is_err(), "unknown phase");
        let mut bad_height = ResetMarker {
            anchor: BlockHash::new([0u8; 32]),
            height: BlockHeight::zero(),
            phase: ResetPhase::Rewinding,
        }
        .encode();
        bad_height[32..40].copy_from_slice(&(-1i64).to_be_bytes());
        assert!(ResetMarker::decode(&bad_height).is_err(), "negative height");
    }

    /// **The halt record's wire form is fixed-width too, and a decode is strict** (W-N10-01, F-U10-02).
    ///
    /// Same rule as the marker's codec and for the same reason: a boot that read a malformed record
    /// would be deciding what to do with a store it does not understand. It refuses rather than guessing,
    /// and `durable` is `true` on the way out because a value that came *from* the store is durable by
    /// construction — the flag is a fact about the write, not about the bytes, so it is not encoded.
    #[test]
    fn a_halt_record_round_trips_and_a_malformed_one_is_refused() {
        let record = HaltRecord {
            at_ms: 1_700_000_000_000,
            episodes: 7,
            durable: false,
        };
        let bytes = record.encode();
        assert_eq!(bytes.len(), 16, "at_ms (8) ++ episodes (8)");
        let decoded = HaltRecord::decode(&bytes).unwrap();
        assert_eq!(decoded.at_ms, record.at_ms);
        assert_eq!(decoded.episodes, record.episodes);
        assert!(
            decoded.durable,
            "a record read from the store is durable by construction, whatever the writer believed"
        );

        assert!(HaltRecord::decode(&[0u8; 15]).is_err(), "short");
        assert!(HaltRecord::decode(&[0u8; 17]).is_err(), "long");
        assert!(HaltRecord::decode(&[]).is_err(), "empty");
        assert_ne!(
            HALT_RECORD_KEY, RESET_MARKER_KEY,
            "the two records share a store, so their keys must not"
        );
    }

    /// **The health counters' codec, and the keys that keep the three records apart** (W-N10-02).
    #[test]
    fn health_counters_round_trip_and_a_malformed_one_is_refused() {
        let counters = HealthCounters {
            stall_episodes: 3,
            non_quiet_merge_reports: 0,
            unrestorable_records: 1,
        };
        let bytes = counters.encode();
        assert_eq!(bytes.len(), 24, "three big-endian u64s");
        assert_eq!(HealthCounters::decode(&bytes).unwrap(), counters);

        assert!(HealthCounters::decode(&[0u8; 23]).is_err(), "short");
        assert!(HealthCounters::decode(&[0u8; 25]).is_err(), "long");

        // A zeroed record is a real record, and it decodes to zeros rather than being confused with the
        // absent case — "counters recorded as zero" and "no counters on record" are different facts.
        assert_eq!(
            HealthCounters::decode(&[0u8; 24]).unwrap(),
            HealthCounters::default()
        );

        // All three records share one store, so their keys must all differ.
        assert_ne!(HEALTH_COUNTERS_KEY, RESET_MARKER_KEY);
        assert_ne!(HEALTH_COUNTERS_KEY, HALT_RECORD_KEY);
        assert_ne!(LEGACY_BODY_SCAN_KEY, RESET_MARKER_KEY);
        assert_ne!(LEGACY_BODY_SCAN_KEY, HALT_RECORD_KEY);
        assert_ne!(LEGACY_BODY_SCAN_KEY, HEALTH_COUNTERS_KEY);
    }
}
