//! The ordered, windowed catch-up a node runs after restoring at an anchor below its peers' tip (C259).
//!
//! **Why this exists.** Every block request in this protocol is keyed by hash, so a node that has just
//! restored at an anchor can only learn the gap by walking *downward* from a peer's tip through
//! `justifications`. Each new block justifies blocks inside the gap, so that walk pulls the whole gap
//! in at once — and `MAX_PENDING_BLOCKS` then turns progress into drops. Measured 2026-10-10 on a
//! four-validator devnet: `block receiver state full (1024 blocks); dropping` 173 times, **one** block
//! validated, the node frozen at height 121 while the master ran to 577
//! (`spec/audit/evidence/n-anchor-drill/run-2-catchup-stall.txt`).
//!
//! **What the anchor has to do with it, precisely.** The freeze is a function of the *size of the
//! gap*, not of the anchor: an anchor is how a gap becomes large. An ordinary joiner syncs to a
//! peer's latest finalised fringe, which is within a few dozen heights of the tip, so its gap is tens
//! of blocks and never approaches the bound — while a node restored at a meet in a net whose finality
//! has frozen has an anchor hundreds of heights below the tip on purpose, and that is the state this
//! exists for. (An ordinary joiner measured on the same rig does share the *restore's* cost — the LFS
//! walk hydrates the state root of every block from its root down to genesis, O(chain) either way —
//! which is a separate cost, recorded and not fixed here.)
//!
//! **What this does instead.** Walk the gap *upward* in windows: ask the peer for the heights
//! `[H+1, H+W]` (`BlockRangeRequest`), ask for those blocks by hash, and only ask for the next window
//! once the node's own frontier has reached the last one. Two things make it hold:
//!
//! - the answer is **topological** (ascending height, parent before child), and the requests go out in
//!   that order, so a window's blocks validate as they arrive instead of piling up;
//! - the [`CatchupWindow`] the driver holds narrows the ingest to `frontier + W`, so the gossip that
//!   keeps arriving — the tip's hashes, whose justifications point into the gap — cannot refill the
//!   pending set behind the walk. Blocks above the window are left **un-acked**, not refused: the
//!   retriever re-requests them once the frontier has moved past, which is what makes the window a
//!   pace rather than a loss.
//!
//! Nothing above the anchor is *installed*: every block the walk delivers goes through the ordinary
//! validation path, so only the anchor's state is adopted — #287's first invariant.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rchain_block_storage::dag::dag_storage::{BlockDagStorage, ResetMarker, ResetPhase};
use rchain_comm::peer_node::PeerNode;
use rchain_comm::rp::rp_conf::RPConf;
use rchain_comm::transport::transport_layer::TransportLayer;
use rchain_models::casper::protocol::casper_message::{BlockRange, BlockRangeRequest};
use rchain_shared::log::{Log, LogSource};

use crate::engine::node_running::{MAX_BLOCK_RANGE, MAX_PENDING_BLOCKS};
use crate::protocol::comm_util::{CommUtil, ConnectionsCell};

/// How many heights one window covers. A four-validator shard holds at most four blocks per height, so
/// one window is at most 32 blocks — far below `MAX_PENDING_BLOCKS`, which is the whole point: the
/// pending set has to hold a window, never a gap. See [`MAX_SHARD_WIDTH`] for where that stops being
/// true.
pub const WINDOW_HEIGHTS: i64 = 8;

/// **The stated bound (Law 71): the widest shard one window may span before the pending bound, not the
/// window, is what decides the node's fate.**
///
/// Law 71 says a catch-up window bounds what a joiner holds, independently of the tip. It bounds
/// *heights*; the bound it has to stay under — `MAX_PENDING_BLOCKS` — counts *blocks*. The two are
/// related by the shard's width: a shard produces at most one block per bonded validator per height, so
/// one window pends at most `WINDOW_HEIGHTS × width` blocks. At a width of four (the rig every
/// measurement here ran on) that is 32 of 1024; the relation turns over at
/// `MAX_PENDING_BLOCKS ÷ WINDOW_HEIGHTS` = **128 bonded validators**, and past it a single window alone
/// can saturate the pending set — which is the drop path C267 measured, arriving by a different route.
///
/// **Why it is a constant and not only a comment.** The width is a property of the *shard*, not of this
/// crate, so it cannot be asserted where the windows are built; naming the turnover is what lets the
/// driver compare itself against it (below) and what turns "far below the bound" into a number a
/// reviewer can check.
pub const MAX_SHARD_WIDTH: usize = MAX_PENDING_BLOCKS / WINDOW_HEIGHTS as usize;

// **The coupling that *is* known here, and the drift it catches.** The responder clamps a
// `BlockRangeRequest` to `MAX_BLOCK_RANGE` (`node_running.rs`), so a window wider than the responder's
// range leaves the walk waiting — in `wait_for_frontier` — for heights nobody was ever asked for. That
// is a self-inflicted stall of exactly the shape C267 measured, and it is one constant edit away: raise
// `WINDOW_HEIGHTS` alone and it fails the build rather than the network.
const _: () = assert!(
    (WINDOW_HEIGHTS as usize) <= MAX_BLOCK_RANGE,
    "WINDOW_HEIGHTS must not exceed MAX_BLOCK_RANGE: the responder clamps to the range, so a wider \
     window makes the walk wait for heights it never asked for"
);

/// How long one window may take before the walk gives up and hands the node back to the ordinary
/// ingest. Bounded so a silent or stalling peer cannot hold the gate shut — the node is never *worse*
/// off than before the catch-up, which is the property that makes this safe to run automatically.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(45);

/// The most windows one catch-up will walk, so a peer answering `[H+1, H+W]` forever cannot keep the
/// walk (and the gate) alive without bound.
const MAX_WINDOWS: u32 = 4096;

/// How often the driver looks at its own frontier while a window is in flight.
const FRONTIER_POLL: Duration = Duration::from_millis(200);

/// The temporary ingest window a node holds while it catches up.
///
/// **Released is the ordinary node.** `top` is `i64::MAX` unless a driver is walking, so an
/// unattended node admits everything exactly as it did before this module existed, and the gate can
/// only ever be as narrow as a catch-up makes it.
pub struct CatchupWindow {
    top: AtomicI64,
    width: i64,
}

impl CatchupWindow {
    pub fn new(width: i64) -> Self {
        CatchupWindow {
            top: AtomicI64::new(i64::MAX),
            width,
        }
    }

    /// **The one question the ingest asks**: may a block at this height be pended *now*?
    ///
    /// A block above the window is one the walk has not reached yet, so pending it would be holding a
    /// block whose parents are not validated — the shape that fills `MAX_PENDING_BLOCKS`. It is left
    /// for later rather than refused (the caller does not ack it), so nothing is lost.
    pub fn admits(&self, height: i64) -> bool {
        height <= self.top.load(Ordering::Relaxed)
    }

    /// Whether a walk is holding the window shut. For logs and tests.
    pub fn is_active(&self) -> bool {
        self.top.load(Ordering::Relaxed) != i64::MAX
    }

    /// Narrow the ingest to `frontier + width`. `pub(crate)` only so the ingest's own test can hold a
    /// window without a driver; the walk in this module is the only production caller.
    pub(crate) fn hold(&self, frontier: i64) {
        self.top
            .store(frontier.saturating_add(self.width), Ordering::Relaxed);
    }

    /// Hand the node back: every height is admitted again.
    pub(crate) fn release(&self) {
        self.top.store(i64::MAX, Ordering::Relaxed);
    }
}

/// Walk the gap above this node's frontier, in windows, against `peer`.
///
/// Returns the height the walk reached (the frontier it saw last), which is what the caller logs. The
/// window is released on **every** exit path — a catch-up that cannot finish must not leave the node
/// with a narrowed ingest.
#[allow(clippy::too_many_arguments)]
/// **Law 72b's release condition.** A reset marker may be released only from the completed phase and
/// only once the catch-up frontier has passed its anchor. A `Rewinding` record brackets effects that
/// may be partially applied; clearing it would erase the record before the boot repair re-applies them.
fn may_release(marker: &ResetMarker, frontier: i64) -> bool {
    marker.phase == ResetPhase::Rewound && frontier > i64::from(marker.height)
}

pub async fn run_catchup(
    transport: Arc<dyn TransportLayer>,
    conf: RPConf,
    connections: ConnectionsCell,
    dag: Arc<dyn BlockDagStorage>,
    log: Arc<dyn Log>,
    window: Arc<CatchupWindow>,
    // The one wake a completed self-replay reset needs (#564, F-U23-11).
    resume: Arc<dyn crate::engine::reset::SelfReplayResume>,
    peer: PeerNode,
    mut answers: tokio::sync::mpsc::Receiver<BlockRange>,
) -> i64 {
    let source = LogSource::new("casper.engine.Catchup");
    let comm = CommUtil::new(transport, conf, connections, log.clone());
    let mut frontier = frontier_of(dag.as_ref()).await;
    window.hold(frontier);
    log.info(
        source,
        &format!(
            "catch-up against {}: this node's frontier is height {}, walking outward in windows of {} \
             heights (nothing above the anchor is installed — every window is validated)",
            peer.endpoint.host, frontier, window.width
        ),
    );
    // **The shard check (Law 71).** The window's whole claim is that one window's blocks fit inside the
    // pending bound, and that is a relation between `WINDOW_HEIGHTS` and the *shard's* width — which no
    // compile-time constant can carry, so it is read here, off the frontier this node is holding. The
    // check does not stop the walk: a catch-up that refuses to run leaves the node with no recovery at
    // all, while one that runs too wide is no worse than the ingest was before it. What it does is name
    // the state loudly, so a shard past the turnover is a line in the log rather than drops C267 taught
    // us to read backwards.
    let shard_width = dag.get_representation().await.latest_fringe().len();
    if shard_width > MAX_SHARD_WIDTH {
        log.warn(
            source,
            &format!(
                "[catchup-window-too-wide] this node's frontier is {shard_width} block(s) wide, over \
                 the {MAX_SHARD_WIDTH} a {WINDOW_HEIGHTS}-height window can pace within the \
                 {MAX_PENDING_BLOCKS}-block pending bound: one window alone may saturate it, and the \
                 ingest's drop path binds before this walk's pace does"
            ),
        );
    }
    let mut walked: u32 = 0;
    loop {
        if walked >= MAX_WINDOWS {
            log.warn(
                source,
                &format!("catch-up stopped after {MAX_WINDOWS} windows at height {frontier}"),
            );
            break;
        }
        walked += 1;
        let from = frontier.saturating_add(1);
        let req = BlockRangeRequest {
            from,
            to: from.saturating_add(window.width - 1),
        };
        comm.request_block_range(&peer, &req).await;
        let answer = match tokio::time::timeout(WINDOW_TIMEOUT, answers.recv()).await {
            Ok(Some(answer)) => answer,
            Ok(None) => break,
            Err(_) => {
                // A silent peer is the end of the walk, not a failure of the node: the gate opens and
                // the ordinary ingest resumes exactly where it was.
                log.warn(
                    source,
                    &format!("no window arrived for heights {from}..={} in {WINDOW_TIMEOUT:?}: stopping the catch-up at height {frontier}", req.to),
                );
                break;
            }
        };
        log.info(
            source,
            &format!(
                "window {}..={} arrived with {} hash(es), peer tip {}",
                answer.from,
                answer.to,
                answer.hashes.len(),
                answer.tip
            ),
        );
        // **An empty window is the end of the gap**, and it is also what this node gets from a peer
        // that does not know the message — the same degraded exchange the anchored fringe request
        // gets, and the reason the message is a non-breaking addition.
        if answer.hashes.is_empty() {
            log.info(
                source,
                &format!("the peer has nothing above height {frontier}; catch-up finished"),
            );
            break;
        }
        // **The peer's own frontier ends the walk too, and it is the case that matters.** A peer that
        // keeps producing — which is what a frozen-finality net's survivors do, proposing above the
        // meet — closes every window it is asked for, so a walk that only stopped on an empty answer
        // would chase a moving tip and hold the ingest window shut for as long as the peer produced
        // (up to `MAX_WINDOWS`). `tip` is 0 when the responder did not say, and then the walk falls
        // back to its own bounds.
        if answer.tip > 0 && answer.tip <= frontier {
            log.info(
                source,
                &format!(
                    "this node's frontier ({frontier}) has reached the peer's tip ({}); catch-up finished",
                    answer.tip
                ),
            );
            break;
        }
        for hash in &answer.hashes {
            comm.request_for_block(&peer, hash).await;
        }
        // Ask for the window, then let it validate: the next window starts only once this node's own
        // frontier has reached the height this one covered — or the peer's tip, if that is nearer.
        let target = if answer.tip > 0 {
            answer.to.min(answer.tip)
        } else {
            answer.to
        };
        let reached = wait_for_frontier(dag.as_ref(), target, frontier).await;
        frontier = reached;
        window.hold(frontier);
    }
    window.release();
    log.info(
        source,
        &format!("catch-up released the ingest window at height {frontier}"),
    );
    // **A reset's marker is cleared once this node is past the anchor it rewound to** (F-U5-01's crash
    // window, register C274). The condition is the walk's own result and not its exit reason: a walk
    // that ended with nothing above the anchor — a silent peer, a closed door — leaves the marker, so
    // the *next* boot re-enters the guard rather than reconnecting to a truncated DAG. Clearing on the
    // walk's mere completion would re-open exactly the window the marker exists to close.
    match dag.read_reset_marker().await {
        // **Law 72b: a record is released only from the completed phase.** A `Rewinding` marker
        // brackets effects that may be partially applied; clearing it here would erase the record
        // before the boot repair has re-applied them. It is kept, and the next boot re-enters the
        // guard (which repairs rather than re-rewinds).
        Ok(Some(marker)) if marker.phase == ResetPhase::Rewinding => log.warn(
            source,
            &format!(
                "the reset marker at height {} is still `Rewinding` — a partial reset was recorded; \
                 keeping it so the next boot repairs the effects rather than clearing a record that \
                 brackets them (Law 72b)",
                i64::from(marker.height)
            ),
        ),
        Ok(Some(marker)) if may_release(&marker, frontier) => {
            // **A completed self-replay reset is resumed here** (#564, F-U23-11): the walk's own
            // result is the only evidence that this node is past the anchor it rewound to, and the
            // record names whether the reset was this fault's. The helper is store-first and fails
            // closed; a marker from any other reset leaves the predicate false and the latch alone.
            crate::engine::reset::resume_self_replay_after_catchup(
                dag.as_ref(),
                &marker,
                frontier,
                resume.as_ref(),
                &log,
            )
            .await;
            match dag.clear_reset_marker().await {
                Ok(()) => log.info(
                    source,
                    &format!(
                        "this node has caught up past the anchor it rewound to (height {}); clearing the \
                         reset marker so the next boot is an ordinary one (#320, C274)",
                        i64::from(marker.height)
                    ),
                ),
                Err(e) => log.warn(
                    source,
                    &format!(
                        "caught up past the reset anchor (height {}) but could not clear the marker: \
                         {e} — the next boot will re-enter the reset guard, which repairs rather than \
                         re-rewinds",
                        i64::from(marker.height)
                    ),
                ),
            }
        }
        Ok(Some(marker)) => log.info(
            source,
            &format!(
                "still at or below the anchor this node rewound to (height {}, frontier {frontier}): \
                 keeping the reset marker, so the next boot resumes the repair rather than reconnecting \
                 to a truncated DAG",
                i64::from(marker.height)
            ),
        ),
        Ok(None) => {}
        Err(e) => log.warn(
            source,
            &format!("could not read the reset marker while finishing the catch-up: {e}"),
        ),
    }
    frontier
}

/// The node's own frontier: the highest height in its DAG. `latest_block_number` is one past it, and
/// an empty DAG reads as -1 so a caller can always say "the next height is frontier + 1".
async fn frontier_of(dag: &dyn BlockDagStorage) -> i64 {
    let repr = dag.get_representation().await;
    repr.latest_block_number() - 1
}

/// Wait until the DAG has reached `target`, bounded by [`WINDOW_TIMEOUT`]; returns the frontier it
/// saw. A window that never arrives in time yields the frontier as it stands — the walk then asks
/// again for the heights it still lacks, which is the pace, not a failure.
async fn wait_for_frontier(dag: &dyn BlockDagStorage, target: i64, current: i64) -> i64 {
    let deadline = tokio::time::Instant::now() + WINDOW_TIMEOUT;
    let mut frontier = current;
    while frontier < target && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(FRONTIER_POLL).await;
        frontier = frontier_of(dag).await;
    }
    frontier
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Released, a window admits everything** — an ordinary node's ingest must be exactly what it
    /// was before the driver existed, or this would be a new bound on every node rather than a pace
    /// for one that is catching up.
    #[test]
    fn a_released_window_admits_every_height() {
        let w = CatchupWindow::new(WINDOW_HEIGHTS);
        assert!(!w.is_active());
        for height in [0, 1, 1_000_000, i64::MAX - 1] {
            assert!(
                w.admits(height),
                "height {height} was refused by a released window"
            );
        }
    }

    /// **Law 72b: release only from the completed phase.** A `Rewinding` marker may bracket effects
    /// that are only partially applied; clearing it would erase the record before the boot repair.
    #[test]
    fn a_rewinding_marker_is_never_released() {
        let marker = ResetMarker {
            anchor: rchain_models::block_hash::BlockHash::new([1; 32]),
            height: rchain_shared::refined::BlockHeight::zero(),
            phase: ResetPhase::Rewinding,
        };
        assert!(
            !may_release(&marker, 100),
            "a partial reset is never released, however far the frontier has advanced"
        );
        let rewound = ResetMarker {
            phase: ResetPhase::Rewound,
            ..marker
        };
        assert!(may_release(&rewound, 100), "past the anchor releases");
        assert!(
            !may_release(&rewound, 0),
            "at or below the anchor keeps the marker"
        );
    }

    /// Held at a frontier, a window admits up to `frontier + width` and nothing above — and it admits
    /// blocks *below* the frontier too (a block that arrives late, or an ancestor, is not what fills
    /// the pending set).
    #[test]
    fn a_held_window_admits_the_window_and_not_the_gap_above_it() {
        let w = CatchupWindow::new(WINDOW_HEIGHTS);
        w.hold(100);
        assert!(w.is_active());
        assert!(w.admits(100), "the frontier itself");
        assert!(w.admits(108), "the top of the window");
        assert!(!w.admits(109), "one past the window is the gap");
        assert!(
            !w.admits(577),
            "the tip is the gap while the frontier is at 100"
        );
        assert!(w.admits(0), "an ancestor is always admissible");
    }

    /// The window advances with the frontier, and releasing it restores the ordinary ingest.
    #[test]
    fn the_window_advances_with_the_frontier_and_release_restores_it() {
        let w = CatchupWindow::new(WINDOW_HEIGHTS);
        w.hold(100);
        assert!(!w.admits(109));
        w.hold(108);
        assert!(w.admits(116), "the window moved with the frontier");
        w.release();
        assert!(!w.is_active());
        assert!(w.admits(i64::MAX - 1));
    }

    /// A frontier at the top of the range saturates rather than wrapping: the width is added with
    /// `saturating_add`, so the only thing that can happen at the top is a *wider* window, and there is
    /// no height above `i64::MAX` to admit anyway. An overflow that wrapped would turn the window
    /// negative and refuse everything, which is the failure this pins.
    #[test]
    fn a_frontier_near_the_top_of_the_range_saturates_rather_than_wrapping() {
        let w = CatchupWindow::new(WINDOW_HEIGHTS);
        w.hold(i64::MAX - 2);
        assert!(
            w.admits(i64::MAX - 2),
            "the frontier itself is always admitted"
        );
        assert!(w.admits(i64::MAX), "saturating, not wrapping");
    }

    /// **Law 71's bound, as arithmetic a reviewer can check rather than a claim in a comment.**
    ///
    /// The window pends at most `WINDOW_HEIGHTS × shard width` blocks and the pending bound is
    /// `MAX_PENDING_BLOCKS`, so `MAX_SHARD_WIDTH` is *defined* by the relation — and the two facts worth
    /// pinning are that it is at least the width the rigs here actually run at (four bonded validators,
    /// and thirty-two, and sixty-four), and that it is the **largest** width for which the relation
    /// holds. The second is what stops the constant drifting into decorative: one validator wider and a
    /// single window alone can saturate the pending set.
    #[test]
    fn the_window_fits_the_pending_bound_up_to_the_stated_shard_width() {
        for width in [1usize, 4, 32, 64, MAX_SHARD_WIDTH] {
            assert!(
                (WINDOW_HEIGHTS as usize) * width <= MAX_PENDING_BLOCKS,
                "a {width}-wide shard pends {} blocks in one window, over the {MAX_PENDING_BLOCKS} the \
                 ingest will hold",
                (WINDOW_HEIGHTS as usize) * width
            );
        }
        assert!(
            (WINDOW_HEIGHTS as usize) * (MAX_SHARD_WIDTH + 1) > MAX_PENDING_BLOCKS,
            "MAX_SHARD_WIDTH is not the largest width the window fits: {} is over the bound too",
            (WINDOW_HEIGHTS as usize) * (MAX_SHARD_WIDTH + 1)
        );
        assert_eq!(
            MAX_SHARD_WIDTH, 128,
            "the turnover is 128 bonded validators at a window of 8 heights and a 1024-block pending \
             bound; a change here changes what the driver's shard warning means"
        );
    }
}
