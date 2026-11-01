//! **Connection diagnostics** — the counters the connection layer never had.
//!
//! The connection layer took every decision from table order, XOR distance or a random shuffle, and
//! recorded none of its outcomes: a failed dial, an eviction at `MAX_CONNECTIONS`, a heartbeat that
//! went unanswered, a relay that could not connect, a routing queue at its cap. An operator could see
//! `peers` (a count) and nothing about how it got there. This module is the first instrument, and it
//! is **advisory only**: no value here is read by a dial, eviction, retrieval, relay or consensus
//! decision. It exists so a later decision can be taken with evidence rather than a guess.
//!
//! Scope is **process**: every counter is a running total since process start (`last_peer_contact_at`
//! is a monotonic stamp, not a wall clock), and `peer_contact_age_ms` is `None` until a peer is
//! heard — never `0`, which would claim a contact that has not happened.
//!
//! The pattern mirrors `casper::blocks::block_processor::pipeline_faults()`: process-global relaxed
//! atomics, no lock, safe to touch on the hot path. Nothing here allocates.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::OnceLock;

/// The process-wide connection counters.
#[derive(Debug)]
pub struct ConnectionStats {
    connect_rounds: AtomicU64,
    dial_attempts: AtomicU64,
    dial_failures: AtomicU64,
    dial_budget_skipped: AtomicU64,
    evictions: AtomicU64,
    heartbeat_pings: AtomicU64,
    heartbeat_failures: AtomicU64,
    relay_offers: AtomicU64,
    relay_failures: AtomicU64,
    routing_queue_depth: AtomicU64,
    last_peer_contact_at: AtomicI64,
}

/// A point-in-time copy, for a metrics scrape. Every age is milliseconds; `None` is unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnectionSnapshot {
    pub connect_rounds: u64,
    pub dial_attempts: u64,
    pub dial_failures: u64,
    pub dial_budget_skipped: u64,
    pub evictions: u64,
    pub heartbeat_pings: u64,
    pub heartbeat_failures: u64,
    pub relay_offers: u64,
    pub relay_failures: u64,
    pub routing_queue_depth: u64,
    pub peer_contact_age_ms: Option<u64>,
}

impl Default for ConnectionStats {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionStats {
    /// A fresh cell has **not** observed a peer: `i64::MIN` is the unset sentinel, so a real stamp
    /// of `0` (`nano_time`'s first reading) is still an observation.
    pub fn new() -> Self {
        Self {
            connect_rounds: AtomicU64::new(0),
            dial_attempts: AtomicU64::new(0),
            dial_failures: AtomicU64::new(0),
            dial_budget_skipped: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            heartbeat_pings: AtomicU64::new(0),
            heartbeat_failures: AtomicU64::new(0),
            relay_offers: AtomicU64::new(0),
            relay_failures: AtomicU64::new(0),
            routing_queue_depth: AtomicU64::new(0),
            last_peer_contact_at: AtomicI64::new(i64::MIN),
        }
    }

    pub fn note_connect_round(&self) {
        self.connect_rounds.fetch_add(1, Ordering::Relaxed);
    }
    pub fn note_dial_attempt(&self) {
        self.dial_attempts.fetch_add(1, Ordering::Relaxed);
    }
    pub fn note_dial_failure(&self) {
        self.dial_failures.fetch_add(1, Ordering::Relaxed);
    }
    /// Peers the round did not reach because `room` or the round budget ended it.
    pub fn note_dial_budget_skipped(&self, count: usize) {
        self.dial_budget_skipped
            .fetch_add(count as u64, Ordering::Relaxed);
    }
    pub fn note_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }
    pub fn note_heartbeat_ping(&self) {
        self.heartbeat_pings.fetch_add(1, Ordering::Relaxed);
    }
    pub fn note_heartbeat_failure(&self) {
        self.heartbeat_failures.fetch_add(1, Ordering::Relaxed);
    }
    pub fn note_relay_offer(&self) {
        self.relay_offers.fetch_add(1, Ordering::Relaxed);
    }
    pub fn note_relay_failure(&self) {
        self.relay_failures.fetch_add(1, Ordering::Relaxed);
    }
    /// The routing queue's occupancy, sampled where a message is sent.
    pub fn set_routing_queue_depth(&self, depth: usize) {
        self.routing_queue_depth
            .store(depth as u64, Ordering::Relaxed);
    }
    /// **A peer was heard.** Only a same-network, authenticated inbound message calls this; a
    /// connection-table entry is not contact.
    pub fn note_peer_contact(&self) {
        self.last_peer_contact_at
            .store(rchain_shared::time::nano_time(), Ordering::Relaxed);
    }

    /// Milliseconds since the last authenticated peer contact; `None` until one.
    pub fn peer_contact_age_ms(&self) -> Option<u64> {
        let stamp = self.last_peer_contact_at.load(Ordering::Relaxed);
        if stamp < 0 {
            None
        } else {
            Some(((rchain_shared::time::nano_time() - stamp).max(0) / 1_000_000) as u64)
        }
    }

    pub fn snapshot(&self) -> ConnectionSnapshot {
        ConnectionSnapshot {
            connect_rounds: self.connect_rounds.load(Ordering::Relaxed),
            dial_attempts: self.dial_attempts.load(Ordering::Relaxed),
            dial_failures: self.dial_failures.load(Ordering::Relaxed),
            dial_budget_skipped: self.dial_budget_skipped.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            heartbeat_pings: self.heartbeat_pings.load(Ordering::Relaxed),
            heartbeat_failures: self.heartbeat_failures.load(Ordering::Relaxed),
            relay_offers: self.relay_offers.load(Ordering::Relaxed),
            relay_failures: self.relay_failures.load(Ordering::Relaxed),
            routing_queue_depth: self.routing_queue_depth.load(Ordering::Relaxed),
            peer_contact_age_ms: self.peer_contact_age_ms(),
        }
    }
}

static STATS: OnceLock<ConnectionStats> = OnceLock::new();

/// The process-wide cell. Every writer is on a hot path, so this is a `OnceLock` of relaxed atomics
/// rather than a mutex or a registry lookup.
pub fn stats() -> &'static ConnectionStats {
    STATS.get_or_init(ConnectionStats::default)
}

/// A point-in-time copy for a scrape.
pub fn snapshot() -> ConnectionSnapshot {
    stats().snapshot()
}

/// Milliseconds since the last authenticated peer contact; `None` until one.
pub fn peer_contact_age_ms() -> Option<u64> {
    stats().peer_contact_age_ms()
}

/// **The occupancy of a bounded queue, sampled before an enqueue.** After a successful send the
/// queue has already drained by one, so a post-send sample cannot observe a full queue.
pub fn queue_depth<T>(sender: &tokio::sync::mpsc::Sender<T>) -> usize {
    sender.max_capacity().saturating_sub(sender.capacity())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_cell_has_no_contact_age_not_zero() {
        let cell = ConnectionStats::default();
        assert_eq!(cell.peer_contact_age_ms(), None);
    }

    #[test]
    fn a_contact_stamps_and_the_age_is_monotone() {
        let cell = ConnectionStats::default();
        cell.note_peer_contact();
        let first = cell.peer_contact_age_ms().expect("a contact was recorded");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = cell.peer_contact_age_ms().expect("still recorded");
        assert!(second >= first, "{second} must not precede {first}");
    }

    #[tokio::test]
    async fn a_queue_depth_is_sampled_before_the_enqueue() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(2);
        assert_eq!(queue_depth(&tx), 0);
        tx.send(1).await.expect("room");
        assert_eq!(queue_depth(&tx), 1);
        rx.recv().await.expect("one item");
        assert_eq!(queue_depth(&tx), 0);
    }

    #[test]
    fn counters_are_running_totals() {
        let cell = ConnectionStats::default();
        cell.note_connect_round();
        cell.note_dial_attempt();
        cell.note_dial_failure();
        cell.note_eviction();
        cell.note_heartbeat_ping();
        cell.note_heartbeat_failure();
        cell.note_relay_offer();
        cell.note_relay_failure();
        cell.set_routing_queue_depth(7);
        let snap = cell.snapshot();
        assert_eq!(snap.connect_rounds, 1);
        assert_eq!(snap.dial_attempts, 1);
        assert_eq!(snap.dial_failures, 1);
        assert_eq!(snap.evictions, 1);
        assert_eq!(snap.heartbeat_pings, 1);
        assert_eq!(snap.heartbeat_failures, 1);
        assert_eq!(snap.relay_offers, 1);
        assert_eq!(snap.relay_failures, 1);
        assert_eq!(snap.routing_queue_depth, 7);
    }
}
