//! **The sync census** (C339 performance): counters for the state-sync path, so a slow or stalled
//! restore is diagnosable from the node instead of only from its journal.
//!
//! Why a census and not a log. The 2026-11-14 campaign could say "eight 30-second windows" only by
//! grepping the requester's warnings; pages requested vs received, bytes, in-flight work, retries and
//! replay cost had no surface at all. These are process-global relaxed atomics: they are evidence, not
//! a gate, and a missing increment must never change what the walk accepts.
//!
//! The counters are cumulative since process start; `in_flight_pages` is a gauge (the walk is
//! concurrent, so the reader may see it change between two loads). The snapshot is serialized on the
//! metrics route as `sync_*` keys.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

/// One process's sync counters.
#[derive(Default)]
pub struct SyncCensus {
    pub pages_requested: AtomicU64,
    pub pages_received: AtomicU64,
    pub page_bytes_received: AtomicU64,
    pub pages_served: AtomicU64,
    pub page_bytes_served: AtomicU64,
    pub page_serve_nanos: AtomicU64,
    pub in_flight_pages: AtomicI64,
    pub block_requests: AtomicU64,
    pub blocks_received: AtomicU64,
    pub resends: AtomicU64,
    pub state_roots_requested: AtomicU64,
    pub replay_blocks: AtomicU64,
    pub replay_nanos: AtomicU64,
}

impl SyncCensus {
    pub fn add_pages_requested(&self, n: u64) {
        self.pages_requested.fetch_add(n, Ordering::Relaxed);
        self.in_flight_pages.fetch_add(n as i64, Ordering::Relaxed);
    }

    pub fn add_pages_received(&self, n: u64, bytes: u64) {
        self.pages_received.fetch_add(n, Ordering::Relaxed);
        self.page_bytes_received.fetch_add(bytes, Ordering::Relaxed);
        self.in_flight_pages.fetch_sub(n as i64, Ordering::Relaxed);
    }

    pub fn add_pages_served(&self, n: u64, bytes: u64, nanos: u64) {
        self.pages_served.fetch_add(n, Ordering::Relaxed);
        self.page_bytes_served.fetch_add(bytes, Ordering::Relaxed);
        self.page_serve_nanos.fetch_add(nanos, Ordering::Relaxed);
    }

    pub fn add_block_requests(&self, n: u64) {
        self.block_requests.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_blocks_received(&self, n: u64) {
        self.blocks_received.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_resend(&self) {
        self.resends.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_state_roots(&self, n: u64) {
        self.state_roots_requested.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_replay(&self, blocks: u64, nanos: u64) {
        self.replay_blocks.fetch_add(blocks, Ordering::Relaxed);
        self.replay_nanos.fetch_add(nanos, Ordering::Relaxed);
    }

    /// A JSON-able reading. Field names are the metrics keys (`sync_pages_requested`, …).
    pub fn snapshot(&self) -> SyncCensusSnapshot {
        let load = |v: &AtomicU64| v.load(Ordering::Relaxed);
        SyncCensusSnapshot {
            sync_pages_requested: load(&self.pages_requested),
            sync_pages_received: load(&self.pages_received),
            sync_page_bytes_received: load(&self.page_bytes_received),
            sync_pages_served: load(&self.pages_served),
            sync_page_bytes_served: load(&self.page_bytes_served),
            sync_page_serve_nanos: load(&self.page_serve_nanos),
            sync_in_flight_pages: self.in_flight_pages.load(Ordering::Relaxed),
            sync_block_requests: load(&self.block_requests),
            sync_blocks_received: load(&self.blocks_received),
            sync_resends: load(&self.resends),
            sync_state_roots_requested: load(&self.state_roots_requested),
            sync_replay_blocks: load(&self.replay_blocks),
            sync_replay_nanos: load(&self.replay_nanos),
        }
    }
}

/// The process's census. A `static` rather than a threaded parameter for the same reason the sync
/// status is: the writers are deep in the walk and the responder, and a reading that is a *global*
/// fact about this process should not be re-plumbed through every call site to be read once.
pub fn census() -> &'static SyncCensus {
    static CENSUS: SyncCensus = SyncCensus {
        pages_requested: AtomicU64::new(0),
        pages_received: AtomicU64::new(0),
        page_bytes_received: AtomicU64::new(0),
        pages_served: AtomicU64::new(0),
        page_bytes_served: AtomicU64::new(0),
        page_serve_nanos: AtomicU64::new(0),
        in_flight_pages: AtomicI64::new(0),
        block_requests: AtomicU64::new(0),
        blocks_received: AtomicU64::new(0),
        resends: AtomicU64::new(0),
        state_roots_requested: AtomicU64::new(0),
        replay_blocks: AtomicU64::new(0),
        replay_nanos: AtomicU64::new(0),
    };
    &CENSUS
}

/// The serialized reading; `serde` derives the camelCase names the `/metrics` route and the status
/// surface both use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncCensusSnapshot {
    pub sync_pages_requested: u64,
    pub sync_pages_received: u64,
    pub sync_page_bytes_received: u64,
    pub sync_pages_served: u64,
    pub sync_page_bytes_served: u64,
    pub sync_page_serve_nanos: u64,
    pub sync_in_flight_pages: i64,
    pub sync_block_requests: u64,
    pub sync_blocks_received: u64,
    pub sync_resends: u64,
    pub sync_state_roots_requested: u64,
    pub sync_replay_blocks: u64,
    pub sync_replay_nanos: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gauge moves in both directions and a snapshot carries every counter.
    #[test]
    fn a_census_snapshot_names_every_counter() {
        let c = SyncCensus::default();
        c.add_pages_requested(3);
        c.add_pages_received(2, 100);
        c.add_pages_served(1, 50, 7);
        c.add_block_requests(9);
        c.add_blocks_received(4);
        c.add_resend();
        c.add_state_roots(2);
        c.add_replay(10, 20);
        let s = c.snapshot();
        assert_eq!(s.sync_pages_requested, 3);
        assert_eq!(s.sync_pages_received, 2);
        assert_eq!(s.sync_page_bytes_received, 100);
        assert_eq!(s.sync_in_flight_pages, 1, "requested minus received");
        assert_eq!(s.sync_pages_served, 1);
        assert_eq!(s.sync_page_bytes_served, 50);
        assert_eq!(s.sync_page_serve_nanos, 7);
        assert_eq!(s.sync_block_requests, 9);
        assert_eq!(s.sync_blocks_received, 4);
        assert_eq!(s.sync_resends, 1);
        assert_eq!(s.sync_state_roots_requested, 2);
        assert_eq!(s.sync_replay_blocks, 10);
        assert_eq!(s.sync_replay_nanos, 20);
    }
}
