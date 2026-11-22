//! **A shard's phase, as its launch decided it** (HAZOP U20/B47, RCA S7/getting-ready).
//!
//! Before this cell there was no phase at all: `NodeLaunch` chose genesis, recovery or reconnection
//! and then constructed `NodeRunning`, and every surface had to reconstruct "is this node serving?"
//! from a set of unrelated booleans. A syncing node, a node whose recovery had not started, and a
//! running node with a faulted pipeline looked different only because a reader happened to combine
//! the right fields.
//!
//! The phase is **set by a shard's launch and only by it**. Each shard owns one cell (created in
//! `setup_shard`, carried in `ShardParts`, and cloned to the API, the proposer, launch and the
//! disposition loops); the disposition loops wait for [`NodePhase::Running`] so a start-up re-offer
//! cannot race a recovery into the live DAG, and a propose that arrives while launch is still
//! deciding waits for the decision ([`NodePhaseCell::wait_until_decided`]) instead of answering a
//! healthy boot with a failure. This is not a consensus fact and is not persisted: a restart begins
//! at [`NodePhase::Starting`].
//!
//! **What it does not yet do, said rather than implied:** `Running` is set after the start-up
//! disposition audit is attempted but the audit's drain is bounded, so a row whose parent is
//! missing can still be `Pending` when a shard first reports running. That row is visible (it is the
//! ledger's record) and the periodic stored-body audit withholds readiness for a sustained
//! store/DAG gap; a constructor that refuses to build `Running` with any unsettled row is
//! deliberately not built (U19/B46). The phase is per shard, but `pipeline_faults`,
//! `proposal_isolated`, `orphan_bodies` and `sync_state` remain process-wide; `/health` says which
//! of its readiness terms is per shard and which is not.

use std::time::Duration;

use tokio::sync::watch;

/// Where a shard is in its start-up lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodePhase {
    /// Launch has not yet chosen a path (genesis, sync, reconnection).
    Starting,
    /// The shard is restoring the last finalized state; it serves no chain and proposes nothing.
    Syncing,
    /// **A node whose own validated tip has not caught up** (Law 121, RC-4). The engine runs — it
    /// receives, validates and backfills — but the shard does not serve or propose: its own
    /// validated tip (`own`) is outside the catch-up window of the peer tip it has observed
    /// (`peer_tip`). `supported` is the quorum-supported height over the node's own validated
    /// messages, kept as a diagnostic coordinate. The phase cannot be `Running` without the window
    /// being satisfied, which is the state the pre-fix launcher allowed (a live, silent freeze at
    /// tip 183 while peers were at 306).
    CatchingUp {
        supported: i64,
        own: i64,
        peer_tip: i64,
    },
    /// Launch reached the running engine: the DAG is the shard's served chain.
    Running,
    /// Launch refused to serve (a terminal sync failure, or a named launch failure).
    Blocked(String),
}

impl NodePhase {
    /// The coarse label `/health` publishes.
    pub fn label(&self) -> &'static str {
        match self {
            NodePhase::Starting => "starting",
            NodePhase::Syncing => "syncing",
            NodePhase::CatchingUp { .. } => "catching-up",
            NodePhase::Running => "running",
            NodePhase::Blocked(_) => "blocked",
        }
    }

    /// **Deploy admission is about the pool, not readiness.** `Syncing` and `Blocked` are the two
    /// phases in which telling a client "accepted" would be a promise the node cannot keep: it is
    /// restoring a chain it is not serving, or it has refused to serve. `Starting` is launch still
    /// deciding — the deploy can be pooled and is included when the node reaches `Running`, or
    /// after a sync; refusing it here would turn a millisecond-wide boot window into a client-visible
    /// 503 for a deploy the node was seconds from accepting (the `deploy_is_processed_into_a_block`
    /// integration test hit exactly that window). `/health`'s `ready` is still false in `Starting`;
    /// admission and readiness answer different questions.
    pub fn admits_work(&self) -> bool {
        !matches!(
            self,
            NodePhase::Syncing | NodePhase::CatchingUp { .. } | NodePhase::Blocked(_)
        )
    }

    /// **The fail-closed phase.** An outstanding durable pipeline fault is reported as `Blocked`
    /// whatever the launch cell says; a settled fault returns the cell's phase. The cell keeps its
    /// single writer; this is a derived surface, not a second writer.
    pub fn effective(&self, outstanding: u64) -> NodePhase {
        if outstanding > 0 {
            NodePhase::Blocked(format!("pipeline fault outstanding: {outstanding}"))
        } else {
            self.clone()
        }
    }
}

/// **One shard's phase cell.** Clone-shared (the `ShardHealth`/`ProposeHealth` pattern): every gate
/// that belongs to the shard holds a clone, and the shard's launch is the only writer. A `watch`
/// channel rather than a mutex + polling: a waiter is woken on the transition and cannot miss one
/// between reading the phase and waiting.
#[derive(Clone, Debug)]
pub struct NodePhaseCell {
    tx: watch::Sender<NodePhase>,
}

impl Default for NodePhaseCell {
    fn default() -> Self {
        Self::new()
    }
}

impl NodePhaseCell {
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(NodePhase::Starting);
        Self { tx }
    }

    pub fn phase(&self) -> NodePhase {
        self.tx.borrow().clone()
    }

    /// The shard's launch moves the phase. Exactly one writer per cell is the invariant; a test may
    /// drive a cell it owns directly.
    pub fn set_phase(&self, next: NodePhase) {
        self.tx.send_replace(next);
    }

    pub fn is_running(&self) -> bool {
        matches!(*self.tx.borrow(), NodePhase::Running)
    }

    pub fn admits_work(&self) -> bool {
        self.phase().admits_work()
    }

    /// Block until the shard reaches the running engine. A cell whose writer is gone never reaches
    /// it; the waiter returns rather than hanging a task that can no longer be served.
    pub async fn wait_until_running(&self) {
        let mut rx = self.tx.subscribe();
        loop {
            if matches!(*rx.borrow_and_update(), NodePhase::Running) {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// **Wait until the shard is live**: the engine is constructed and is either serving
    /// (`Running`) or catching up (`CatchingUp`), both of which receive, validate and backfill. The
    /// disposition loops use this rather than [`Self::wait_until_running`]: a stored body must be
    /// re-offered while the node is closing its gap, or the gap cannot close (RC-4 item A/C,
    /// 2026-11-21). Returns `false` when the shard is `Blocked` — the fail-closed phase is terminal,
    /// and a loop that swept into it would be servicing a chain the node has refused.
    pub async fn wait_until_live(&self) -> bool {
        let mut rx = self.tx.subscribe();
        loop {
            match &*rx.borrow_and_update() {
                NodePhase::Running | NodePhase::CatchingUp { .. } => return true,
                NodePhase::Blocked(_) => return false,
                NodePhase::Starting | NodePhase::Syncing => {}
            }
            if rx.changed().await.is_err() {
                return false;
            }
        }
    }

    /// How long a propose may wait for launch to decide before it is answered "not ready".
    ///
    /// The proposer stream starts **beside** `node_launch`, not after it (`setup_shard_runtime`
    /// spawns launch and then constructs the stream), so a propose that arrives while a genesis
    /// master is still in its boot repair sees `Starting`. Refusing immediately turned a healthy
    /// boot into a failure for the client; waiting for ever would turn a hung launch into a hung
    /// propose.
    pub const PHASE_DECISION_WAIT: Duration = Duration::from_secs(120);

    /// Wait for launch to leave `Starting` (or for the bounded window to close) and return the
    /// phase a propose should be judged against.
    pub async fn wait_until_decided(&self) -> NodePhase {
        self.wait_until_decided_within(Self::PHASE_DECISION_WAIT)
            .await
    }

    /// The bounded form, public for tests so a decision-wait test does not take two minutes.
    pub async fn wait_until_decided_within(&self, wait: Duration) -> NodePhase {
        let deadline = tokio::time::Instant::now() + wait;
        let mut rx = self.tx.subscribe();
        loop {
            let current = rx.borrow_and_update().clone();
            if !matches!(current, NodePhase::Starting) {
                return current;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return current;
            }
            match tokio::time::timeout(remaining, rx.changed()).await {
                Ok(Ok(())) => continue,
                Ok(Err(_)) | Err(_) => return self.phase(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_cells_hold_independent_phases() {
        let first = NodePhaseCell::new();
        let second = NodePhaseCell::new();
        first.set_phase(NodePhase::Running);
        assert!(first.is_running());
        assert!(
            !second.is_running(),
            "a sibling shard must not inherit Running"
        );
        second.set_phase(NodePhase::Syncing);
        assert!(first.is_running(), "and the sibling must not take it down");
        assert_eq!(second.phase(), NodePhase::Syncing);
    }

    #[test]
    fn a_starting_cell_admits_work_but_is_not_running() {
        let cell = NodePhaseCell::new();
        assert!(cell.admits_work());
        assert!(!cell.is_running());
        cell.set_phase(NodePhase::Blocked("terminal sync".to_string()));
        assert!(!cell.admits_work());
        assert_eq!(cell.phase().label(), "blocked");
    }

    /// **A catching-up shard is live and serves nothing** (Law 121, RC-4). It names the height a
    /// reader compares with the peer tip, refuses work, and is not `Running` — the pre-fix state
    /// reported `running`/ready at tip 183 against peers at 306.
    #[test]
    fn a_catching_up_shard_names_its_coordinate_and_serves_nothing() {
        let phase = NodePhase::CatchingUp {
            supported: 198,
            own: 198,
            peer_tip: 335,
        };
        assert_eq!(phase.label(), "catching-up");
        assert!(!phase.admits_work());
        assert!(!matches!(phase.effective(0), NodePhase::Running));
        let cell = NodePhaseCell::new();
        cell.set_phase(phase);
        assert!(!cell.is_running());
        assert!(!cell.admits_work());
    }

    #[test]
    fn an_outstanding_pipeline_fault_reports_blocked() {
        let running = NodePhase::Running;
        assert_eq!(running.effective(1).label(), "blocked");
        assert_eq!(running.effective(0), NodePhase::Running);
        assert_eq!(NodePhase::Syncing.effective(2).label(), "blocked");
    }

    /// **A catching-up shard is live for the disposition loops** (RC-4 item A/C). It must receive,
    /// validate and re-offer stored bodies while it closes the gap; a `Blocked` shard is not live.
    #[tokio::test]
    async fn a_catching_up_shard_is_live_and_a_blocked_one_is_not() {
        let cell = NodePhaseCell::new();
        let writer = cell.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            writer.set_phase(NodePhase::CatchingUp {
                supported: 198,
                own: 198,
                peer_tip: 335,
            });
        });
        assert!(
            tokio::time::timeout(Duration::from_secs(2), cell.wait_until_live())
                .await
                .expect("a catching-up cell is live")
        );
        let blocked = NodePhaseCell::new();
        blocked.set_phase(NodePhase::Blocked("terminal".to_string()));
        assert!(
            !blocked.wait_until_live().await,
            "a blocked shard is not live"
        );
    }

    #[tokio::test]
    async fn set_phase_wakes_a_decided_waiter() {
        let cell = NodePhaseCell::new();
        let writer = cell.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            writer.set_phase(NodePhase::Running);
        });
        let decided = tokio::time::timeout(
            Duration::from_secs(2),
            cell.wait_until_decided_within(Duration::from_secs(1)),
        )
        .await
        .expect("the waiter is woken by the transition");
        assert_eq!(decided, NodePhase::Running);
    }
}
