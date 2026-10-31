//! The reset's durable half — what a node rewinding to its last finalised block must do to *two* stores
//! (#320, F-U5-01's acting half).
//!
//! **Why this is a function of its own.** The reset's other halves are process state (the runtime's root,
//! the catch-up walk) and they live where their handles do; this half is two store operations whose
//! effects outlive the process, and it is the half a test can reach without a `RuntimeManager`. Splitting
//! it out is what turns "the loop looks right" into an assertion about the stores — which is the same
//! moved `drop_above`'s own test makes for the DAG alone ("a rewind that lived only in memory would be
//! undone by the next boot").
//!
//! **And the two operations are one meaning.** `drop_above` rewinds the DAG and deliberately leaves
//! `BlockStore` alone — it does not own it, and its own doc says so. Leaving the bodies is not harmless:
//!
//! * the node keeps **serving** them (`node_running::block_is_known` asks the store, so a peer
//!   requesting a dropped block gets it — a node handing out blocks it can no longer replay);
//! * and its **receiver re-inserts** them. `block_receiver::not_validated` reads a body that is stored
//!   but absent from the DAG as *"stored but not yet validated"*, adds it to `parents_to_validate`, and
//!   `send_to_validate` puts it back on the incoming queue — so the next child of a dropped block
//!   arriving would re-insert the dropped block **above the anchor** and undo the reset entirely.
//!
//! So the bodies go with the metadata, and the rewound state becomes unrepresentable rather than guarded:
//! there is nothing left to serve or to re-insert, and everything above the anchor is re-fetched from
//! peers, which is the whole of what a rewind means.
//!
//! **And the effects are bracketed, because unrepresentable is not the same as atomic** (register
//! C274's second item; law 72, `spec/Rchain/Durability.lean`). A process that dies *between* them leaves
//! bodies the DAG does not hold — the very set the re-insertion arm re-injects — so the node records the
//! intent durably before the first operation and the completion after the **last**, and
//! [`resume_after_reset`] is the boot guard that reads it.
//!
//! **The bracket covers three durable effects, not two.** The DAG rewind and the body purge are the two
//! `rewind_and_purge` performs, and the runtime's root is the third: moving it is a durable
//! `CURRENT_ROOT` write (`HistoryRepository::reset` → `validate_and_set_current_root`). Writing the
//! completion before that write returns is the state **law 72** forbids — a `rewound` record with the
//! root unmoved, which the guard reads as "the stores agree" and repairs nothing, exactly the shape
//! `reset_volatile`'s own doc names for the merge. So the two marker writes live in the callers, around
//! all three effects, and the guard's `Rewinding` branch re-applies all three.
//!
//! Nothing here is best-effort: a reset that cannot record itself does not run, which is why the
//! marker's write refuses on a storage that has no store for it.

use std::sync::Arc;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::dag_storage::{BlockDagStorage, ResetMarker, ResetPhase};
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::BlockMessage;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::refined::BlockHeight;

use crate::interpreter_util::ResetRequest;
use crate::runtime_manager::RuntimeManager;

/// Rewind the DAG to `anchor_height` and remove the bodies of everything it dropped.
///
/// Returns `(blocks rewound, bodies removed)`. **An error is fatal and the caller must not carry on**:
/// `drop_above` deletes the persisted metadata and *then* rebuilds the representation, and the rebuild
/// can refuse (it re-applies the equivocation gate), which leaves a store that has been rewound under a
/// view that has not. A node in that shape is worse than the refusal it replaces, so the caller
/// propagates rather than logging and continuing.
///
/// **This is the destructive half only** — the two store operations, with no record and no runtime.
/// The record that brackets them (and the runtime root that is the third effect) lives in the callers,
/// [`perform_reset`] and [`resume_after_reset`]: the bracket must cover *all three* durable effects, so
/// it cannot be drawn by the function that performs two of them. See law 72
/// (`spec/Rchain/Durability.lean`).
pub async fn rewind_and_purge(
    dag: &Arc<dyn BlockDagStorage>,
    block_store: &BlockStore,
    anchor_height: BlockHeight,
) -> Result<(usize, usize), String> {
    let dropped = dag.drop_above(anchor_height).await.map_err(|e| {
        format!(
            "the reset's rewind failed: {e}. This is fatal on purpose — the store may have been rewound \
             under a view that was not rebuilt, so the node must not keep serving"
        )
    })?;
    let deleted = block_store.delete(&dropped).await?;
    Ok((dropped.len(), deleted))
}

/// **What the boot path does about a reset this node performed and did not finish** (F-U5-01, register
/// C274) — the guard that closes the crash window the bracketed effects open.
///
/// Returns the anchor the caller must hand to the catch-up walk, or `None` when this node has never
/// reset. Three states, and the middle two are different facts:
///
/// * **no marker** (`Ok(None)`) — the ordinary boot. The caller logs "reconnecting" and the node runs on
///   the DAG it has, which is what every build before this one did;
/// * **`Rewinding`** — the reset was intended and may not have completed, so **every** effect it
///   brackets is re-applied: the DAG rewind, the body purge, and the runtime's root. They are
///   idempotent: `drop_above` returns nothing the second time, the body purge is driven by a predicate
///   (see [`purge_orphan_bodies`]) that does not depend on what the rewind returned — which matters,
///   because after a crash *during* the purge the rewind has already happened and returns nothing, so a
///   purge keyed on its return value would delete nothing — and the runtime reset re-sets the same
///   `CURRENT_ROOT`;
/// * **`Rewound`** — every effect returned and the stores agree. **Nothing is re-applied**: the node
///   may have caught up and produced blocks above the anchor in the meantime, and a second `drop_above`
///   would throw those away. The marker is only telling the caller where to catch up from.
///
/// **The repair is total over the record's effects** (law 72, `spec/Rchain/Durability.lean`): the
/// completion is written only once every effect has returned — the rewind, the purge, the runtime root
/// **and the derived-index cleanup** (B39, HAZOP F-U17-11). A `Rewinding` a boot can find is one whose
/// root or indexes may not have moved, and a guard that repaired only the two stores would hand a node
/// to the catch-up walk with the runtime still rooted at the state the rewind dropped, or with a deploy
/// index still telling the proposer that a pooled deploy was already included.
///
/// The marker is **not** cleared here. It is cleared when the node's frontier passes the anchor (the
/// catch-up walk's own end), and that is the honest condition: until then, a crash must land back in
/// this guard rather than in the truncated-DAG state it exists to prevent.
pub async fn resume_after_reset(
    dag: &Arc<dyn BlockDagStorage>,
    block_store: &BlockStore,
    runtime_manager: &Arc<RuntimeManager>,
    log: &Arc<dyn Log>,
    source: LogSource,
) -> Result<Option<BlockHash>, String> {
    let Some(marker) = dag.read_reset_marker().await? else {
        return Ok(None);
    };
    if marker.phase == ResetPhase::Rewinding {
        let dropped = dag.drop_above(marker.height).await.map_err(|e| {
            format!(
                "this node was rewinding to height {} when it stopped, and the rewind could not be \
                 re-applied on this boot: {e}",
                i64::from(marker.height)
            )
        })?;
        let purged = purge_orphan_bodies(dag, block_store).await?;
        let anchor_block = anchor_body(block_store, marker.anchor).await?;
        runtime_manager
            .runtime()
            .reset(Blake2b256Hash::from_byte_array(
                anchor_block.post_state_hash.as_bytes(),
            ))
            .await?;
        // **The fourth effect, before the record says the stores agree** (B39). A crash that left the
        // metadata drop applied and the index cleanup not is exactly the state this call repairs; it
        // derives the stale rows from what survives, so it does not depend on this boot's rewind having
        // dropped anything.
        let (deploy_rows, deployer_rows) = dag.repair_derived_indexes().await?;
        log.warn(
            source,
            &format!(
                "this node was rewinding to {} (height {}) when it stopped, part-way through the \
                 reset's durable effects: re-applied the rewind ({} block(s)), removed {purged} body(ies) \
                 the DAG does not hold, reset the runtime to the anchor's state, and repaired \
                 {deploy_rows} deploy-index and {deployer_rows} deployer-index row(s) — the four effects \
                 the completion record brackets (law 72, #320, C274, B39)",
                marker.anchor.to_hex(),
                i64::from(marker.height),
                dropped.len(),
            ),
        );
        dag.write_reset_marker(ResetMarker {
            phase: ResetPhase::Rewound,
            ..marker
        })
        .await?;
    } else {
        log.warn(
            source,
            &format!(
                "this node rewound to {} (height {}) before it stopped: every effect returned, so \
                 nothing is re-applied, and the catch-up walk is anchored there rather than reconnecting \
                 to a truncated DAG (#320, C274)",
                marker.anchor.to_hex(),
                i64::from(marker.height),
            ),
        );
    }
    Ok(Some(marker.anchor))
}

/// **Which anchor a node boots on when the store's reset record and the operator's `--sync-anchor`
/// disagree** (W-N11-01, C274).
///
/// The record is the state this store holds: the reset truncated the DAG to that anchor and repaired
/// the runtime to it, so it is the only anchor the catch-up walk can honestly start from — and C274
/// made it win for exactly that reason. What C274 left was the *manner*: the operator's named anchor
/// differing from the record's produced a `log.warn` and the node booted on the record anyway. On a
/// store the operator did not create — the wargame's host that "supplies the data directory supplies
/// the restore point" — that is a planted record choosing where the node rewinds, ahead of the
/// operator's stated intent, with nothing but a warning between. Two contradicting anchors are not a
/// state to pick from silently, so this refuses and names both.
///
/// The ways out are the operator's and each is deliberate: drop `--sync-anchor` (or set it to the
/// recorded anchor) to boot on the reset; or clear the reset record, which is an act rather than an
/// override, if the named anchor is what is meant. Vacuously `Ok` for an anchor that agrees and for no
/// anchor at all, so nothing here fires on an ordinary boot.
pub fn boot_anchor_after_reset(
    named: Option<BlockHash>,
    recorded: BlockHash,
) -> Result<BlockHash, String> {
    match named {
        Some(named) if named != recorded => Err(format!(
            "--sync-anchor names {} but this node's store carries a reset record for {}: the record is \
             the state this store actually holds — the reset truncated its DAG to that anchor and \
             repaired its runtime to it — so booting on the named anchor would unwind a completed \
             reset. Nothing was chosen for you. Drop --sync-anchor (or set it to the recorded anchor) \
             to boot on the reset; if the named anchor is what you mean, clear the reset record \
             deliberately, which is an act rather than an override",
            named.to_hex(),
            recorded.to_hex()
        )),
        _ => Ok(recorded),
    }
}

/// Read a block's body from the store, refusing with the reset's own sentence when it is absent.
///
/// **Both halves of the reset need the anchor's body** — the act, to reset the runtime to its state, and
/// the boot guard, to repair a reset whose runtime step may not have run — so the refusal lives here
/// rather than being restated, and the sentence it prints is the one an operator reads to tell a
/// genuinely missing anchor from a store fault.
async fn anchor_body(block_store: &BlockStore, anchor: BlockHash) -> Result<BlockMessage, String> {
    block_store
        .get(&[anchor])
        .await?
        .pop()
        .flatten()
        .ok_or_else(|| {
            format!(
                "the reset's anchor {} is not in this node's block store, so the state to reset the \
                 runtime to cannot be read — refusing the reset rather than rewinding to a block this \
                 node cannot replay from",
                anchor.to_hex()
            )
        })
}

/// Remove every body the DAG does not hold — **the receiver's own re-injection set**, which is what the
/// boot guard has to delete when a reset crashed part-way.
///
/// **Why a predicate and not the rewind's return value.** The ordinary purge deletes the hashes
/// `drop_above` returned, and that is exactly the set a completed rewind drops. A crash *during* the
/// delete leaves a subset of those hashes un-deleted while the rewind is already durable, so a
/// re-applied `drop_above` returns nothing and the returned set is empty — the survivors would go
/// unreaped and the receiver would re-inject them. The predicate is the same fact stated independently
/// of the order: a body whose block the DAG does not hold is a body that can be neither replayed nor
/// served.
///
/// **What it costs, and what it may take with it.** The store's **keys** are enumerated (no body is
/// decoded), so this is O(bodies) in key bytes — and it is paid **only when a marker is present**, a
/// rare and deliberate state, not on any ordinary boot.
/// It may also remove a body that was legitimately in flight (the receiver stores a body before it
/// inserts the block), which is harmless: the reset puts the node behind its peers on purpose and the
/// catch-up walk re-fetches what it needs.
async fn purge_orphan_bodies(
    dag: &Arc<dyn BlockDagStorage>,
    block_store: &BlockStore,
) -> Result<usize, String> {
    let keys = block_store.keys().await?;
    let repr = dag.get_representation().await;
    let orphans: Vec<BlockHash> = keys
        .into_iter()
        .filter(|hash| !repr.contains(hash))
        .collect();
    if orphans.is_empty() {
        return Ok(0);
    }
    block_store.delete(&orphans).await
}

/// **What a running node does when the reset waker fires** (#320) — the running loop's reset arm,
/// extracted so the decision and its effect are testable without a transport, an importer, a
/// `ShardSpec` or a `NodeRunning`.
///
/// Three outcomes, and the middle one is the one an operator will meet first:
///
/// * **nothing pending** (`Ok(None)`) — a spurious wake, or a request another arm already took. The
///   running phase carries on; it does *not* rebuild anything;
/// * **the switch is off** (`Ok(None)`, after logging) — the detector has already counted and logged
///   the divergence, and this line is the operator's evidence that the node *would* have reset itself.
///   That is what makes turning `casper.self-reset` on a decision rather than a leap, and it is why the
///   request is consumed here rather than left pending: a surface that showed a request for ever would
///   be the same silence in a different place;
/// * **acted** (`Ok(Some(request))`) — the durable half ran and the caller has an anchor to catch up
///   from.
///
/// **The caller propagates the error rather than continuing**: a rewind that failed leaves a store
/// rewound under a view that was not, and a node in that shape serving on is worse than the refusal the
/// reset replaces.
pub async fn act_on_reset_request(
    shard_id: &str,
    self_reset: bool,
    dag: &Arc<dyn BlockDagStorage>,
    block_store: &BlockStore,
    runtime_manager: &Arc<RuntimeManager>,
    log: &Arc<dyn Log>,
    source: LogSource,
) -> Result<Option<ResetRequest>, String> {
    // **Peek, do not take** (law 73, `spec/Rchain/Requests.lean`). A request is spent only by the act
    // that commits; taking it first would lose it to a failure that destroyed nothing. *This shard's*
    // request, too: the lane is the key, so a node that is a member of several shards cannot act on
    // another's anchor (§107's D4).
    let Some(request) = crate::interpreter_util::reset_request(shard_id) else {
        return Ok(None);
    };
    if !self_reset {
        // Consumed rather than left pending: a surface that showed a request for ever would be the same
        // silence in a different place, and the switch is a *terminal* configuration, not a transient
        // fault — which is why this arm takes and the pre-flight arm below does not.
        let _ = crate::interpreter_util::take_reset_request(shard_id);
        log.warn(
            source,
            &format!(
                "a reset was requested (rewind to {} at height {}) but `casper.self-reset` is off, so \
                 this node keeps refusing {} and its descendants. Turn it on to let the node reset \
                 itself (#320)",
                request.anchor.to_hex(),
                i64::from(request.anchor_height),
                request.refused.to_hex(),
            ),
        );
        return Ok(None);
    }
    // **The pre-flight read runs before the take** (law 73). A reset whose anchor this node cannot read
    // has destroyed nothing, so the request stays pending — actionable by the detector's next scan — and
    // the node is left in the switch-off state: consistent, and still refusing. Every failure *after*
    // the intent is recorded is fatal instead, because by then a store may have moved.
    if let Err(e) = anchor_body(block_store, request.anchor).await {
        log.warn(
            source,
            &format!(
                "a reset was requested (rewind to {} at height {}) but cannot proceed: {e} — the request \
                 stays pending and the node keeps refusing {} and its descendants (law 73, #320)",
                request.anchor.to_hex(),
                i64::from(request.anchor_height),
                request.refused.to_hex(),
            ),
        );
        return Ok(None);
    }
    perform_reset(dag, block_store, runtime_manager, &request, log, source).await?;
    // The act committed; the request is spent now, and not one step earlier.
    let _ = crate::interpreter_util::take_reset_request(shard_id);
    Ok(Some(request))
}

/// The act itself, in the order the design requires — the anchor's state is read first (an anchor this
/// node cannot replay from is a free refusal), and the two stores and the runtime root move under one
/// completion record (law 72, `spec/Rchain/Durability.lean`).
async fn perform_reset(
    dag: &Arc<dyn BlockDagStorage>,
    block_store: &BlockStore,
    runtime_manager: &Arc<RuntimeManager>,
    request: &ResetRequest,
    log: &Arc<dyn Log>,
    source: LogSource,
) -> Result<(), String> {
    let anchor_block = anchor_body(block_store, request.anchor).await?;
    let mark = |phase| ResetMarker {
        anchor: request.anchor,
        height: request.anchor_height,
        phase,
    };
    // **The intent is durable before anything is destroyed** (law 72). A write that fails stops the
    // reset before it has touched a store, which is the only point at which refusing is free.
    dag.write_reset_marker(mark(ResetPhase::Rewinding)).await?;
    let (rewound, removed) = rewind_and_purge(dag, block_store, request.anchor_height).await?;
    // **The runtime root is inside the bracket** (law 72). The completion record is written only once
    // *every* effect has returned, so a crash here leaves `Rewinding` — which the boot guard repairs —
    // rather than a `Rewound` record claiming the stores agree while `CURRENT_ROOT` still names the
    // state the rewind dropped, which `resume_after_reset` reads and re-applies nothing.
    runtime_manager
        .runtime()
        .reset(Blake2b256Hash::from_byte_array(
            anchor_block.post_state_hash.as_bytes(),
        ))
        .await?;
    dag.write_reset_marker(mark(ResetPhase::Rewound)).await?;
    log.warn(
        source,
        &format!(
            "rewound {rewound} block(s) above height {} — {removed} body(ies) removed from the block \
             store so they cannot be served or re-inserted — and reset the runtime to that state (#320)",
            i64::from(request.anchor_height),
        ),
    );
    Ok(())
}
