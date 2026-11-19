//! The in-memory hot store overlay over a history snapshot.
//!
//! Mirrors `rspace/src/main/scala/coop/rchain/rspace/HotStore.scala`. Reads from the history store
//! are memoized per key in a `HistoryStoreCache` (the Scala `Deferred`-backed cache), so concurrent
//! readers of the same key share a single back-fill and readers of different keys do not serialize.
//!
//! The overlay state is *striped*: `SHARDS` per-key mutexes, each holding a `HotStoreState`
//! partition. Data and joins shard by the Law 7 stable hash of their channel
//! (`hash_channel`); continuations and installed continuations shard by the stable hash of their
//! channel set (`hash_channels`). Every per-key operation touches exactly one shard, so unrelated
//! channels proceed concurrently; the aggregate readers (`changes`/`to_map`/`snapshot`) take the
//! shard locks one at a time in index order and merge, preserving the single-map semantics (and,
//! for `changes`, the exact global key order of the unstrided store).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use rchain_shared::serialize::Serialize;
use tokio::sync::{Mutex, OnceCell};

use crate::errors::RSpaceError;
use crate::hashing::stable_hash_provider::{hash_channel, hash_channels};
use crate::history::history_reader::HistoryReaderBase;
use crate::hot_store_action::HotStoreAction;
use crate::internal::{Datum, Row, WaitingContinuation};

/// The number of hot-store state shards. A power of two so channel hashes spread evenly; the
/// shard count scales with cores, so 64 covers 8-core dev machines and CI without contention.
pub const SHARDS: usize = 64;

/// The shard a 32-byte hash maps to (its first 8 bytes as a little-endian word, modulo the shard
/// count — the hash is uniform, so any fixed window spreads evenly).
fn shard_of(bytes: &[u8; 32], shards: usize) -> usize {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[..8]);
    // `max(1)`: a zero shard count is clamped to the single (unstrided) shard rather than panicking.
    (u64::from_le_bytes(word) as usize) % shards.max(1)
}

/// The shard of a single channel: the stable hash of its serialized form (Law 7).
fn channel_shard<C: Serialize<C>>(channel: &C, shards: usize) -> usize {
    shard_of(hash_channel(channel).as_bytes(), shards)
}

/// The shard of a channel set (a join key): the stable hash of its sorted channel hashes (Law 7).
fn channels_shard<C: Serialize<C>>(channels: &[C], shards: usize) -> usize {
    shard_of(hash_channels(channels).as_bytes(), shards)
}

/// The hot-store overlay state (port of `HotStoreState`), reused as the payload of one state
/// shard: each shard owns a partition of the five maps.
#[derive(Clone, Debug)]
pub struct HotStoreState<C, P, A, K> {
    pub continuations: BTreeMap<Vec<C>, Vec<WaitingContinuation<P, K>>>,
    pub installed_continuations: BTreeMap<Vec<C>, WaitingContinuation<P, K>>,
    pub data: BTreeMap<C, Vec<Datum<A>>>,
    pub joins: BTreeMap<C, Vec<Vec<C>>>,
    pub installed_joins: BTreeMap<C, Vec<Vec<C>>>,
}

impl<C, P, A, K> Default for HotStoreState<C, P, A, K> {
    fn default() -> Self {
        HotStoreState {
            continuations: BTreeMap::new(),
            installed_continuations: BTreeMap::new(),
            data: BTreeMap::new(),
            joins: BTreeMap::new(),
            installed_joins: BTreeMap::new(),
        }
    }
}

fn remove_index<E: Clone>(col: &[E], index: usize) -> Vec<E> {
    let mut out = col.to_vec();
    out.remove(index);
    out
}

/// Memoized history-store reads (port of `HistoryStoreCache`).
struct HistoryStoreCache<C, P, A, K> {
    continuations: BTreeMap<Vec<C>, Arc<OnceCell<Vec<WaitingContinuation<P, K>>>>>,
    datums: BTreeMap<C, Arc<OnceCell<Vec<Datum<A>>>>>,
    joins: BTreeMap<C, Arc<OnceCell<Vec<Vec<C>>>>>,
}

impl<C, P, A, K> Default for HistoryStoreCache<C, P, A, K> {
    fn default() -> Self {
        HistoryStoreCache {
            continuations: BTreeMap::new(),
            datums: BTreeMap::new(),
            joins: BTreeMap::new(),
        }
    }
}

/// The hot store interface (port of `HotStore[F]`).
#[async_trait]
pub trait HotStore<C, P, A, K>: Send + Sync {
    async fn get_continuations(
        &self,
        channels: &[C],
    ) -> Result<Vec<WaitingContinuation<P, K>>, RSpaceError>;
    async fn put_continuation(
        &self,
        channels: &[C],
        wc: WaitingContinuation<P, K>,
    ) -> Result<(), RSpaceError>;
    /// Install the persistent continuation a channel matches against. `Err` when the channel already
    /// carries a **different** one — see [`RSpaceError::InstallConflict`] and Law 60.
    async fn install_continuation(
        &self,
        channels: &[C],
        wc: WaitingContinuation<P, K>,
    ) -> Result<(), RSpaceError>;
    async fn remove_continuation(&self, channels: &[C], index: usize) -> Result<(), RSpaceError>;

    async fn get_data(&self, channel: &C) -> Result<Vec<Datum<A>>, RSpaceError>;
    async fn put_datum(&self, channel: &C, datum: Datum<A>) -> Result<(), RSpaceError>;
    async fn remove_datum(&self, channel: &C, index: i64) -> Result<(), RSpaceError>;

    async fn get_joins(&self, channel: &C) -> Result<Vec<Vec<C>>, RSpaceError>;
    async fn put_join(&self, channel: &C, join: &[C]) -> Result<(), RSpaceError>;
    async fn install_join(&self, channel: &C, join: &[C]);
    async fn remove_join(&self, channel: &C, join: &[C]) -> Result<(), RSpaceError>;

    async fn changes(&self) -> Vec<HotStoreAction<C, P, A, K>>;
    async fn to_map(&self) -> BTreeMap<Vec<C>, Row<P, A, K>>;
    async fn snapshot(&self) -> HotStoreState<C, P, A, K>;
}

/// The in-memory hot store (port of `InMemHotStore`), with the overlay state striped across
/// `SHARDS` shards (see the module docs). The per-shard maps are `HotStoreState`s; `with_shards`
/// selects the count (1 = the unstrided behavior, used by the equality test).
pub struct InMemHotStore<C, P, A, K> {
    state: Vec<Mutex<HotStoreState<C, P, A, K>>>,
    cache: Mutex<HistoryStoreCache<C, P, A, K>>,
    reader_base: Arc<dyn HistoryReaderBase<C, P, A, K>>,
}

impl<C, P, A, K> InMemHotStore<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
    A: Clone + Send + Sync + 'static,
    K: Clone + Send + Sync + 'static,
{
    pub fn new(reader_base: Arc<dyn HistoryReaderBase<C, P, A, K>>) -> Self {
        Self::with_shards(reader_base, SHARDS)
    }

    /// Build the store with `shards` state shards (clamped to ≥ 1; `1` reproduces the unstrided
    /// store exactly, which the equality test relies on).
    pub fn with_shards(reader_base: Arc<dyn HistoryReaderBase<C, P, A, K>>, shards: usize) -> Self {
        let shards = shards.max(1);
        InMemHotStore {
            state: (0..shards)
                .map(|_| Mutex::new(HotStoreState::default()))
                .collect(),
            cache: Mutex::new(HistoryStoreCache::default()),
            reader_base,
        }
    }

    pub fn from_state(
        state: HotStoreState<C, P, A, K>,
        reader_base: Arc<dyn HistoryReaderBase<C, P, A, K>>,
    ) -> Self {
        Self::from_state_with_shards(state, reader_base, SHARDS)
    }

    /// Build the store from a snapshot, distributing its entries across `shards` state shards by
    /// each key's stable hash (the inverse of `snapshot`).
    pub fn from_state_with_shards(
        state: HotStoreState<C, P, A, K>,
        reader_base: Arc<dyn HistoryReaderBase<C, P, A, K>>,
        shards: usize,
    ) -> Self {
        let shards = shards.max(1);
        let mut shard_states: Vec<HotStoreState<C, P, A, K>> =
            (0..shards).map(|_| HotStoreState::default()).collect();
        for (channels, wcs) in state.continuations {
            shard_states[channels_shard(&channels, shards)]
                .continuations
                .insert(channels, wcs);
        }
        for (channels, wc) in state.installed_continuations {
            shard_states[channels_shard(&channels, shards)]
                .installed_continuations
                .insert(channels, wc);
        }
        for (channel, data) in state.data {
            shard_states[channel_shard(&channel, shards)]
                .data
                .insert(channel, data);
        }
        for (channel, joins) in state.joins {
            shard_states[channel_shard(&channel, shards)]
                .joins
                .insert(channel, joins);
        }
        for (channel, joins) in state.installed_joins {
            shard_states[channel_shard(&channel, shards)]
                .installed_joins
                .insert(channel, joins);
        }
        InMemHotStore {
            state: shard_states.into_iter().map(Mutex::new).collect(),
            cache: Mutex::new(HistoryStoreCache::default()),
            reader_base,
        }
    }

    async fn get_cont_from_history_store(
        &self,
        channels: &[C],
    ) -> Result<Vec<WaitingContinuation<P, K>>, RSpaceError> {
        let cell = {
            let mut cache = self.cache.lock().await;
            cache
                .continuations
                .entry(channels.to_vec())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let reader_base = self.reader_base.clone();
        let channels = channels.to_vec();
        let was_cached = cell.get().is_some();
        let result = cell
            .get_or_try_init(|| async move { reader_base.get_continuations(&channels).await })
            .await?;
        crate::read_path_census::record_history_read(was_cached);
        Ok(result.clone())
    }

    async fn get_data_from_history_store(&self, channel: &C) -> Result<Vec<Datum<A>>, RSpaceError> {
        let cell = {
            let mut cache = self.cache.lock().await;
            cache
                .datums
                .entry(channel.clone())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let reader_base = self.reader_base.clone();
        let channel = channel.clone();
        let was_cached = cell.get().is_some();
        let result = cell
            .get_or_try_init(|| async move { reader_base.get_data(&channel).await })
            .await?;
        crate::read_path_census::record_history_read(was_cached);
        Ok(result.clone())
    }

    async fn get_joins_from_history_store(&self, channel: &C) -> Result<Vec<Vec<C>>, RSpaceError> {
        let cell = {
            let mut cache = self.cache.lock().await;
            cache
                .joins
                .entry(channel.clone())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let reader_base = self.reader_base.clone();
        let channel = channel.clone();
        let was_cached = cell.get().is_some();
        let result = cell
            .get_or_try_init(|| async move { reader_base.get_joins(&channel).await })
            .await?;
        crate::read_path_census::record_history_read(was_cached);
        Ok(result.clone())
    }
}

#[async_trait]
impl<C, P, A, K> HotStore<C, P, A, K> for InMemHotStore<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
    A: Clone + Send + Sync + 'static,
    K: Clone + Send + Sync + 'static,
{
    async fn get_continuations(
        &self,
        channels: &[C],
    ) -> Result<Vec<WaitingContinuation<P, K>>, RSpaceError> {
        let from_history = self.get_cont_from_history_store(channels).await?;
        let mut state = self.state[channels_shard(channels, self.state.len())]
            .lock()
            .await;
        Ok(match state.continuations.get(channels) {
            Some(conts) => {
                let mut out = Vec::new();
                if let Some(installed) = state.installed_continuations.get(channels) {
                    out.push(installed.clone());
                }
                out.extend(conts.clone());
                out
            }
            None => {
                state
                    .continuations
                    .insert(channels.to_vec(), from_history.clone());
                let mut out = Vec::new();
                if let Some(installed) = state.installed_continuations.get(channels) {
                    out.push(installed.clone());
                }
                out.extend(from_history);
                out
            }
        })
    }

    async fn put_continuation(
        &self,
        channels: &[C],
        wc: WaitingContinuation<P, K>,
    ) -> Result<(), RSpaceError> {
        let from_history = self.get_cont_from_history_store(channels).await?;
        let mut state = self.state[channels_shard(channels, self.state.len())]
            .lock()
            .await;
        let cur = state
            .continuations
            .entry(channels.to_vec())
            .or_insert(from_history);
        cur.insert(0, wc);
        Ok(())
    }

    /// Install the persistent continuation a channel matches against.
    ///
    /// **A channel carries one, and an install that would replace a different one is refused here
    /// rather than dropped** (AUDIT C219; the law is Law 60). The map holds one
    /// `WaitingContinuation` per channel, so before this a second install simply overwrote the first:
    /// a minted vault handle installed `balance` at arity 2 and then `transfer` at arity 5, and only
    /// `transfer` ever matched — with no error, because a send that finds no continuation is what an
    /// unmatched receive has always been. An *identical* re-install stays a no-op: `locked_install`
    /// skips that case before reaching here (play and replay both install the system contracts over
    /// one store), and the guard keeps this method honest on its own.
    async fn install_continuation(
        &self,
        channels: &[C],
        wc: WaitingContinuation<P, K>,
    ) -> Result<(), RSpaceError> {
        let mut state = self.state[channels_shard(channels, self.state.len())]
            .lock()
            .await;
        if let Some(existing) = state.installed_continuations.get(channels) {
            if existing.source == wc.source {
                return Ok(());
            }
            return Err(RSpaceError::InstallConflict(
                "a contract that serves several methods installs one continuation at \
                 `arity: 1, remainder: true` and dispatches on the method inside it",
            ));
        }
        state.installed_continuations.insert(channels.to_vec(), wc);
        Ok(())
    }

    async fn remove_continuation(&self, channels: &[C], index: usize) -> Result<(), RSpaceError> {
        let from_history = self.get_cont_from_history_store(channels).await?;
        let mut state = self.state[channels_shard(channels, self.state.len())]
            .lock()
            .await;
        let is_installed = state.installed_continuations.contains_key(channels);
        if is_installed && index == 0 {
            // Attempted to remove the installed continuation — skip.
            return Ok(());
        }
        let removed_index = if is_installed { index - 1 } else { index };
        let cur = state
            .continuations
            .entry(channels.to_vec())
            .or_insert(from_history);
        if removed_index < cur.len() {
            *cur = remove_index(cur, removed_index);
        }
        Ok(())
    }

    async fn get_data(&self, channel: &C) -> Result<Vec<Datum<A>>, RSpaceError> {
        let from_history = self.get_data_from_history_store(channel).await?;
        let mut state = self.state[channel_shard(channel, self.state.len())]
            .lock()
            .await;
        Ok(match state.data.get(channel) {
            Some(data) => data.clone(),
            None => {
                state.data.insert(channel.clone(), from_history.clone());
                from_history
            }
        })
    }

    async fn put_datum(&self, channel: &C, datum: Datum<A>) -> Result<(), RSpaceError> {
        let from_history = self.get_data_from_history_store(channel).await?;
        let mut state = self.state[channel_shard(channel, self.state.len())]
            .lock()
            .await;
        let cur = state.data.entry(channel.clone()).or_insert(from_history);
        cur.insert(0, datum);
        Ok(())
    }

    async fn remove_datum(&self, channel: &C, index: i64) -> Result<(), RSpaceError> {
        let from_history = self.get_data_from_history_store(channel).await?;
        let mut state = self.state[channel_shard(channel, self.state.len())]
            .lock()
            .await;
        let cur = state.data.entry(channel.clone()).or_insert(from_history);
        if index >= 0 && (index as usize) < cur.len() {
            *cur = remove_index(cur, index as usize);
        }
        Ok(())
    }

    async fn get_joins(&self, channel: &C) -> Result<Vec<Vec<C>>, RSpaceError> {
        let from_history = self.get_joins_from_history_store(channel).await?;
        let mut state = self.state[channel_shard(channel, self.state.len())]
            .lock()
            .await;
        Ok(match state.joins.get(channel) {
            Some(joins) => {
                let mut out = state
                    .installed_joins
                    .get(channel)
                    .cloned()
                    .unwrap_or_default();
                out.extend(joins.clone());
                out
            }
            None => {
                state.joins.insert(channel.clone(), from_history.clone());
                let mut out = state
                    .installed_joins
                    .get(channel)
                    .cloned()
                    .unwrap_or_default();
                out.extend(from_history);
                out
            }
        })
    }

    async fn put_join(&self, channel: &C, join: &[C]) -> Result<(), RSpaceError> {
        let from_history = self.get_joins_from_history_store(channel).await?;
        let mut state = self.state[channel_shard(channel, self.state.len())]
            .lock()
            .await;
        let cur = state.joins.entry(channel.clone()).or_insert(from_history);
        if !cur.contains(&join.to_vec()) {
            cur.insert(0, join.to_vec());
        }
        Ok(())
    }

    async fn install_join(&self, channel: &C, join: &[C]) {
        let mut state = self.state[channel_shard(channel, self.state.len())]
            .lock()
            .await;
        let cur = state.installed_joins.entry(channel.clone()).or_default();
        if !cur.contains(&join.to_vec()) {
            cur.insert(0, join.to_vec());
        }
    }

    async fn remove_join(&self, channel: &C, join: &[C]) -> Result<(), RSpaceError> {
        let from_history = self.get_joins_from_history_store(channel).await?;
        let mut state = self.state[channel_shard(channel, self.state.len())]
            .lock()
            .await;
        let cur = state.joins.entry(channel.clone()).or_insert(from_history);
        if let Some(index) = cur.iter().position(|j| j == join) {
            *cur = remove_index(cur, index);
        }
        Ok(())
    }

    async fn changes(&self) -> Vec<HotStoreAction<C, P, A, K>> {
        // Collect from every shard (one lock at a time, index order — the aggregate readers are
        // the only ones that touch more than one shard, so this order can never deadlock), then
        // re-sort per map to reproduce the unstrided store's exact global key order.
        let mut continuations: Vec<(Vec<C>, Vec<WaitingContinuation<P, K>>)> = Vec::new();
        let mut data: Vec<(C, Vec<Datum<A>>)> = Vec::new();
        let mut joins: Vec<(C, Vec<Vec<C>>)> = Vec::new();
        for shard in &self.state {
            let state = shard.lock().await;
            continuations.extend(
                state
                    .continuations
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
            data.extend(state.data.iter().map(|(k, v)| (k.clone(), v.clone())));
            joins.extend(state.joins.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        continuations.sort_by(|a, b| a.0.cmp(&b.0));
        data.sort_by(|a, b| a.0.cmp(&b.0));
        joins.sort_by(|a, b| a.0.cmp(&b.0));

        let mut out = Vec::with_capacity(continuations.len() + data.len() + joins.len());
        for (k, v) in continuations {
            if v.is_empty() {
                out.push(HotStoreAction::DeleteContinuations(k));
            } else {
                out.push(HotStoreAction::InsertContinuations(k, v));
            }
        }
        for (k, v) in data {
            if v.is_empty() {
                out.push(HotStoreAction::DeleteData(k));
            } else {
                out.push(HotStoreAction::InsertData(k, v));
            }
        }
        for (k, v) in joins {
            if v.is_empty() {
                out.push(HotStoreAction::DeleteJoins(k));
            } else {
                out.push(HotStoreAction::InsertJoins(k, v));
            }
        }
        out
    }

    async fn to_map(&self) -> BTreeMap<Vec<C>, Row<P, A, K>> {
        // The output BTreeMap orders by key, so per-shard insertion order is irrelevant: apply the
        // unstrided per-shard body and let the map re-sort the merged keys.
        let mut out: BTreeMap<Vec<C>, Row<P, A, K>> = BTreeMap::new();
        for shard in &self.state {
            let state = shard.lock().await;
            for (k, v) in &state.data {
                out.entry(vec![k.clone()]).or_default().data = v.clone();
            }
            for (k, v) in &state.continuations {
                out.entry(k.clone()).or_default().wks.extend(v.clone());
            }
            for (k, v) in &state.installed_continuations {
                out.entry(k.clone()).or_default().wks.insert(0, v.clone());
            }
        }
        out.retain(|_, row| !(row.data.is_empty() && row.wks.is_empty()));
        out
    }

    async fn snapshot(&self) -> HotStoreState<C, P, A, K> {
        // `from_state_with_shards` distributes by key, so merging shards in index order rebuilds
        // the snapshot regardless of which shard each key landed in.
        let mut merged = HotStoreState::default();
        for shard in &self.state {
            let state = shard.lock().await;
            merged.continuations.extend(state.continuations.clone());
            merged
                .installed_continuations
                .extend(state.installed_continuations.clone());
            merged.data.extend(state.data.clone());
            merged.joins.extend(state.joins.clone());
            merged.installed_joins.extend(state.installed_joins.clone());
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use rchain_shared::store_manager::InMemoryStoreManager;

    use crate::factory::create_history_repository;

    /// A store over a fresh in-memory history (empty base), with the given shard count.
    async fn test_store(shards: usize) -> InMemHotStore<String, String, String, String> {
        let manager = InMemoryStoreManager::default();
        let history =
            create_history_repository::<String, String, String, String>(&manager, "hot-store-test")
                .await
                .expect("history repository");
        let reader = history.get_history_reader(history.root()).await;
        InMemHotStore::with_shards(reader.base(), shards)
    }

    fn datum(channel: &str, a: &str) -> Datum<String> {
        Datum::create(&channel.to_string(), a.to_string(), false)
    }

    fn continuation(channels: &[&str]) -> WaitingContinuation<String, String> {
        WaitingContinuation::create(
            &channels.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
            vec![],
            "k".to_string(),
            false,
            BTreeSet::new(),
        )
    }

    /// **A channel carries one installed continuation, and a different second one is refused**
    /// (AUDIT C219; the law is Law 60).
    ///
    /// The map is keyed by the channel and holds one `WaitingContinuation`, so a second install used
    /// to **replace** the first silently — which is how a minted vault handle's `balance` arm came to
    /// have never existed while its `transfer` arm worked. The `Err` is the whole point: an install
    /// that cannot take effect says so rather than dropping what was there.
    ///
    /// The *identical* re-install is asserted first, because play and replay both install the system
    /// contracts over one store and that path has to stay a no-op rather than becoming a refusal.
    #[tokio::test]
    async fn a_second_different_install_on_a_channel_is_refused() {
        let store = test_store(1).await;
        let channels = vec!["a".to_string(), "b".to_string()];
        store
            .install_continuation(&channels, continuation(&["a", "b"]))
            .await
            .expect("the first install lands");
        store
            .install_continuation(&channels, continuation(&["a", "b"]))
            .await
            .expect("an identical re-install is a no-op, not a conflict");
        let other = WaitingContinuation::create(
            &channels,
            vec![],
            "other".to_string(),
            false,
            BTreeSet::new(),
        );
        let refused = store.install_continuation(&channels, other).await;
        assert!(
            matches!(refused, Err(RSpaceError::InstallConflict(_))),
            "a different second install on one channel must be refused, got {refused:?}"
        );
        let installed = store
            .get_continuations(&channels)
            .await
            .expect("read the continuation back");
        assert_eq!(
            installed.len(),
            1,
            "the refused install must not have replaced the one that was there: {installed:?}"
        );
    }

    /// **An install is not a checkpoint action.** `changes()` is exactly what
    /// `RSpace::create_checkpoint` folds into the next history root, and it reads `continuations`,
    /// `data` and `joins` — not `installed_continuations`/`installed_joins`. That is why a system
    /// process's definition can be installed without moving any block's post-state hash, and why a
    /// `reset` has to re-install them. Pinned because a future tidy-up that folded the install maps
    /// into `changes()` would silently turn every new system process into a state-hash change, and
    /// with it a replay divergence for every historical block.
    #[tokio::test]
    async fn an_install_is_not_a_checkpoint_action() {
        let store = test_store(1).await;
        let channels = vec!["a".to_string()];
        store
            .install_continuation(&channels, continuation(&["a"]))
            .await
            .expect("the install lands");
        store
            .install_join(&"a".to_string(), &["a".to_string()])
            .await;

        assert!(
            store.changes().await.is_empty(),
            "an install is in-memory only: it must not appear in the actions a checkpoint folds"
        );

        // The installs are still readable — they are omitted from `changes()`, not dropped.
        assert_eq!(
            store
                .get_continuations(&channels)
                .await
                .expect("read the installed continuation")
                .len(),
            1
        );
        assert_eq!(
            store
                .get_joins(&"a".to_string())
                .await
                .expect("read the installed join")
                .len(),
            1
        );
    }

    /// A scripted sequence covering every mutation, run against both stores. `f` awaits one op.
    async fn run_script(store: &InMemHotStore<String, String, String, String>) {
        store
            .put_datum(&"a".to_string(), datum("a", "d1"))
            .await
            .unwrap();
        store
            .put_datum(&"a".to_string(), datum("a", "d2"))
            .await
            .unwrap();
        store
            .put_datum(&"b".to_string(), datum("b", "d3"))
            .await
            .unwrap();
        store.remove_datum(&"a".to_string(), 1).await.unwrap();
        let _ = store.get_data(&"c".to_string()).await.unwrap(); // memoize an empty channel

        store
            .put_continuation(
                &["a".to_string(), "b".to_string()],
                continuation(&["a", "b"]),
            )
            .await
            .unwrap();
        store
            .put_continuation(&["a".to_string()], continuation(&["a"]))
            .await
            .unwrap();
        store
            .install_continuation(
                &["a".to_string(), "b".to_string()],
                continuation(&["a", "b"]),
            )
            .await
            .expect("the script installs on a channel that carries none");
        store
            .remove_continuation(&["a".to_string(), "b".to_string()], 0)
            .await
            .unwrap();
        let _ = store
            .get_continuations(&["d".to_string(), "e".to_string()])
            .await
            .unwrap(); // memoize an empty join

        store
            .put_join(&"a".to_string(), &["b".to_string(), "c".to_string()])
            .await
            .unwrap();
        store
            .put_join(&"b".to_string(), &["c".to_string()])
            .await
            .unwrap();
        store
            .install_join(&"b".to_string(), &["d".to_string()])
            .await;
        store
            .remove_join(&"b".to_string(), &["c".to_string()])
            .await
            .unwrap();
        let _ = store.get_joins(&"z".to_string()).await.unwrap(); // memoize an empty channel
    }

    /// The striped store (64 shards) is observably identical to the unstrided store (1 shard):
    /// same actions, same map, same snapshot.
    #[tokio::test]
    async fn striped_store_equals_unstriped() {
        let unstrided = test_store(1).await;
        let striped = test_store(SHARDS).await;

        run_script(&unstrided).await;
        run_script(&striped).await;

        assert_eq!(striped.changes().await, unstrided.changes().await);
        assert_eq!(striped.to_map().await, unstrided.to_map().await);
        let ss = striped.snapshot().await;
        let us = unstrided.snapshot().await;
        assert_eq!(ss.continuations, us.continuations);
        assert_eq!(ss.installed_continuations, us.installed_continuations);
        assert_eq!(ss.data, us.data);
        assert_eq!(ss.joins, us.joins);
        assert_eq!(ss.installed_joins, us.installed_joins);
    }

    /// A snapshot round-trips through `from_state_with_shards`: re-distributing and re-merging
    /// yields the same state, at any shard count.
    #[tokio::test]
    async fn snapshot_roundtrips_through_from_state() {
        let store = test_store(SHARDS).await;
        run_script(&store).await;
        let snapshot = store.snapshot().await;

        let manager = InMemoryStoreManager::default();
        let history = create_history_repository::<String, String, String, String>(
            &manager,
            "hot-store-test-2",
        )
        .await
        .expect("history repository");
        let reader = history.get_history_reader(history.root()).await;
        let rebuilt =
            InMemHotStore::from_state_with_shards(snapshot.clone(), reader.base(), SHARDS);
        assert_eq!(
            rebuilt.snapshot().await.continuations,
            snapshot.continuations
        );
        assert_eq!(rebuilt.snapshot().await.data, snapshot.data);
        assert_eq!(rebuilt.snapshot().await.joins, snapshot.joins);
    }

    /// Concurrent tasks on disjoint channels proceed without deadlock, and each channel's final
    /// state reflects exactly its own task's ops (stripe isolation).
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn striped_concurrent_stress() {
        let store = Arc::new(test_store(SHARDS).await);
        let mut tasks = Vec::new();
        for t in 0..8u32 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                let channel = format!("task-{t}");
                for i in 0..40 {
                    store
                        .put_datum(&channel, datum(&channel, &format!("v{i}")))
                        .await
                        .unwrap();
                    if i % 2 == 0 {
                        store
                            .put_join(&channel, &[format!("{channel}-j{i}")])
                            .await
                            .unwrap();
                    }
                    if i % 7 == 0 {
                        let _ = store.get_data(&channel).await.unwrap();
                        let _ = store.get_joins(&channel).await.unwrap();
                    }
                }
                for _ in 0..20 {
                    store.remove_datum(&channel, 0).await.unwrap();
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        for t in 0..8u32 {
            let channel = format!("task-{t}");
            assert_eq!(store.get_data(&channel).await.unwrap().len(), 20);
            assert_eq!(store.get_joins(&channel).await.unwrap().len(), 20);
        }
        // The aggregate readers still work after the storm.
        assert!(!store.changes().await.is_empty());
        assert_eq!(store.to_map().await.len(), 8);
        assert_eq!(store.snapshot().await.data.len(), 8);
    }
}
