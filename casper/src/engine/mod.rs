//! Node engine (port of the `casper/engine/` state machines).
//!
//! The pure `LfsState`/`LfsTupleSpaceState` requester states, the effectful
//! `LfsBlockRequester`/`LfsTupleSpaceRequester`/`NodeSyncing`/`NodeRunning` machines, and
//! `NodeLaunch`'s genesis-from-config helpers + `apply` mode-dispatch are ported.

pub mod catchup;
pub mod lfs_block_requester;
pub mod lfs_tuple_space_requester;
pub mod node_launch;
pub mod node_running;
pub mod node_syncing;
pub mod recovery;
pub mod reset;

use std::collections::{BTreeMap, BTreeSet};

/// A casper-message handling status (port of `NodeRunning.CasperMessageStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CasperMessageStatus {
    BlockIsInDag,
    BlockIsInCasperBuffer,
    BlockIsReceived,
    BlockIsWaitingForCasper,
    BlockIsInProcessing,
    DoNotIgnore,
}

/// Whether to ignore a casper message + the status (port of `IgnoreCasperMessageStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IgnoreCasperMessageStatus {
    pub do_ignore: bool,
    pub status: CasperMessageStatus,
}

/// A request status (port of `LfsBlockRequester.ReqStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReqStatus {
    Init,
    Requested,
    Received,
}

/// Flags describing a received block (port of `LfsBlockRequester.ReceiveInfo`).
///
/// `known` is the #457 addition: whether the walk still owed this key, whatever request status it was
/// in. Validation reads it rather than `requested`, so an answer that arrives before its request turn
/// is accepted on its content — the request slot is a scheduling detail, not the verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceiveInfo {
    pub requested: bool,
    pub known: bool,
    pub latest: bool,
    pub last_latest: bool,
}

/// Request-state machine for the Last Finalized State block requester (port of
/// `LfsBlockRequester.ST`, holding `d`/`latest`/`height_map`/`finished`).
///
/// **`lowerBound` and `extraHeights` are deliberately absent, and that is faithful rather than a
/// divergence** (AUDIT C64). The oracle's `lowerBound` bounds the walk only if something seeds it,
/// and nothing does: `LfsBlockRequester.scala:125` defaults it to `0`, the sole construction in the
/// tree (`:318`) passes `latest` and `extraHeights` but never `lowerBound`, and so both of its gates
/// — `blockIsAccepted = isReceivedLatest || blockNumber >= minimumHeight` (`:228`) and
/// `NodeSyncing`'s `blockHeightOk = blockHeight >= minHeight` — are `>= 0` and vacuously true.
/// `extraHeights` is `deployLifespan` (50) and *lowers* the bound further rather than bounding the
/// walk. Adding the field here without the oracle seeding it would change behaviour away from the
/// oracle, not towards it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LfsState<Key: Ord + Clone> {
    pub d: BTreeMap<Key, ReqStatus>,
    pub latest: BTreeSet<Key>,
    pub height_map: BTreeMap<i64, BTreeSet<Key>>,
    pub finished: BTreeSet<Key>,
}

impl<Key: Ord + Clone> LfsState<Key> {
    /// Create a request state with initial keys (port of `ST.apply`).
    pub fn new(initial: BTreeSet<Key>, latest: BTreeSet<Key>) -> Self {
        LfsState {
            d: initial.into_iter().map(|k| (k, ReqStatus::Init)).collect(),
            latest,
            height_map: BTreeMap::new(),
            finished: BTreeSet::new(),
        }
    }

    /// Add new keys in `Init` status, skipping existing/finished keys (port of `add`).
    pub fn add(&self, keys: &BTreeSet<Key>) -> LfsState<Key> {
        let mut st = self.clone();
        for k in keys {
            if !st.finished.contains(k) && !st.d.contains_key(k) {
                st.d.insert(k.clone(), ReqStatus::Init);
            }
        }
        st
    }

    /// Get the next keys to request (port of `getNext`).
    pub fn get_next(&self, resend: bool) -> (LfsState<Key>, BTreeSet<Key>) {
        let mut requests = self.d.clone();
        if !self.latest.is_empty() {
            for k in &self.latest {
                requests.entry(k.clone()).or_insert(ReqStatus::Init);
            }
        }

        let mut new_requests: BTreeMap<Key, ReqStatus> = BTreeMap::new();
        for (key, status) in &requests {
            // **#457: a resend re-asks `Received` too.** `Received` means "an answer arrived" (and,
            // before the validate-before-consume fix, "its body may have been rejected"); it does not
            // mean "finished". Re-requesting only `Init`/`Requested` left such a key in the map for
            // ever, and `is_finished` — which requires `d` empty — could never hold.
            let check_for_request = *status == ReqStatus::Init
                || (resend && (*status == ReqStatus::Requested || *status == ReqStatus::Received));
            let select = if self.latest.is_empty() {
                check_for_request
            } else {
                check_for_request && self.latest.contains(key)
            };
            if select {
                new_requests.insert(key.clone(), ReqStatus::Requested);
            }
        }

        let mut st = self.clone();
        for (k, s) in &new_requests {
            st.d.insert(k.clone(), s.clone());
        }
        let keys: BTreeSet<Key> = new_requests.into_keys().collect();
        (st, keys)
    }

    /// Whether the walk still owes this key, whatever request status it is in.
    ///
    /// The distinction #457 needs: `d` is the set of keys the walk owes (an entry is removed only by
    /// `done`), while `latest`/`Requested` are scheduling state. An answer for a key in `d` is one the
    /// walk can verify and keep.
    pub fn known(&self, k: &Key) -> bool {
        self.d.contains_key(k)
    }

    /// The outstanding keys by status — `(Init, Requested, Received)` — for the give-up diagnostic.
    /// The `received` count is the one that used to be invisible: those keys were neither requested
    /// again nor counted as finished.
    pub fn outstanding_breakdown(&self) -> (usize, usize, usize) {
        let mut init = 0;
        let mut requested = 0;
        let mut received = 0;
        for status in self.d.values() {
            match status {
                ReqStatus::Init => init += 1,
                ReqStatus::Requested => requested += 1,
                ReqStatus::Received => received += 1,
            }
        }
        (init, requested, received)
    }

    /// Mark `k` received, returning the updated state + receive flags (port of `received`).
    ///
    /// **Any key the walk still owes is accepted, whatever its request status** (#457). The oracle
    /// accepts only `Requested`: an answer that overtakes its request turn is dropped, and — because
    /// `received` used to run before the content check — a rejected body consumed the key to
    /// `Received`, which no request ever selected again. `known` tells the caller whether the key was
    /// owed at all; an unknown hash is still refused without touching state.
    pub fn received(&self, k: &Key, height: i64) -> (LfsState<Key>, ReceiveInfo) {
        let Some(status) = self.d.get(k).copied() else {
            return (
                self.clone(),
                ReceiveInfo {
                    requested: false,
                    known: false,
                    latest: false,
                    last_latest: false,
                },
            );
        };
        let is_req = status == ReqStatus::Requested;

        let mut new_latest = self.latest.clone();
        new_latest.remove(k);
        let is_latest = self.latest != new_latest;
        let is_last_latest = is_latest && new_latest.is_empty();

        let mut height_map = self.height_map.clone();
        height_map.entry(height).or_default().insert(k.clone());

        let mut d = self.d.clone();
        d.insert(k.clone(), ReqStatus::Received);

        let st = LfsState {
            d,
            latest: new_latest,
            height_map,
            finished: self.finished.clone(),
        };
        (
            st,
            ReceiveInfo {
                requested: is_req,
                known: true,
                latest: is_latest,
                last_latest: is_last_latest,
            },
        )
    }

    /// Mark `k` finished if it was received (port of `done`).
    pub fn done(&self, k: &Key) -> LfsState<Key> {
        if self.d.get(k) == Some(&ReqStatus::Received) {
            let mut st = self.clone();
            st.d.remove(k);
            st.finished.insert(k.clone());
            st
        } else {
            self.clone()
        }
    }

    /// Whether all keys are finished (port of `isFinished`).
    pub fn is_finished(&self) -> bool {
        self.latest.is_empty() && self.d.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[i32]) -> BTreeSet<i32> {
        items.iter().copied().collect()
    }

    #[test]
    fn add_skips_finished_and_existing() {
        let st = LfsState::new(set(&[1]), set(&[]));
        let st = st.add(&set(&[1, 2]));
        assert_eq!(st.d.len(), 2);
        assert_eq!(st.d[&1], ReqStatus::Init);
        assert_eq!(st.d[&2], ReqStatus::Init);
    }

    #[test]
    fn get_next_requests_init_keys() {
        let st = LfsState::new(set(&[1, 2]), set(&[]));
        let (st, req) = st.get_next(false);
        assert_eq!(req, set(&[1, 2]));
        // All requested now.
        assert!(st.d.values().all(|s| *s == ReqStatus::Requested));
        // Second call without resend requests nothing.
        let (_, req2) = st.get_next(false);
        assert!(req2.is_empty());
        // With resend, requested keys are re-requested.
        let (_, req3) = st.get_next(true);
        assert_eq!(req3, set(&[1, 2]));
    }

    #[test]
    fn received_tracks_latest_flags() {
        let st = LfsState::new(set(&[]), set(&[1, 2]));
        let (st, _req) = st.get_next(false);
        let (st, info) = st.received(&1, 5);
        assert!(info.latest);
        assert!(!info.last_latest);

        let (st, info) = st.received(&2, 6);
        assert!(info.latest);
        assert!(info.last_latest);
        // Not finished until received keys are marked done.
        assert!(!st.is_finished());

        let st = st.done(&1).done(&2);
        assert!(st.is_finished());
    }

    /// #457: an answer for a key the walk owes is accepted even when its request turn has not come.
    /// The content check is what verifies a block; the request slot is only scheduling.
    #[test]
    fn received_accepts_a_known_key_before_its_request_turn() {
        let st = LfsState::new(set(&[1]), set(&[]));
        assert_eq!(st.d[&1], ReqStatus::Init);
        let (st, info) = st.received(&1, 5);
        assert!(info.known, "the key was owed, so the answer is the walk's");
        assert!(
            !info.requested,
            "not yet requested — this is the early answer the gate used to drop"
        );
        assert_eq!(st.d[&1], ReqStatus::Received);
    }

    /// #457: `Received` is not finished, and a resend must select it. Before the fix this key was in
    /// neither the request set nor the finished set, so `is_finished` could never hold.
    #[test]
    fn resend_re_requests_a_received_key() {
        let st = LfsState::new(set(&[1]), set(&[]));
        let (st, req) = st.get_next(false);
        assert_eq!(req, set(&[1]));
        let (st, _) = st.received(&1, 5);
        assert_eq!(st.d[&1], ReqStatus::Received);

        let (_, normal) = st.get_next(false);
        assert!(
            normal.is_empty(),
            "the normal path does not re-ask a received key"
        );
        let (st, resent) = st.get_next(true);
        assert_eq!(resent, set(&[1]), "the resend path re-asks it");
        assert_eq!(st.d[&1], ReqStatus::Requested);
    }

    /// An unknown hash is still dropped: `known` is the gate, not `Requested`.
    #[test]
    fn received_still_rejects_an_unknown_key() {
        let st = LfsState::new(set(&[1]), set(&[]));
        let (st, info) = st.received(&2, 5);
        assert!(!info.known);
        assert!(!st.d.contains_key(&2));
        assert_eq!(st.d[&1], ReqStatus::Init, "state is untouched");
    }

    #[test]
    fn outstanding_breakdown_counts_each_status() {
        let st = LfsState::new(set(&[1, 2, 3]), set(&[]));
        let (st, _) = st.get_next(false);
        let (st, _) = st.received(&1, 5);
        let (st, _) = st.received(&2, 5);
        assert_eq!(st.outstanding_breakdown(), (0, 1, 2));
    }

    #[test]
    fn done_marks_finished() {
        let st = LfsState::new(set(&[1]), set(&[]));
        let (st, _) = st.get_next(false);
        let (st, _) = st.received(&1, 0);
        let st = st.done(&1);
        assert!(st.d.is_empty());
        assert!(st.finished.contains(&1));
        assert!(st.is_finished());
    }
}
