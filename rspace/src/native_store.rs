//! The native system-contract store: byte-oriented state (registry / PoS / vault) folded into the
//! same content-addressed radix trie as the tuple space.
//!
//! Native state is stored under dedicated trie prefixes (`PREFIX_REGISTRY`/`PREFIX_POS`/
//! `PREFIX_VAULT`) as `NativeLeaf` payloads. The `InMemNativeStore` is a write-through overlay on top
//! of a `NativeHistoryReader`: reads fall through to the persisted trie, writes are buffered in the
//! overlay, and `drain_changes` produces the `NativeStoreAction`s that the history repository folds
//! into the next checkpoint. This mirrors the `HotStore`/`HistoryRepository` split so native state
//! stays content-addressed, replayable, and queryable at an arbitrary state hash.

use rchain_shared::lock::Unpoison;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;

/// Trie prefix for native registry entries (`uri-bytes -> Par`).
pub const PREFIX_REGISTRY: u8 = 0x03;
/// Trie prefix for native PoS state (`bonds` / `active` / `withdrawers` / `params` leaves).
pub const PREFIX_POS: u8 = 0x04;
/// Trie prefix for native vault state (`rev-address -> balance`).
pub const PREFIX_VAULT: u8 = 0x05;
/// Trie prefix for native cross-shard transaction state (`txn-id -> TxnRecord`).
pub const PREFIX_TXN: u8 = 0x06;
/// Trie prefix for the native HTTP claim registry (`url -> recorded value`); RCHIP #54.
pub const PREFIX_HTTP: u8 = 0x07;
/// Trie prefix for the vault **handle** map (`minted-name -> base58 rev-address`) — the record that
/// lets a name handed out by `findOrCreate` still resolve to its vault in a later block, and after a
/// replay. A handle is a rholang `GPrivate` name, and it is a *lookup* key only: holding the name a
/// vault was opened under is not by itself the right to spend from it — that is [`PREFIX_VAULT_AUTH`],
/// which is the distinction that keeps `findOrCreate(someone_else's_address)` from being an authority.
pub const PREFIX_VAULT_NAME: u8 = 0x08;
/// Trie prefix for the vault **authority** map (`unforgeable-name -> base58 rev-address`).
///
/// Separate from [`PREFIX_VAULT_NAME`] because the two are different questions: a *handle* says which
/// vault a name opens, and an *authority* says who may spend it. Keeping them in one map would make
/// `findOrCreate(victim_address)` an authority over the victim's vault — the handle alone would be
/// enough to spend, which is exactly the hole this level of the design exists to close. An authority
/// is recorded only by `unforgeableAuthKey`, whose argument the caller must already hold.
pub const PREFIX_VAULT_AUTH: u8 = 0x09;
/// Trie prefix for the **ERTP issuer ledger** (issue #249): what an issuer says its purses and
/// payments hold.
///
/// Two kinds of leaf share it, told apart by their key: a *brand* leaf records the bytes of the
/// issuing authority (the issuer's unforgeable name, the one thing that may mint), and a *holding*
/// leaf records one purse's or payment's amount and whether it is still live — a payment is consumed
/// by its first deposit, which is ERTP's double-spend guard.
///
/// Separate from [`PREFIX_VAULT`] because REV is not special here: this is the multi-token layer
/// REV becomes one brand of, and a REV balance stays where it was rather than moving into a ledger
/// whose shapes would then have to serve two masters.
pub const PREFIX_ERTP: u8 = 0x0A;

/// A native-state mutation, folded into the trie at checkpoint (port of a `NativeStoreAction`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeStoreAction {
    Put {
        prefix: u8,
        key: Blake2b256Hash,
        value: Vec<u8>,
    },
    Delete {
        prefix: u8,
        key: Blake2b256Hash,
    },
}

impl NativeStoreAction {
    /// The **one** trie slot this action addresses, as `(prefix, key)`.
    ///
    /// This is the identity a batch may hold only once: `RadixHistory::process` refuses a batch with
    /// two actions on one key, and native actions share the trie's key space under their own
    /// prefixes (`PREFIX_REGISTRY`..`PREFIX_VAULT_AUTH`), so the check applies to them exactly as it
    /// does to tuple-space actions.
    ///
    /// It exists as a method because the *producer* of a batch has to dedupe by it and the reason is
    /// not obvious from either variant: a `Put` and a `Delete` on one slot collide just as two
    /// `Put`s do, and "one action per slot" is what a merge needs to restore when it concatenates
    /// the native effects of several blocks (issue #83).
    pub fn slot(&self) -> (u8, Blake2b256Hash) {
        match self {
            NativeStoreAction::Put { prefix, key, .. } => (*prefix, *key),
            NativeStoreAction::Delete { prefix, key } => (*prefix, *key),
        }
    }
}

/// A keyed read of native state from a history root (implemented by the history reader).
#[async_trait]
pub trait NativeHistoryReader: Send + Sync {
    async fn get_native(&self, prefix: u8, key: Blake2b256Hash) -> Result<Option<Vec<u8>>, String>;
}

/// A no-op native reader (returns `None` for every key) — used as the initial reader before any
/// checkpoint/reset has established a history root.
struct NoopNativeReader;

#[async_trait]
impl NativeHistoryReader for NoopNativeReader {
    async fn get_native(
        &self,
        _prefix: u8,
        _key: Blake2b256Hash,
    ) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}

/// **Which deploy a native write belongs to** (#280).
///
/// The overlay records one of these per slot, so a block's native effects are attributable *by
/// construction*: no consumer can be handed a set that belongs to "the block" as a whole, which is the
/// representation that let the merge reject a chain for a slot it never wrote
/// (`spec/audit/evidence/n280-merge-loses-a-write-results.md`).
///
/// There is no "unknown" arm and no "the block" arm. The one arm that is not a deploy is
/// [`NativeWriter::OutsideAnyDeploy`], which exists for genesis installation and for direct store use
/// outside block production — and which the **block path refuses** rather than attributing to the
/// block (`BlockNativeEffects::from_drain`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeWriter {
    /// The deploy at this **ordinal** in the block's list: `state.deploys` (user deploys, in their
    /// given order) followed by `state.system_deploys`. The ordinal is the one name both the drain and
    /// `BlockIndex::apply` have, and it is what maps a slot to the chain that must carry it.
    Deploy(u32),
    /// Cost accounting's window (`pre_charge` / `refund` / `pay_executor`). Re-derived by the merge
    /// from the accepted deploys (AUDIT C207), so it never travels in a block's sidecar — but it is
    /// *named* here rather than being "the other kind".
    ///
    /// It carries no ordinal because it does not need one: the window always lies inside the deploy
    /// it charges (the stack makes that structural), and a slot whose only writer is cost accounting
    /// is collected into the flat cost set rather than attributed.
    CostAccounting,
    /// **The genesis installation** — `install_genesis`, the vault balances and the registry aliases
    /// `compute_genesis` writes before block 0's deploy list exists.
    ///
    /// It is a *named* arm and not an "unknown" one, and it is the only writer that is not a deploy.
    /// It is deliberately **not carried in a sidecar**: what it writes is the block-0 configuration,
    /// which is already in the state every node holds — a merge never asks the genesis block for its
    /// native set, because the genesis is the merge's *base* rather than one of its conflict blocks.
    /// If that ever stops being true, this is the line to re-read, and the day it does the arm is what
    /// says so.
    Genesis,
    /// A write made outside any deploy's window, and outside the genesis installation too. Nothing in
    /// block production should produce one, which is why `BlockNativeEffects::from_drain` refuses it
    /// instead of filing it under the block.
    OutsideAnyDeploy,
}

impl NativeWriter {
    /// Whether this write is cost accounting's — the predicate the sidecar's split is made on, and the
    /// one that used to be a bare `bool`.
    pub fn is_cost(&self) -> bool {
        matches!(self, NativeWriter::CostAccounting)
    }
}

/// One overlay slot: the value it will hold, and **which deploy last wrote it** (AUDIT C207, #280).
///
/// The second half is what lets a block's sidecar carry its *own* native effects, attributed to the
/// deploys that made them. That matters twice over. Cost accounting writes `pos:vault` from **every**
/// user deploy, so a block that carries a deploy overlaps every concurrent sibling on that slot — and
/// the merge resolves an overlap by rejecting a chain, which silently took the deploy's own writes (a
/// delegation, a bond, a trust) with it. And at an epoch boundary every block runs `close_block`, so
/// every block's *union* overlaps every sibling's, which took a whole deploy with a contention it had
/// no part in. Splitting the two lets cost accounting move to the granularity the tuple space already
/// merges at — per accepted deploy — and lets every other write be carried by the chain that made it.
#[derive(Clone, Debug)]
struct Slot {
    /// Final value folded into the checkpoint.
    value: Option<Vec<u8>>,
    /// Lossless per-writer history for this slot since the last drain.  Merge attribution must not
    /// be derived from only the last writer: a later deploy may be rejected while an earlier deploy
    /// in the same block remains accepted (#280 review).
    history: Vec<(NativeWriter, Option<Vec<u8>>)>,
}

/// **The overlay, drained, grouped by the deploy that last wrote each slot** (#280).
///
/// One drain, three views — the checkpoint wants every action, the sidecar wants the attributed ones,
/// and the third arm is the one a block cannot carry. They are views of one drain rather than three
/// passes because three passes is three chances for the attributions to drift.
#[derive(Clone, Debug, Default)]
pub struct NativeDrain {
    /// The final checkpoint batch: exactly one action per slot.  This is intentionally separate from
    /// `by_deploy`, which may contain historical writes by several deploys to the same slot.
    final_actions: Vec<NativeStoreAction>,
    /// The block's **own** writes, by deploy ordinal.  Each deploy keeps its own last write to a slot,
    /// sidecar's shape. Cost accounting's writes are not here (AUDIT C207).
    pub by_deploy: BTreeMap<u32, Vec<NativeStoreAction>>,
    /// Cost accounting's writes, one flat set: the merge re-derives them from the accepted deploys.
    pub cost: Vec<NativeStoreAction>,
    /// Writes made outside any deploy's window. A block cannot attribute these, so the block path
    /// refuses them **loudly** rather than filing them under the block.
    pub unattributed: Vec<NativeStoreAction>,
    /// The genesis installation's writes — folded into the checkpoint like every other action, and
    /// deliberately not carried in a sidecar (see [`NativeWriter::Genesis`]).
    pub genesis: Vec<NativeStoreAction>,
}

impl NativeDrain {
    /// Every action, for the checkpoint that folds them — the union of the three views, and the only
    /// place they are combined.
    pub fn all(&self) -> Vec<NativeStoreAction> {
        self.final_actions.clone()
    }
}

/// **A block's native effects, keyed by the deploy that wrote them** (#280).
///
/// This is the sidecar's value type — what a `Vec<NativeStoreAction>` used to be. That flat type was
/// the representation the defect lived in: a set belonging to *no* deploy, which the merge could only
/// resolve at block granularity (`reject_whole_blocks`), and which therefore let a chain be rejected
/// for a slot it never wrote. An unattributed, block-level set is now not a value at all.
///
/// Private field, no `Deref`, no getter — the discipline `shared/src/refined.rs` states and
/// `tools/audit-type-system.sh` enforces (`spec/TYPE-SYSTEM.md` §1.7, "no type escape"). The ways out
/// are [`BlockNativeEffects::of_deploy`] (a slice, borrowed) and [`BlockNativeEffects::into_map`] (a
/// one-way discharge at the codec's boundary); neither hands the invariant back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockNativeEffects(BTreeMap<u32, Vec<NativeStoreAction>>);

impl BlockNativeEffects {
    /// Build from a drain, **refusing** a block that made a write outside any deploy's window.
    ///
    /// Such a block cannot attribute that write, and filing it under the block is precisely what this
    /// type removes — so the refusal is an error, never a silent omission. (Genesis installation is the
    /// legitimate producer of an unattributed write, and it does not build one of these.)
    pub fn from_drain(drain: &NativeDrain) -> Result<Self, String> {
        if !drain.unattributed.is_empty() {
            return Err(format!(
                "{} native write(s) outside any deploy's window: a block cannot attribute them, and \
                 filing them under the block is the representation #280 removed",
                drain.unattributed.len()
            ));
        }
        Ok(BlockNativeEffects(drain.by_deploy.clone()))
    }

    /// A block that wrote nothing native — a *different* value from a block whose writes could not be
    /// attributed ([`Self::from_drain`] answers that with an `Err`).
    pub fn empty() -> Self {
        BlockNativeEffects(BTreeMap::new())
    }

    /// The writes of the deploy at `ordinal` — empty when that deploy wrote nothing native.
    pub fn of_deploy(&self, ordinal: u32) -> &[NativeStoreAction] {
        self.0.get(&ordinal).map_or(&[], |v| v.as_slice())
    }

    /// Whether the block made no native write at all.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every ordinal this block has effects for, ascending.
    pub fn ordinals(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.keys().copied()
    }

    /// One-way discharge for the codec and for a caller that must place **every** entry: a
    /// `BTreeMap` leaves the domain. Paired with [`Self::from_map`], and neither is an escape: the
    /// invariant this type carries is its **key type** — every effect is keyed by a deploy ordinal,
    /// and there is no key that could mean "the block" — so a map of ordinals cannot express the
    /// state #280 removed. What `from_drain` adds on top is the refusal of a write with *no* deploy
    /// to key it by, which is a fact about a drain rather than about this type.
    pub fn into_map(self) -> BTreeMap<u32, Vec<NativeStoreAction>> {
        self.0
    }

    /// The decoder's and the accumulator's constructor: a map keyed by deploy ordinal, which is the
    /// form the wire carries.
    pub fn from_map(map: BTreeMap<u32, Vec<NativeStoreAction>>) -> Self {
        BlockNativeEffects(map)
    }

    /// Union with a later drain of the same block — the runtime accumulates one drain per checkpoint.
    /// Entries for one ordinal concatenate rather than replace, because the drains are disjoint (each
    /// checkpoint clears the overlay) and a replacement would silently drop the earlier half.
    pub fn merge(&mut self, other: BlockNativeEffects) {
        for (ordinal, mut actions) in other.0 {
            self.0.entry(ordinal).or_default().append(&mut actions);
        }
    }
}

/// Snapshot of the native-store overlay (for soft-checkpoint revert).
#[derive(Clone, Default, Debug)]
pub struct NativeStoreState {
    overlay: BTreeMap<(u8, Blake2b256Hash), Slot>,
}

/// The in-memory native store: a write-through overlay over a [`NativeHistoryReader`].
pub struct InMemNativeStore {
    /// Written keys and tombstones since the last `drain_changes` / `reset`.
    overlay: Mutex<BTreeMap<(u8, Blake2b256Hash), Slot>>,
    /// The base reader for keys not present in the overlay (updated on checkpoint/reset).
    reader: RwLock<Arc<dyn NativeHistoryReader>>,
    /// Set once a non-noop reader is installed, so [`InMemNativeStore::live_entries`] can report that
    /// it is no longer looking at the whole state.
    has_history: AtomicBool,
    /// **The window the writes being made right now happen inside** — the deploy (or cost-accounting
    /// step) whose reduction is running. Set by the runtime, which is the only place that knows the
    /// deploy's ordinal; `None` means outside any window, which is genesis installation and direct
    /// store use, and which the block path refuses rather than attributing to the block.
    /// **A stack**, because the windows nest: a deploy's own reduction contains the cost-accounting
    /// windows of its pre-charge and refund, and a caller that wraps a whole deploy must not have its
    /// window closed by the inner one. The innermost open window is the writer; the empty stack is
    /// "outside any deploy", which is genesis installation and direct store use.
    writer: Mutex<Vec<NativeWriter>>,
}

impl InMemNativeStore {
    pub fn new(reader: Arc<dyn NativeHistoryReader>) -> Self {
        InMemNativeStore {
            overlay: Mutex::new(BTreeMap::new()),
            reader: RwLock::new(reader),
            has_history: AtomicBool::new(false),
            writer: Mutex::new(Vec::new()),
        }
    }

    /// Open the window the writes that follow belong to, until [`Self::end_writer`].
    ///
    /// `pre_charge`, `refund` and `pay_executor` are system deploys evaluated *outside* the deploy's
    /// own reduction (`play_deploy_with_cost_accounting_once`), which is what makes their window
    /// exact: no user term can run between the two calls, so no user write can be misattributed. The
    /// deploy's own reduction gets a window of its own, and the ordinal it is opened under is the name
    /// the sidecar and the block index both use (#280).
    pub fn begin_writer(&self, writer: NativeWriter) {
        self.writer.lock().unpoison().push(writer);
    }

    /// Close the innermost window — see [`Self::begin_writer`].
    pub fn end_writer(&self) {
        self.writer.lock().unpoison().pop();
    }

    /// Which deploy the write being made right now belongs to.
    fn current_writer(&self) -> NativeWriter {
        self.writer
            .lock()
            .unpoison()
            .last()
            .copied()
            .unwrap_or(NativeWriter::OutsideAnyDeploy)
    }

    /// A store with no backing reader (reads return `None` until a reader is set).
    pub fn empty() -> Self {
        Self::new(Arc::new(NoopNativeReader))
    }

    /// Read a native value, consulting the overlay first and falling through to the persisted trie.
    pub async fn get(&self, prefix: u8, key: &Blake2b256Hash) -> Result<Option<Vec<u8>>, String> {
        {
            let overlay = self.overlay.lock().unpoison();
            if let Some(slot) = overlay.get(&(prefix, *key)) {
                return Ok(slot.value.clone());
            }
        }
        let reader = self.reader.read().unpoison().clone();
        reader.get_native(prefix, *key).await
    }

    /// Write a native value into the overlay (and record a `Put` action).
    pub fn put(&self, prefix: u8, key: Blake2b256Hash, value: Vec<u8>) {
        let writer = self.current_writer();
        self.record(prefix, key, Some(value), writer);
    }

    /// Delete a native value (record a `Delete` action via a tombstone).
    pub fn delete(&self, prefix: u8, key: &Blake2b256Hash) {
        let writer = self.current_writer();
        self.record(prefix, *key, None, writer);
    }

    /// The one place a slot is written, so a window cannot be forgotten at a call site. **A slot
    /// belongs to the last deploy that wrote it outside cost accounting**: cost accounting's writes are
    /// re-derived by the merge from the accepted deploys, so a later charge must not take a user's slot
    /// away from them — the C207 rule, which used to be a `bool` and is now the writer's name.
    fn record(
        &self,
        prefix: u8,
        key: Blake2b256Hash,
        value: Option<Vec<u8>>,
        writer: NativeWriter,
    ) {
        let mut overlay = self.overlay.lock().unpoison();
        if let Some(slot) = overlay.get_mut(&(prefix, key)) {
            slot.value = value.clone();
            slot.history.push((writer, value));
            return;
        }
        overlay.insert(
            (prefix, key),
            Slot {
                value: value.clone(),
                history: vec![(writer, value)],
            },
        );
    }

    /// Drain the pending mutations, clearing the overlay (the caller folds the actions into a
    /// checkpoint). This is **every** action, whatever its writer; a caller that needs them attributed
    /// wants [`Self::drain_native`].
    pub fn drain_changes(&self) -> Vec<NativeStoreAction> {
        self.drain_native().all()
    }

    /// Drain the pending mutations **attributed to the deploy that wrote each slot** (#280).
    ///
    /// All three views belong in the checkpoint — the block's own post-state is what it is — but only
    /// `by_deploy` belongs in the block's sidecar, because cost accounting's writes are re-derived by
    /// the merge from the accepted deploys (AUDIT C207) and a write outside any window has no deploy to
    /// travel under. One drain produces all three, so the attributions cannot drift from the actions
    /// they name.
    pub fn drain_native(&self) -> NativeDrain {
        let mut overlay = self.overlay.lock().unpoison();
        let mut drain = NativeDrain::default();
        let mut by_deploy: BTreeMap<u32, BTreeMap<(u8, Blake2b256Hash), NativeStoreAction>> =
            BTreeMap::new();
        let mut cost: BTreeMap<(u8, Blake2b256Hash), NativeStoreAction> = BTreeMap::new();
        let mut genesis: BTreeMap<(u8, Blake2b256Hash), NativeStoreAction> = BTreeMap::new();
        let mut unattributed: BTreeMap<(u8, Blake2b256Hash), NativeStoreAction> = BTreeMap::new();

        for (&(prefix, key), slot) in overlay.iter() {
            let final_action = match &slot.value {
                Some(v) => NativeStoreAction::Put {
                    prefix,
                    key,
                    value: v.clone(),
                },
                None => NativeStoreAction::Delete { prefix, key },
            };
            drain.final_actions.push(final_action);

            // Preserve every deploy's own last write to this slot.  The checkpoint still receives only
            // the final action above, so RadixHistory's one-action-per-key invariant is unchanged.
            for (writer, value) in &slot.history {
                let action = match value {
                    Some(v) => NativeStoreAction::Put {
                        prefix,
                        key,
                        value: v.clone(),
                    },
                    None => NativeStoreAction::Delete { prefix, key },
                };
                match writer {
                    NativeWriter::Deploy(ordinal) => {
                        by_deploy
                            .entry(*ordinal)
                            .or_default()
                            .insert((prefix, key), action);
                    }
                    NativeWriter::CostAccounting => {
                        cost.insert((prefix, key), action);
                    }
                    NativeWriter::Genesis => {
                        genesis.insert((prefix, key), action);
                    }
                    NativeWriter::OutsideAnyDeploy => {
                        unattributed.insert((prefix, key), action);
                    }
                }
            }
        }
        drain.by_deploy = by_deploy
            .into_iter()
            .map(|(ordinal, actions)| (ordinal, actions.into_values().collect()))
            .collect();
        drain.cost = cost.into_values().collect();
        drain.genesis = genesis.into_values().collect();
        drain.unattributed = unattributed.into_values().collect();
        overlay.clear();
        drain
    }

    /// Capture the current overlay for a soft-checkpoint rollback.
    pub fn snapshot(&self) -> NativeStoreState {
        NativeStoreState {
            overlay: self.overlay.lock().unpoison().clone(),
        }
    }

    /// Restore a previously captured overlay (soft-checkpoint rollback).
    pub fn revert(&self, state: NativeStoreState) {
        *self.overlay.lock().unpoison() = state.overlay;
    }

    /// Point the store at a new history root (called on checkpoint/reset).
    pub fn set_reader(&self, reader: Arc<dyn NativeHistoryReader>) {
        *self.reader.write().unpoison() = reader;
        self.has_history.store(true, Ordering::SeqCst);
    }

    /// Whether a backing history reader has been installed (AUDIT C109's invariant).
    ///
    /// [`Self::live_entries`] can enumerate the **overlay** and nothing else: `NativeHistoryReader`
    /// exposes `get_native` and no iteration, so a store with a history root holds values this cannot
    /// see. That is not a detail to paper over — a total that silently omits part of its subject is
    /// the failure this repository keeps finding — so [`Self::live_entries`]' only caller refuses to
    /// run when this is true. A store that cannot be enumerated is reported as such rather than
    /// summed as though it had been.
    pub fn has_base_history(&self) -> bool {
        self.has_history.load(Ordering::SeqCst)
    }

    /// Every live `(key, value)` in the overlay under `prefix`, tombstones excluded.
    ///
    /// The overlay is the whole state for a store that never took a checkpoint
    /// ([`Self::empty`]), which is the store the conservation tests build. See
    /// [`Self::has_base_history`] for what this cannot see and why its caller must check.
    pub fn live_entries(&self, prefix: u8) -> Vec<(Blake2b256Hash, Vec<u8>)> {
        self.overlay
            .lock()
            .unpoison()
            .iter()
            .filter_map(|(&(p, key), slot)| match (p == prefix, &slot.value) {
                (true, Some(v)) => Some((key, v.clone())),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_then_get_round_trips_without_reader() {
        let store = InMemNativeStore::empty();
        let key = Blake2b256Hash::from_bytes([7u8; 32]);
        store.put(PREFIX_POS, key, vec![1, 2, 3]);
        assert_eq!(
            store.get(PREFIX_POS, &key).await.unwrap(),
            Some(vec![1, 2, 3])
        );
    }

    /// `slot` is the identity a batch may hold once, so it must report the same thing for a `Put`
    /// and a `Delete` on one key — a mix the trie refuses exactly as it refuses two `Put`s, and the
    /// shape a merge has to collapse (issue #83). Both axes are checked, because a `slot` that
    /// ignored the prefix would merge two different stores' keys.
    #[test]
    fn a_slot_is_the_prefix_and_key_whatever_the_variant() {
        let key = Blake2b256Hash::from_bytes([7u8; 32]);
        let put = NativeStoreAction::Put {
            prefix: PREFIX_POS,
            key,
            value: vec![1],
        };
        let delete = NativeStoreAction::Delete {
            prefix: PREFIX_POS,
            key,
        };
        assert_eq!(put.slot(), (PREFIX_POS, key));
        assert_eq!(delete.slot(), (PREFIX_POS, key));
        assert_eq!(
            put.slot(),
            delete.slot(),
            "a write and a delete on one key are one slot, not two"
        );
        let other_prefix = NativeStoreAction::Delete {
            prefix: PREFIX_VAULT,
            key,
        };
        assert_ne!(
            put.slot(),
            other_prefix.slot(),
            "the prefix is part of the identity: the same key under two prefixes is two slots"
        );
    }

    #[tokio::test]
    async fn drain_changes_produces_put_actions() {
        let store = InMemNativeStore::empty();
        let key = Blake2b256Hash::from_bytes([8u8; 32]);
        store.put(PREFIX_REGISTRY, key, vec![9]);
        let changes = store.drain_changes();
        assert_eq!(
            changes,
            vec![NativeStoreAction::Put {
                prefix: PREFIX_REGISTRY,
                key,
                value: vec![9],
            }]
        );
        // Overlay is cleared; a read falls through to the (no-op) reader.
        assert_eq!(store.get(PREFIX_REGISTRY, &key).await.unwrap(), None);
    }

    #[tokio::test]
    async fn delete_records_tombstone_and_blocks_reader() {
        let store = InMemNativeStore::empty();
        let key = Blake2b256Hash::from_bytes([9u8; 32]);
        store.put(PREFIX_VAULT, key, vec![1]);
        store.delete(PREFIX_VAULT, &key);
        assert_eq!(store.get(PREFIX_VAULT, &key).await.unwrap(), None);
        let changes = store.drain_changes();
        assert_eq!(
            changes,
            vec![NativeStoreAction::Delete {
                prefix: PREFIX_VAULT,
                key
            }]
        );
    }

    /// **A drained write names the deploy that made it, and cost accounting does not take it away**
    /// (#280).
    ///
    /// This is the provenance the merge's native relation is built on, and the one thing the flat
    /// sidecar this replaced could not say. The second half is the C207 rule — cost accounting writes
    /// `pos:vault` from every deploy, and if a later charge took ownership of a slot the deploy had
    /// written, the deploy's own write would travel as cost accounting's and never reach a sidecar.
    #[tokio::test]
    async fn a_drained_write_names_the_deploy_that_made_it() {
        let store = InMemNativeStore::empty();
        let key = Blake2b256Hash::from_bytes([11u8; 32]);
        store.begin_writer(NativeWriter::Deploy(3));
        store.put(PREFIX_POS, key, vec![1]);
        store.end_writer();
        // A later charge on the same slot: the slot stays the deploy's.
        store.begin_writer(NativeWriter::CostAccounting);
        store.put(PREFIX_POS, key, vec![2]);
        store.end_writer();

        let drain = store.drain_native();
        assert_eq!(
            drain.by_deploy.get(&3).map(Vec::len),
            Some(1),
            "the slot is attributed to deploy 3: {drain:?}"
        );
        assert_eq!(
            drain.cost.len(),
            1,
            "the later charge keeps its own provenance without stealing the deploy's write: {drain:?}"
        );
        assert!(
            drain.unattributed.is_empty(),
            "and it is not lost: {drain:?}"
        );
        assert_eq!(
            drain.all().len(),
            1,
            "the checkpoint folds it exactly once, whatever its writer: {drain:?}"
        );
    }

    /// Two deploys may write the same native slot in one block.  The checkpoint needs only the final
    /// value, but merge attribution must retain both deploy-local values so rejecting the later chain
    /// cannot erase the earlier accepted transition.
    #[test]
    fn two_deploys_writing_one_slot_keep_their_own_writes() {
        let store = InMemNativeStore::empty();
        let key = Blake2b256Hash::from_bytes([14u8; 32]);

        store.begin_writer(NativeWriter::Deploy(0));
        store.put(PREFIX_POS, key, vec![1]);
        store.end_writer();

        store.begin_writer(NativeWriter::Deploy(1));
        store.put(PREFIX_POS, key, vec![2]);
        store.end_writer();

        let drain = store.drain_native();
        assert_eq!(
            drain.by_deploy.get(&0),
            Some(&vec![NativeStoreAction::Put {
                prefix: PREFIX_POS,
                key,
                value: vec![1]
            }]),
            "deploy 0's transition must survive attribution: {drain:?}"
        );
        assert_eq!(
            drain.by_deploy.get(&1),
            Some(&vec![NativeStoreAction::Put {
                prefix: PREFIX_POS,
                key,
                value: vec![2]
            }]),
            "deploy 1 keeps its own later transition: {drain:?}"
        );
        assert_eq!(
            drain.all(),
            vec![NativeStoreAction::Put {
                prefix: PREFIX_POS,
                key,
                value: vec![2]
            }],
            "the checkpoint still folds exactly one final action per slot"
        );
    }

    /// **A write outside any window is separated, and cannot be filed under the block** (#280).
    ///
    /// The arm exists for genesis installation and for direct store use, and the block path refuses it:
    /// "the block's own native set" is the representation the defect lived in, so the type has no way
    /// to hold one. The refusal is asserted here at the store's own boundary — the drain keeps it
    /// apart, and `BlockNativeEffects::from_drain` is what turns it into an error.
    #[tokio::test]
    async fn a_write_outside_any_window_is_not_attributed_to_a_block() {
        let store = InMemNativeStore::empty();
        store.put(PREFIX_POS, Blake2b256Hash::from_bytes([12u8; 32]), vec![1]);
        let drain = store.drain_native();
        assert!(
            drain.by_deploy.is_empty(),
            "no deploy made it, so no deploy's map holds it: {drain:?}"
        );
        assert_eq!(drain.unattributed.len(), 1, "it is kept apart: {drain:?}");
        assert!(
            BlockNativeEffects::from_drain(&drain).is_err(),
            "and a block cannot carry it — filing it under the block is the defect: {drain:?}"
        );
    }

    /// The genesis installation is a **named** writer rather than an unattributed one, and it does not
    /// travel in a sidecar (`NativeWriter::Genesis` says why): a merge never asks block 0 for its
    /// native set, because block 0 is the merge's base and not one of its conflict blocks.
    #[tokio::test]
    async fn a_genesis_write_is_named_and_does_not_travel_in_a_sidecar() {
        let store = InMemNativeStore::empty();
        store.begin_writer(NativeWriter::Genesis);
        store.put(PREFIX_POS, Blake2b256Hash::from_bytes([13u8; 32]), vec![1]);
        store.end_writer();
        let drain = store.drain_native();
        assert_eq!(drain.genesis.len(), 1, "named, not unattributed: {drain:?}");
        assert!(drain.unattributed.is_empty(), "{drain:?}");
        assert_eq!(
            drain.all().len(),
            1,
            "and the checkpoint still folds it: {drain:?}"
        );
        let effects = BlockNativeEffects::from_drain(&drain).expect("genesis is not refused");
        assert!(
            effects.is_empty(),
            "and it does **not** travel in a sidecar: what it writes is the block-0 configuration, \
             already in the state every node holds, and a merge asks the genesis for its *state* (it \
             is the base) rather than for its native set"
        );
    }

    #[tokio::test]
    async fn snapshot_and_revert_restore_overlay() {
        let store = InMemNativeStore::empty();
        let key = Blake2b256Hash::from_bytes([10u8; 32]);
        store.put(PREFIX_POS, key, vec![1]);
        let snap = store.snapshot();
        store.put(PREFIX_POS, key, vec![2]);
        assert_eq!(store.get(PREFIX_POS, &key).await.unwrap(), Some(vec![2]));
        store.revert(snap);
        assert_eq!(store.get(PREFIX_POS, &key).await.unwrap(), Some(vec![1]));
    }
}
