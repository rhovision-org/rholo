//! Bounded tables, and the caps the peer-facing ones use.
//!
//! **Why a type and not a check.** Six tables in this crate grow on input a peer chooses: the export
//! and answer tables, the gift store and its replay guard, a promise's listener list, and the registry
//! of peers. Every one of them was an ordinary `BTreeMap`/`Vec`, so a peer could grow the node's
//! memory without bound — measured by the HAZOP's red team, three independent vectors, `gifts` at
//! 191 B per message (AUDIT C223).
//!
//! A check at each insert site would work until someone adds a seventh site and forgets, which is the
//! shape of every "we already bound that" claim this repository has had to correct. [`Bounded`] has
//! **no unbounded constructor and no `insert`**: the only way in is [`Bounded::try_insert`], which
//! answers `Err(Full)` and names the cap. A site that forgets to handle `Full` does not compile.
//!
//! **The caps are on the count, so the keys must be bounded separately.** A count of 1024 over a key a
//! peer may make 4 MiB long is a 4 GiB table — which is why [`MAX_GIFT_ID`], [`MAX_DESIGNATOR`],
//! [`MAX_TRANSPORT`] and [`MAX_HINTS`] live here beside the counts, and why [`check_peer_sized`] is
//! the function a caller must run on a peer's own bytes *before* they become a key. Counts are
//! calibrated from a per-session memory budget, not from a multiple of the conformance suite's usage:
//! the suite is one cooperative client, and a node's cap has to hold for the largest legitimate holder.

use std::collections::{BTreeMap, BTreeSet};

/// A table that cannot grow past `cap`.
pub struct Bounded<K: Ord, V> {
    inner: BTreeMap<K, V>,
    cap: usize,
}

/// The table is at its cap. Carries the cap so a refusal can name the number it hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full {
    pub cap: usize,
}

impl std::fmt::Display for Full {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the table is at its cap of {}", self.cap)
    }
}

impl<K: Ord, V> Bounded<K, V> {
    /// The only constructor: a cap is not optional, and there is no `Default`.
    pub fn new(cap: usize) -> Self {
        Bounded {
            inner: BTreeMap::new(),
            cap,
        }
    }

    /// Insert, or refuse. Replacing an existing key is always allowed — it does not grow the table.
    pub fn try_insert(&mut self, key: K, value: V) -> Result<(), Full> {
        if !self.inner.contains_key(&key) && self.inner.len() >= self.cap {
            return Err(Full { cap: self.cap });
        }
        self.inner.insert(key, value);
        Ok(())
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.inner.get(key)
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.inner.get_mut(key)
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key)
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.inner.iter()
    }
}

/// A set that cannot grow past `cap` — the replay guard's shape.
pub struct BoundedSet<K: Ord> {
    inner: BTreeSet<K>,
    cap: usize,
}

impl<K: Ord> BoundedSet<K> {
    pub fn new(cap: usize) -> Self {
        BoundedSet {
            inner: BTreeSet::new(),
            cap,
        }
    }

    /// Add, or refuse. Returning `Err` for a key already present distinguishes "full" from "already
    /// there", which the replay guard needs: a *repeat* is an error of a different kind and the
    /// caller decides it, not this table.
    pub fn try_add(&mut self, key: K) -> Result<(), Full> {
        if !self.inner.contains(&key) && self.inner.len() >= self.cap {
            return Err(Full { cap: self.cap });
        }
        self.inner.insert(key);
        Ok(())
    }

    pub fn contains(&self, key: &K) -> bool {
        self.inner.contains(key)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

// --- the caps -------------------------------------------------------------------------------------
//
// One place, each with what it is derived from. The suite's own usage is the *floor* (a cap below it
// breaks the conformance gate); the ceiling is a session's memory budget.

/// Exports one session may hold. One per fetched or returned capability, and each `ChainCapability` is
/// a few hundred bytes; 1024 is ~50× the suite's heaviest module and a few hundred KB at worst.
pub const MAX_EXPORTS: usize = 1024;

/// Answers one session may hold. Keyed by the **peer's** `answer_pos`, so this is the cap that keeps a
/// peer from naming a fresh position per delivery for ever. The suite's pipelining cases use a handful.
pub const MAX_ANSWERS: usize = 1024;

/// Answers a session may have **in flight** at once (Law 61): deliveries whose object returned
/// `Reply::Deferred` and whose waiter has not resolved yet. Bounded like every queue a peer can drive
/// — a peer that keeps claiming gifts nobody deposits must not grow this without limit. A full queue
/// costs the *waiter* nothing: its answer never lands, and the claim times out as it did before the
/// deferral existed. 64 is the hand-off queue's depth, which bounds the same shape one layer up.
pub const MAX_DEFERRED_ANSWERS: usize = 64;

/// **Live deferred-answer waiters on one session** (HAZOP row C230). [`MAX_DEFERRED_ANSWERS`] bounds
/// the *channel* a landed answer travels on, not the tasks that produce them: each `Reply::Deferred`
/// spawns one, and each polls the node-global gift store every 10 ms for up to ten seconds while
/// holding that store's mutex for each look. A peer that repeats a claim for a gift nobody deposited
/// spawns one per delivery — and may repeat it freely, because a claim that withdraws nothing must
/// not count against the replay guard — so the count was the peer's to choose, per session, with no
/// ceiling at all. Past this many the delivery is refused with the same `<break>` a failed claim
/// produces, which is an answer rather than a silence.
///
/// Much smaller than [`MAX_DEFERRED_ANSWERS`] on purpose: sixteen waiters polling once per 10 ms is
/// already ~1 600 mutex acquisitions a second from one session, and the number worth bounding is the
/// task count, not the answer queue. A conforming peer needs one or two.
pub const MAX_DEFERRED_WAITERS: usize = 16;

/// Gifts the node's store may hold, across all sessions (the store is per peer, not per session).
/// With a bounded gift id this is a bounded number of bytes; without one it was the 4 GiB table.
pub const MAX_GIFTS: usize = 256;

/// Handoff counts remembered **per gift**. The guard only needs to refuse a count it has seen; 256 is
/// far past any legitimate sequence and stops an arbitrary `u64` per withdraw from growing a set.
pub const MAX_WITHDRAWN_PER_GIFT: usize = 256;

/// Listeners one promise cell may hold. The peer-driven growth is one `op:listen` per entry, and the
/// cell's own count is bounded by the export table's cap (each resolver delivery mints one cell).
pub const MAX_LISTENERS: usize = 256;

/// Peers the registry tracks. Keyed by the peer's own designator, so this is the cap that makes the
/// *keys* finite; `forget` removes emptied entries, so a well-behaved peer is not what fills it.
pub const MAX_PEERS: usize = 256;

/// **Sessions the node is made to *dial*, as a share of the same budget** (HAZOP row C241). The
/// session ceiling is taken by the accept path — one share per transport — and nothing took it for a
/// dial, while a dialed session stays in the registry for its life: the node could be made to hold
/// `MAX_PEERS` of them, four times the ceiling it thinks it has. This is that share, and it is
/// deliberately a *quarter* of the ceiling: a dialed session is one this node started, so the number
/// an operator's own tooling needs is small, and the transports' capacity is not what pays for it.
pub const MAX_DIALED_SESSIONS: usize = 16;

/// **Take a slot from the node's own-dial budget, before the socket is opened** (HAZOP row C241;
/// C347 for the second caller).
///
/// Two fixtures dial — the enlivener and the greeter — and both must draw from the one semaphore the
/// node installs, so the sessions the node *starts* are bounded by [`MAX_DIALED_SESSIONS`] rather than
/// by [`MAX_PEERS`]. `None` is the conformance harness, which installs no budget: its dials happen
/// inside a session that already holds a slot.
///
/// **The permit's lifetime is the caller's.** A dialer holds it in the task that serves the session,
/// so the slot lasts exactly as long as the session and a peer that closes releases it; taking it
/// before the connect is what makes a dial the node cannot afford one it does not open.
pub(crate) fn reserve_dial_slot(
    dials: &Option<std::sync::Arc<tokio::sync::Semaphore>>,
) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, String> {
    match dials {
        Some(dials) => match dials.clone().try_acquire_owned() {
            Ok(permit) => Ok(Some(permit)),
            Err(_) => Err(format!(
                "this node already holds as many dialed sessions as it may ({MAX_DIALED_SESSIONS})"
            )),
        },
        None => Ok(None),
    }
}

/// Longest gift id accepted, in bytes. A gift id is a peer's own name for a handoff; 64 bytes is past
/// every real one and far below the 4 MiB message cap that would otherwise be the key's size.
pub const MAX_GIFT_ID: usize = 64;

/// Longest peer designator accepted, in bytes — it becomes half a registry key.
pub const MAX_DESIGNATOR: usize = 256;

/// Longest transport name accepted, in bytes — the other half of a registry key.
pub const MAX_TRANSPORT: usize = 64;

/// Most hints a locator may carry.
pub const MAX_HINTS: usize = 8;

/// Longest hint value (host, port, …) accepted, in bytes.
pub const MAX_HINT_VALUE: usize = 256;

/// **The peer-sized-key check.** A locator's fields arrive in the peer's own `op:start-session` and
/// become a registry key, so they are bounded before they are stored rather than after — the counts
/// above cannot help against a key that is itself megabytes.
pub fn check_peer_sized(
    designator: &str,
    transport: &str,
    hints: &BTreeMap<String, String>,
) -> Result<(), String> {
    if designator.len() > MAX_DESIGNATOR {
        return Err(format!(
            "the peer's designator is {} bytes, past the {MAX_DESIGNATOR}-byte bound",
            designator.len()
        ));
    }
    if transport.len() > MAX_TRANSPORT {
        return Err(format!(
            "the peer's transport is {} bytes, past the {MAX_TRANSPORT}-byte bound",
            transport.len()
        ));
    }
    if hints.len() > MAX_HINTS {
        return Err(format!(
            "the peer's locator carries {} hints, past the bound of {MAX_HINTS}",
            hints.len()
        ));
    }
    for (key, value) in hints {
        if key.len() > MAX_HINT_VALUE || value.len() > MAX_HINT_VALUE {
            return Err(format!(
                "the peer's locator hint {key:?} is past the {MAX_HINT_VALUE}-byte bound"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The cap is the type's whole point**, and there is no way around it: this is the test that
    /// would fail if someone added an `insert` that skipped the check.
    #[test]
    fn a_bounded_table_refuses_the_insert_past_its_cap() {
        let mut table: Bounded<u64, &str> = Bounded::new(2);
        assert_eq!(table.try_insert(1, "a"), Ok(()));
        assert_eq!(table.try_insert(2, "b"), Ok(()));
        assert_eq!(table.try_insert(3, "c"), Err(Full { cap: 2 }));
        assert_eq!(table.len(), 2, "the refused insert changed nothing");

        // Replacing an existing key is not growth, so it is never refused.
        assert_eq!(table.try_insert(1, "A"), Ok(()));
        assert_eq!(table.get(&1), Some(&"A"));

        // And a removal makes room again — the cap bounds the table, not its history.
        assert_eq!(table.remove(&1), Some("A"));
        assert_eq!(table.try_insert(3, "c"), Ok(()));
    }

    #[test]
    fn a_bounded_set_refuses_the_add_past_its_cap_and_says_which() {
        let mut set: BoundedSet<u64> = BoundedSet::new(1);
        assert_eq!(set.try_add(7), Ok(()));
        // Adding a key already present is not growth; the caller decides what a repeat means.
        assert_eq!(set.try_add(7), Ok(()));
        assert_eq!(set.try_add(8), Err(Full { cap: 1 }));
        assert!(set.contains(&7) && !set.contains(&8));
    }

    /// The keys are the other half of the bound: a count cap over a peer-sized key bounds nothing.
    #[test]
    fn a_peer_locator_past_the_byte_bounds_is_refused() {
        let ok = BTreeMap::from([("host".to_string(), "127.0.0.1".to_string())]);
        assert!(check_peer_sized("peer", "tcp-testing-only", &ok).is_ok());

        assert!(check_peer_sized(&"d".repeat(MAX_DESIGNATOR + 1), "t", &ok).is_err());
        assert!(check_peer_sized("d", &"t".repeat(MAX_TRANSPORT + 1), &ok).is_err());
        assert!(check_peer_sized("d", "t", &BTreeMap::new()).is_ok());

        let too_many: BTreeMap<String, String> = (0..=MAX_HINTS)
            .map(|i| (format!("h{i}"), "v".to_string()))
            .collect();
        assert!(check_peer_sized("d", "t", &too_many).is_err());

        let too_long = BTreeMap::from([("host".to_string(), "x".repeat(MAX_HINT_VALUE + 1))]);
        assert!(check_peer_sized("d", "t", &too_long).is_err());
    }

    /// **A dial takes a slot from the shared budget, and the refusal names the ceiling** (C347).
    ///
    /// Both dialing fixtures go through this function, so there is one rule: at most
    /// `MAX_DIALED_SESSIONS` node-started sessions, taken before the connect, released when the
    /// session's own task ends.
    #[test]
    fn a_dial_slot_is_taken_from_the_shared_budget_and_released_with_the_permit() {
        // The conformance harness installs no budget: its dials happen inside a session that
        // already holds a slot.
        assert!(reserve_dial_slot(&None)
            .expect("no budget is not a refusal")
            .is_none());

        let dials = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_DIALED_SESSIONS));
        let held: Vec<_> = (0..MAX_DIALED_SESSIONS)
            .map(|_| reserve_dial_slot(&Some(dials.clone())).expect("within the budget"))
            .collect();
        assert!(held.iter().all(|permit| permit.is_some()));

        let err = reserve_dial_slot(&Some(dials.clone())).expect_err("the budget is spent");
        assert!(
            err.contains(&MAX_DIALED_SESSIONS.to_string()),
            "the refusal names the ceiling it hit: {err}"
        );

        // A closed session releases its slot, so the bound is on *concurrent* dials.
        drop(held);
        assert!(reserve_dial_slot(&Some(dials))
            .expect("a released slot is free")
            .is_some());
    }

    /// **Every dialing fixture draws from the shared budget**, asserted at the source.
    ///
    /// The rule has one implementation and two callers, and a caller that stops drawing from it is
    /// the defect C347 names coming back. The dial itself needs a live session to reach, so the
    /// call site — not the dial — is what this pins: the conformance fixtures are exercised end to
    /// end by the reference suite, which is a Python process this crate does not run.
    #[test]
    fn every_dialing_fixture_draws_from_the_shared_budget() {
        for file in ["src/enliven.rs", "src/fixtures.rs"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
            let text = std::fs::read_to_string(&path).expect("the fixture source is readable");
            assert!(
                text.contains("reserve_dial_slot"),
                "{file} dials without drawing from the node's dial budget"
            );
        }
    }
}
