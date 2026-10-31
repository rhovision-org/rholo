//! The "play" tuple space: consume/produce/install + checkpoint.
//!
//! Mirrors `rspace/src/main/scala/coop/rchain/rspace/{RSpaceOps,RSpace}.scala`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_shared::serialize::Serialize;

use crate::checkpoint::{Checkpoint, SoftCheckpoint};
use crate::concurrent::two_step_lock::TwoStepLock;
use crate::hashing::stable_hash_provider::hash_channel;
use crate::history::history_repository::HistoryRepository;
use crate::hot_store::{HotStore, InMemHotStore};
use crate::i_space::ISpace;
use crate::internal::{
    ConsumeCandidate, Datum, Install, ProduceCandidate, Row, WaitingContinuation,
};
use crate::match_::Match;
use crate::native_store::{InMemNativeStore, NativeDrain, NativeStoreAction, NativeStoreState};
use crate::replay_rspace::ReplayRSpace;
use crate::scheduled_space::{PendingProduce, ScheduledConsume, ScheduledProduce};
use crate::space_matcher::{extract_data_candidates, extract_first_match};
use crate::trace::event::{Comm, Consume, Event, Produce};
use crate::trace::Log;
use crate::tuple_space::{ContResult, Result, Tuplespace};

pub(crate) type MaybeActionResult<C, P, A, K> = std::result::Result<
    Option<(ContResult<C, P, K>, Vec<Result<C, A>>)>,
    crate::errors::RSpaceError,
>;
type MaybeProduceCandidate<C, P, A, K> =
    std::result::Result<Option<ProduceCandidate<C, P, A, K>>, crate::errors::RSpaceError>;

/// The play tuple space (port of `RSpace` / `RSpaceOps`).
pub struct RSpace<C, P, A, K> {
    history_repository: RwLock<Arc<HistoryRepository<C, P, A, K>>>,
    store: RwLock<Arc<dyn HotStore<C, P, A, K>>>,
    event_log: RwLock<Log>,
    produce_counter: RwLock<BTreeMap<Produce, usize>>,
    installs: RwLock<BTreeMap<Vec<C>, Install<P, K>>>,
    pub(crate) lock_f: Arc<TwoStepLock<Blake2b256Hash>>,
    matcher: Arc<dyn Match<P, A>>,
    native_store: Arc<InMemNativeStore>,
    /// The native state mutations folded into the most recent [`RSpace::create_checkpoint`].
    ///
    /// The checkpoint drains the native overlay into the trie, so the overlay no longer holds them
    /// afterwards; the block's post-state hash does, but a *merge* reconstructs tuple-space state
    /// from branch effects and has no way to recover a native leaf from the state alone. This is the
    /// per-checkpoint record a caller reads to carry native effects into the merge (issue #74).
    last_native_changes: RwLock<Vec<NativeStoreAction>>,
    /// The most recent drain, **attributed by the deploy that wrote each slot** — see
    /// [`RSpace::last_native_drain`].
    last_native_drain: RwLock<NativeDrain>,
}

impl<C, P, A, K> RSpace<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Serialize<P> + Send + Sync + 'static,
    A: Clone + Serialize<A> + Send + Sync + 'static,
    K: Clone + Serialize<K> + Send + Sync + 'static,
{
    pub fn new(
        history_repository: Arc<HistoryRepository<C, P, A, K>>,
        store: Arc<dyn HotStore<C, P, A, K>>,
        matcher: Arc<dyn Match<P, A>>,
    ) -> Self {
        Self::with_native_store(
            history_repository,
            store,
            matcher,
            Arc::new(InMemNativeStore::empty()),
        )
    }

    /// The same space with the **native store supplied** — the shape a *fork* is built in, and the
    /// difference matters.
    ///
    /// A fork is made *at* a root, and a read against it must reach that root's native state: the
    /// system contracts read bonds and balances through the native store, and
    /// `InMemNativeStore::empty()` is a `NoopNativeReader` whose reads answer `None` until a reader is
    /// set. `ISpace::reset` is what sets one — so a space built empty is only usable *after* a reset,
    /// which is exactly why the read path reset each fork and paid a second system-process install
    /// into the store that reset replaced (issue #364).
    ///
    /// Building the fork with its reader instead makes that reset the identity by Law 77
    /// (`spec/Rchain/Reset.lean`: a reset to the root a view already names is a projection, so an
    /// install made before it is unobservable), which is what lets the read paths drop it.
    pub fn with_native_store(
        history_repository: Arc<HistoryRepository<C, P, A, K>>,
        store: Arc<dyn HotStore<C, P, A, K>>,
        matcher: Arc<dyn Match<P, A>>,
        native_store: Arc<InMemNativeStore>,
    ) -> Self {
        RSpace {
            history_repository: RwLock::new(history_repository),
            store: RwLock::new(store),
            event_log: RwLock::new(Vec::new()),
            produce_counter: RwLock::new(BTreeMap::new()),
            installs: RwLock::new(BTreeMap::new()),
            lock_f: Arc::new(TwoStepLock::new()),
            matcher,
            native_store,
            last_native_changes: RwLock::new(Vec::new()),
            last_native_drain: RwLock::new(NativeDrain::default()),
        }
    }

    /// Create both the play and replay spaces over the same store (port of
    /// `RSpace.createWithReplay`).
    pub fn create_with_replay(
        history_repository: Arc<HistoryRepository<C, P, A, K>>,
        store: Arc<dyn HotStore<C, P, A, K>>,
        matcher: Arc<dyn Match<P, A>>,
    ) -> (Arc<RSpace<C, P, A, K>>, ReplayRSpace<C, P, A, K>) {
        Self::create_with_replay_and_native_store(
            history_repository,
            store,
            matcher,
            Arc::new(InMemNativeStore::empty()),
        )
    }

    /// `create_with_replay` with the native store supplied: the fork's shape, for the reason
    /// [`Self::with_native_store`] gives. A fork built this way is readable at its root without a
    /// reset, so the read path does not need the reset that would replace its store.
    pub fn create_with_replay_and_native_store(
        history_repository: Arc<HistoryRepository<C, P, A, K>>,
        store: Arc<dyn HotStore<C, P, A, K>>,
        matcher: Arc<dyn Match<P, A>>,
        native_store: Arc<InMemNativeStore>,
    ) -> (Arc<RSpace<C, P, A, K>>, ReplayRSpace<C, P, A, K>) {
        let play = Arc::new(RSpace::with_native_store(
            history_repository,
            store,
            matcher,
            native_store,
        ));
        let replay = ReplayRSpace::new(play.clone());
        (play, replay)
    }

    pub(crate) fn produce_counters(&self, produce_refs: &[Produce]) -> BTreeMap<Produce, usize> {
        let counter = crate::lock::rlock(&self.produce_counter);
        produce_refs
            .iter()
            .map(|p| (p.clone(), *counter.get(p).unwrap_or(&0)))
            .collect()
    }

    pub(crate) fn produce_counter_value(&self, source: &Produce) -> usize {
        crate::lock::rlock(&self.produce_counter)
            .get(source)
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn increment_produce_counter(&self, produce_ref: &Produce) {
        let mut counter = crate::lock::wlock(&self.produce_counter);
        *counter.entry(produce_ref.clone()).or_insert(0) += 1;
    }

    pub(crate) fn matcher(&self) -> Arc<dyn Match<P, A>> {
        self.matcher.clone()
    }

    /// The native system-contract store (shared with the reducer's system processes).
    pub fn native_store(&self) -> Arc<InMemNativeStore> {
        self.native_store.clone()
    }

    /// The native mutations folded into the most recent [`RSpace::create_checkpoint`].
    pub fn last_native_changes(&self) -> Vec<NativeStoreAction> {
        crate::lock::rlock(&self.last_native_changes).clone()
    }

    /// The same drain, **grouped by the deploy that wrote each slot** (#280).
    ///
    /// What a block's *sidecar* is built from, and the reason it is grouped rather than flat. Cost
    /// accounting writes `pos:vault` from every user deploy, and the merge re-derives those from the
    /// accepted deploys instead of carrying them (AUDIT C207), so they are separated out; a write made
    /// outside any deploy's window is separated out too, because a block **cannot attribute it** and
    /// the block path refuses it rather than filing it under the block.
    ///
    /// Replacement, not accumulation: a caller that needs a whole block merges this across the block's
    /// per-deploy checkpoints (`BlockNativeEffects::merge`).
    pub fn last_native_drain(&self) -> NativeDrain {
        crate::lock::rlock(&self.last_native_drain).clone()
    }

    pub(crate) fn current_store(&self) -> Arc<dyn HotStore<C, P, A, K>> {
        crate::lock::rlock(&self.store).clone()
    }

    fn current_history(&self) -> Arc<HistoryRepository<C, P, A, K>> {
        crate::lock::rlock(&self.history_repository).clone()
    }

    fn log_comm(&self, comm: Comm) -> Comm {
        crate::lock::wlock(&self.event_log).insert(0, Event::Comm(comm.clone()));
        comm
    }

    fn log_consume(&self, consume_ref: Consume) -> Consume {
        crate::lock::wlock(&self.event_log).insert(0, Event::Consume(consume_ref.clone()));
        consume_ref
    }

    pub(crate) fn log_produce(&self, produce_ref: Produce, persist: bool) -> Produce {
        crate::lock::wlock(&self.event_log).insert(0, Event::Produce(produce_ref.clone()));
        if !persist {
            let mut counter = crate::lock::wlock(&self.produce_counter);
            *counter.entry(produce_ref.clone()).or_insert(0) += 1;
        }
        produce_ref
    }

    async fn fetch_channel_to_index_data(
        &self,
        channels: &[C],
    ) -> std::result::Result<BTreeMap<C, Vec<(Datum<A>, i64)>>, crate::errors::RSpaceError> {
        let store = self.current_store();
        let mut map = BTreeMap::new();
        for c in channels {
            let data = store.get_data(c).await?;
            let mut indexed: Vec<(Datum<A>, i64)> = data
                .into_iter()
                .enumerate()
                .map(|(i, d)| (d, i as i64))
                .collect();
            // Content-addressed selection: sort candidates by their produce hash so the sorted-first
            // matching datum is chosen regardless of insertion order (Law 4/8).
            indexed.sort_by(|a, b| a.0.source.cmp(&b.0.source));
            map.insert(c.clone(), indexed);
        }
        Ok(map)
    }

    pub(crate) async fn store_waiting_continuation(
        &self,
        channels: &[C],
        wc: WaitingContinuation<P, K>,
    ) -> MaybeActionResult<C, P, A, K> {
        let store = self.current_store();
        store.put_continuation(channels, wc).await?;
        for channel in channels {
            store.put_join(channel, channels).await?;
        }
        Ok(None)
    }

    pub(crate) async fn store_data(
        &self,
        channel: &C,
        data: A,
        persist: bool,
        produce_ref: Produce,
    ) -> MaybeActionResult<C, P, A, K> {
        let store = self.current_store();
        store
            .put_datum(
                channel,
                Datum {
                    a: data,
                    persist,
                    source: produce_ref,
                },
            )
            .await?;
        Ok(None)
    }

    pub(crate) async fn store_persistent_data(
        &self,
        data_candidates: &[ConsumeCandidate<C, A>],
    ) -> std::result::Result<(), crate::errors::RSpaceError> {
        let mut sorted = data_candidates.to_vec();
        sorted.sort_by(|a, b| b.datum_index.cmp(&a.datum_index));
        let store = self.current_store();
        for candidate in sorted {
            if !candidate.datum.persist {
                store
                    .remove_datum(&candidate.channel, candidate.datum_index)
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn remove_matched_datum_and_join(
        &self,
        channels: &[C],
        data_candidates: &[ConsumeCandidate<C, A>],
    ) -> std::result::Result<(), crate::errors::RSpaceError> {
        let mut sorted = data_candidates.to_vec();
        sorted.sort_by(|a, b| b.datum_index.cmp(&a.datum_index));
        let store = self.current_store();
        for candidate in sorted {
            if candidate.datum_index >= 0 && !candidate.datum.persist {
                store
                    .remove_datum(&candidate.channel, candidate.datum_index)
                    .await?;
            }
            store.remove_join(&candidate.channel, channels).await?;
        }
        Ok(())
    }

    pub(crate) fn wrap_result(
        &self,
        channels: &[C],
        wk: &WaitingContinuation<P, K>,
        data_candidates: &[ConsumeCandidate<C, A>],
    ) -> MaybeActionResult<C, P, A, K> {
        Ok(Some((
            ContResult {
                continuation: wk.continuation.clone(),
                persistent: wk.persist,
                channels: channels.to_vec(),
                patterns: wk.patterns.clone(),
                peek: !wk.peeks.is_empty(),
            },
            data_candidates
                .iter()
                .map(|dc| Result {
                    channel: dc.channel.clone(),
                    matched_datum: dc.datum.a.clone(),
                    removed_datum: dc.removed_datum.clone(),
                    persistent: dc.datum.persist,
                })
                .collect(),
        )))
    }

    pub(crate) async fn extract_produce_candidate(
        &self,
        grouped_channels: &[Vec<C>],
        bat_channel: &C,
        data: Datum<A>,
    ) -> MaybeProduceCandidate<C, P, A, K> {
        let store = self.current_store();
        for channels in grouped_channels {
            let conts = store.get_continuations(channels).await?;
            let match_candidates: Vec<(WaitingContinuation<P, K>, usize)> = conts
                .into_iter()
                .enumerate()
                .map(|(i, wc)| (wc, i))
                .collect();
            let mut channel_to_indexed_data = BTreeMap::new();
            for c in channels {
                let data_list = store.get_data(c).await?;
                let mut indexed: Vec<(Datum<A>, i64)> = data_list
                    .into_iter()
                    .enumerate()
                    .map(|(i, d)| (d, i as i64))
                    .collect();
                // Sort the stored data by content hash, then prepend the in-flight datum so it stays
                // the first candidate on its producing channel (the produce's own data).
                indexed.sort_by(|a, b| a.0.source.cmp(&b.0.source));
                if c == bat_channel {
                    indexed.insert(0, (data.clone(), -1));
                }
                channel_to_indexed_data.insert(c.clone(), indexed);
            }
            if let Some(pc) = extract_first_match(
                channels,
                &match_candidates,
                &channel_to_indexed_data,
                self.matcher.as_ref(),
            )? {
                return Ok(Some(pc));
            }
        }
        Ok(None)
    }

    pub(crate) async fn process_match_found(
        &self,
        pc: ProduceCandidate<C, P, A, K>,
    ) -> MaybeActionResult<C, P, A, K> {
        let ProduceCandidate {
            channels,
            continuation: wk,
            continuation_index,
            data_candidates,
        } = pc;
        let consume_ref = wk.source.clone();
        let comm = Comm::apply(&data_candidates, consume_ref, wk.peeks.clone(), |ps| {
            self.produce_counters(ps)
        });
        self.log_comm(comm);
        if !wk.persist {
            let store = self.current_store();
            store
                .remove_continuation(&channels, continuation_index)
                .await?;
            self.remove_matched_datum_and_join(&channels, &data_candidates)
                .await?;
        } else {
            // A persistent continuation stays installed, so its join records must stay too; only the
            // matched (non-persistent) data is consumed. Removing the joins here made a contract
            // unreachable after its first produce-side COMM (issues #21/#22).
            self.store_persistent_data(&data_candidates).await?;
        }
        self.wrap_result(&channels, &wk, &data_candidates)
    }

    pub(crate) async fn locked_consume(
        &self,
        channels: &[C],
        patterns: &[P],
        continuation: K,
        persist: bool,
        peeks: BTreeSet<usize>,
        consume_ref: Consume,
    ) -> MaybeActionResult<C, P, A, K> {
        self.log_consume(consume_ref.clone());
        let channel_to_indexed_data = self.fetch_channel_to_index_data(channels).await?;
        let options = extract_data_candidates(
            &channels
                .iter()
                .cloned()
                .zip(patterns.iter().cloned())
                .collect::<Vec<_>>(),
            &channel_to_indexed_data,
            self.matcher.as_ref(),
        )?;
        let wk = WaitingContinuation {
            patterns: patterns.to_vec(),
            continuation,
            persist,
            peeks: peeks.clone(),
            source: consume_ref.clone(),
        };
        match options.into_iter().collect::<Option<Vec<_>>>() {
            None => self.store_waiting_continuation(channels, wk).await,
            Some(data_candidates) => {
                let comm = Comm::apply(&data_candidates, consume_ref, peeks, |ps| {
                    self.produce_counters(ps)
                });
                self.log_comm(comm);
                self.store_persistent_data(&data_candidates).await?;
                self.wrap_result(channels, &wk, &data_candidates)
            }
        }
    }

    async fn locked_produce(
        &self,
        channel: C,
        data: A,
        persist: bool,
        produce_ref: Produce,
    ) -> MaybeActionResult<C, P, A, K> {
        let grouped_channels = self.current_store().get_joins(&channel).await?;
        self.log_produce(produce_ref.clone(), persist);
        let datum = Datum {
            a: data.clone(),
            persist,
            source: produce_ref.clone(),
        };
        let extracted = self
            .extract_produce_candidate(&grouped_channels, &channel, datum)
            .await?;
        match extracted {
            None => self.store_data(&channel, data, persist, produce_ref).await,
            Some(pc) => self.process_match_found(pc).await,
        }
    }

    async fn locked_install(
        &self,
        channels: &[C],
        patterns: &[P],
        continuation: K,
    ) -> std::result::Result<Option<(K, Vec<A>)>, crate::errors::RSpaceError> {
        let store = self.current_store();
        // Idempotent install: the play and replay runtimes are both created over the same store
        // and each installs the system contracts, so the second install must be a no-op rather
        // than a duplicate. Skipping an identical consume also keeps `install` from erroring on a
        // restored state that already carries data at a fixed channel.
        let consume_ref = Consume::apply(channels, patterns, &continuation, true);
        let existing = store.get_continuations(channels).await?;
        if existing.iter().any(|wc| wc.source == consume_ref) {
            return Ok(None);
        }
        let mut channel_to_indexed_data = BTreeMap::new();
        for c in channels {
            let data = store.get_data(c).await?;
            let indexed: Vec<(Datum<A>, i64)> = data
                .into_iter()
                .enumerate()
                .map(|(i, d)| (d, i as i64))
                .collect();
            channel_to_indexed_data.insert(c.clone(), indexed);
        }
        let options = extract_data_candidates(
            &channels
                .iter()
                .cloned()
                .zip(patterns.iter().cloned())
                .collect::<Vec<_>>(),
            &channel_to_indexed_data,
            self.matcher.as_ref(),
        )?;
        match options.into_iter().collect::<Option<Vec<_>>>() {
            None => {
                let consume_ref = Consume::apply(channels, patterns, &continuation, true);
                let wc = WaitingContinuation {
                    patterns: patterns.to_vec(),
                    continuation: continuation.clone(),
                    persist: true,
                    peeks: BTreeSet::new(),
                    source: consume_ref,
                };
                // **The store first**: an install that would replace a different continuation on the
                // channel is refused there (Law 60), and recording it in `installs` before that would
                // leave this space claiming a continuation the store never took.
                store.install_continuation(channels, wc).await?;
                crate::lock::wlock(&self.installs).insert(
                    channels.to_vec(),
                    Install {
                        patterns: patterns.to_vec(),
                        continuation: continuation.clone(),
                    },
                );
                for channel in channels {
                    store.install_join(channel, channels).await;
                }
                Ok(None)
            }
            Some(_) => Err(crate::errors::RSpaceError::InstallNotAllowed),
        }
    }
}

#[async_trait]
impl<C, P, A, K> Tuplespace<C, P, A, K> for RSpace<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Serialize<P> + Send + Sync + 'static,
    A: Clone + Serialize<A> + Send + Sync + 'static,
    K: Clone + Serialize<K> + Send + Sync + 'static,
{
    async fn consume(
        &self,
        channels: &[C],
        patterns: &[P],
        continuation: K,
        persist: bool,
        peeks: BTreeSet<usize>,
    ) -> MaybeActionResult<C, P, A, K> {
        assert!(!channels.is_empty(), "channels can't be empty");
        assert_eq!(
            channels.len(),
            patterns.len(),
            "channels.length must equal patterns.length"
        );
        let consume_ref = Consume::apply(channels, patterns, &continuation, persist);
        let hashes: Vec<Blake2b256Hash> = channels.iter().map(hash_channel).collect();
        let thunk = self.locked_consume(
            channels,
            patterns,
            continuation,
            persist,
            peeks,
            consume_ref,
        );
        self.lock_f
            .acquire(&hashes, Box::pin(async { Ok(hashes.clone()) }), thunk)
            .await?
    }

    async fn produce(&self, channel: C, data: A, persist: bool) -> MaybeActionResult<C, P, A, K> {
        let produce_ref = Produce::apply(&channel, &data, persist);
        let thunk = self.locked_produce(channel.clone(), data, persist, produce_ref);
        let phase_two = {
            let store = self.current_store();
            let channel = channel.clone();
            Box::pin(async move {
                Ok(store
                    .get_joins(&channel)
                    .await?
                    .into_iter()
                    .flatten()
                    .map(|c| hash_channel(&c))
                    .collect())
            })
        };
        let hash = hash_channel(&channel);
        self.lock_f.acquire(&[hash], phase_two, thunk).await?
    }

    async fn install(
        &self,
        channels: &[C],
        patterns: &[P],
        continuation: K,
    ) -> std::result::Result<Option<(K, Vec<A>)>, crate::errors::RSpaceError> {
        assert_eq!(
            channels.len(),
            patterns.len(),
            "channels.length must equal patterns.length"
        );
        let hashes: Vec<Blake2b256Hash> = channels.iter().map(hash_channel).collect();
        let thunk = self.locked_install(channels, patterns, continuation);
        self.lock_f
            .acquire(&hashes, Box::pin(async { Ok(hashes.clone()) }), thunk)
            .await?
    }

    async fn produce_at(
        &self,
        path: Vec<u16>,
        channel: C,
        data: A,
        persist: bool,
    ) -> std::result::Result<ScheduledProduce<C, P, A, K>, crate::errors::RSpaceError> {
        self.scheduled_produce_at(path, channel, data, persist)
            .await
    }

    async fn consume_at(
        &self,
        path: Vec<u16>,
        channels: &[C],
        patterns: &[P],
        continuation: K,
        persist: bool,
        peeks: BTreeSet<usize>,
    ) -> std::result::Result<ScheduledConsume<C, P, A, K>, crate::errors::RSpaceError> {
        self.scheduled_consume_at(path, channels, patterns, continuation, persist, peeks)
            .await
    }

    async fn commit_produce(&self, pending: PendingProduce<C, A>) -> MaybeActionResult<C, P, A, K> {
        self.scheduled_produce_commit(pending).await
    }
}

#[async_trait]
impl<C, P, A, K> ISpace<C, P, A, K> for RSpace<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Serialize<P> + Send + Sync + 'static,
    A: Clone + Serialize<A> + Send + Sync + 'static,
    K: Clone + Serialize<K> + Send + Sync + 'static,
{
    /// Fold the hot store and the native overlay into a new history root.
    ///
    /// **The root is recorded as known; `CURRENT_ROOT` does not move here** (law 80, B40, HAZOP
    /// F-U17-10). A checkpoint is written for a *candidate* state — a replay, a play not yet accepted
    /// — and publishing it was the defect: a block refused after its checkpoint left the durable
    /// pointer naming a post-state no accepted block carried. The block's acceptance publishes the
    /// root through `HistoryRepository::publish_if_known`.
    async fn create_checkpoint(&self) -> std::result::Result<Checkpoint, String> {
        let changes = self.current_store().changes().await;
        // **The overlay is not discarded until the fold that consumes it has succeeded** (law 74,
        // `spec/Rchain/Checkpoint.lean`). `drain_native` clears the overlay as it reads it, and the fold
        // below is the only fallible step — so without this a fold that returns `Err` would lose native
        // mutations it never committed, while the hot-store side (read above, not cleared) is left
        // untouched. The snapshot is the overlay as it stands and is restored if the fold fails; the
        // drain itself stays atomic, so the restore is the only place the overlay is written back.
        let before = self.native_store.snapshot();
        let drain = self.native_store.drain_native();
        let native_changes = drain.all();
        let next_history = {
            let history = self.current_history();
            match history
                .checkpoint_with_native_deferred(&changes, &native_changes)
                .await
            {
                Ok(next) => next,
                Err(e) => {
                    self.native_store.revert(before);
                    return Err(e);
                }
            }
        };
        // **The drain is recorded only once the fold that consumed it has committed.** Keep the drained
        // mutations for the caller (the block path folds them into the block index so the merge can
        // re-apply them; issue #74). Replacement, not accumulation: a caller that needs a whole block
        // accumulates across this and the block's checkpoints.
        *crate::lock::wlock(&self.last_native_changes) = native_changes.clone();
        // **And the same drain, attributed** (#280), which is what the block's sidecar is built from.
        // The checkpoint folds every action; only the by-deploy view travels in the sidecar, because
        // cost accounting's writes are re-derived by the merge (AUDIT C207) and a write outside any
        // deploy's window has no deploy to travel under.
        *crate::lock::wlock(&self.last_native_drain) = drain;
        *crate::lock::wlock(&self.history_repository) = next_history.clone();
        let log = std::mem::take(&mut *crate::lock::wlock(&self.event_log));
        *crate::lock::wlock(&self.produce_counter) = BTreeMap::new();
        let history_reader = next_history.get_history_reader(next_history.root()).await;
        let base = history_reader.base();
        *crate::lock::wlock(&self.store) = Arc::new(InMemHotStore::new(base));
        let native_reader = next_history.get_native_reader(next_history.root()).await;
        self.native_store.set_reader(native_reader);
        Ok(Checkpoint {
            root: next_history.root(),
            log,
        })
    }

    async fn reset(&self, root: Blake2b256Hash) -> std::result::Result<(), String> {
        let started = crate::read_path_census::Clock::start();
        // **Clean channel locks** — the Scala's `RSpaceOps.reset` does this, and the port dropped it,
        // so the lock map kept an entry per channel the process ever touched (AUDIT C104).
        self.lock_f.clean_up();
        let next_history = {
            let history = self.current_history();
            history.reset(root).await?
        };
        *crate::lock::wlock(&self.history_repository) = next_history.clone();
        *crate::lock::wlock(&self.event_log) = Vec::new();
        *crate::lock::wlock(&self.produce_counter) = BTreeMap::new();
        let history_reader = next_history.get_history_reader(root).await;
        let base = history_reader.base();
        *crate::lock::wlock(&self.store) = Arc::new(InMemHotStore::new(base));
        self.native_store.revert(NativeStoreState::default());
        let native_reader = next_history.get_native_reader(root).await;
        self.native_store.set_reader(native_reader);
        crate::read_path_census::record_space_reset(started.elapsed());
        Ok(())
    }

    async fn get_data(
        &self,
        channel: &C,
    ) -> std::result::Result<Vec<Datum<A>>, crate::errors::RSpaceError> {
        self.current_store().get_data(channel).await
    }

    async fn get_waiting_continuations(
        &self,
        channels: &[C],
    ) -> std::result::Result<Vec<WaitingContinuation<P, K>>, crate::errors::RSpaceError> {
        self.current_store().get_continuations(channels).await
    }

    async fn get_joins(
        &self,
        channel: &C,
    ) -> std::result::Result<Vec<Vec<C>>, crate::errors::RSpaceError> {
        self.current_store().get_joins(channel).await
    }

    async fn clear(&self) -> std::result::Result<(), String> {
        let empty = crate::history::radix_tree::empty_root_hash();
        self.reset(empty).await
    }

    async fn to_map(&self) -> BTreeMap<Vec<C>, Row<P, A, K>> {
        self.current_store().to_map().await
    }

    async fn create_soft_checkpoint(&self) -> SoftCheckpoint<C, P, A, K> {
        let snapshot = self.current_store().snapshot().await;
        let log = std::mem::take(&mut *crate::lock::wlock(&self.event_log));
        let produce_counter = std::mem::take(&mut *crate::lock::wlock(&self.produce_counter));
        SoftCheckpoint {
            cache_snapshot: snapshot,
            log,
            produce_counter,
            native_snapshot: self.native_store.snapshot(),
        }
    }

    async fn revert_to_soft_checkpoint(&self, checkpoint: SoftCheckpoint<C, P, A, K>) {
        let history = self.current_history();
        let history_reader = history.get_history_reader(history.root()).await;
        let base = history_reader.base();
        *crate::lock::wlock(&self.store) =
            Arc::new(InMemHotStore::from_state(checkpoint.cache_snapshot, base));
        *crate::lock::wlock(&self.event_log) = checkpoint.log;
        *crate::lock::wlock(&self.produce_counter) = checkpoint.produce_counter;
        self.native_store.revert(checkpoint.native_snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::RSpaceError;
    use rchain_shared::store_manager::InMemoryStoreManager;

    struct StrMatch;
    impl Match<String, String> for StrMatch {
        fn get(&self, _p: &String, a: &String) -> std::result::Result<Option<String>, RSpaceError> {
            Ok(Some(a.clone()))
        }
    }

    async fn space() -> Arc<RSpace<String, String, String, String>> {
        let manager = InMemoryStoreManager::default();
        crate::factory::create_rspace::<String, String, String, String>(
            &manager,
            Arc::new(StrMatch),
        )
        .await
        .unwrap()
    }

    /// **`reset` releases the channel locks.** The Scala's `RSpaceOps.reset` calls `lockF.cleanUp`
    /// with the comment "Clean channel locks"; the port dropped the call, so the map kept one entry
    /// for every distinct channel the process had ever touched, for the life of the process (AUDIT
    /// C104). Without the call in `reset` this assertion is non-zero, which is the whole test.
    #[tokio::test]
    async fn reset_releases_the_channel_locks() {
        let s = space().await;
        // Both paths that acquire: a produce locks its channel, a consume locks its own set.
        s.produce("a".to_string(), "x".to_string(), false)
            .await
            .unwrap();
        s.consume(
            &["b".to_string()],
            &["p".to_string()],
            "k".to_string(),
            false,
            BTreeSet::new(),
        )
        .await
        .unwrap();
        assert!(
            s.lock_f.lock_map_len() > 0,
            "those operations are what populate the lock map"
        );

        let cp = s.create_checkpoint().await.unwrap();
        s.reset(cp.root).await.unwrap();

        assert_eq!(
            s.lock_f.lock_map_len(),
            0,
            "reset must clean the channel locks, as RSpaceOps.reset does in the Scala"
        );
    }

    #[tokio::test]
    async fn checkpoint_reset_returns_persisted_data() {
        let s = space().await;
        s.produce("c".to_string(), "data".to_string(), false)
            .await
            .unwrap();
        let cp = s.create_checkpoint().await.unwrap();
        s.reset(cp.root).await.unwrap();
        let data = s.get_data(&"c".to_string()).await.unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].a, "data");
    }

    #[tokio::test]
    async fn soft_checkpoint_revert_rolls_back_produces() {
        let s = space().await;
        let soft = s.create_soft_checkpoint().await;
        s.produce("c".to_string(), "extra".to_string(), false)
            .await
            .unwrap();
        s.revert_to_soft_checkpoint(soft).await;
        assert!(
            s.get_data(&"c".to_string()).await.unwrap().is_empty(),
            "reverted produce must be gone"
        );
    }

    #[tokio::test]
    async fn persistent_continuation_keeps_join_records_across_produces() {
        // Regression for #21/#22: a produce-side match used to remove the join records of a
        // persistent continuation, leaving the continuation installed but unreachable (the next
        // produce found no join and simply stored its datum).
        let s = space().await;
        s.consume(
            &["c".to_string()],
            &["p".to_string()],
            "k".to_string(),
            true,
            BTreeSet::new(),
        )
        .await
        .unwrap();

        let first = s
            .produce("c".to_string(), "d1".to_string(), false)
            .await
            .unwrap();
        assert!(
            first.is_some(),
            "persistent continuation must match the first produce"
        );

        let joins = s.get_joins(&"c".to_string()).await.unwrap();
        assert_eq!(
            joins,
            vec![vec!["c".to_string()]],
            "persistent continuation's join record must survive a produce-side COMM"
        );

        let second = s
            .produce("c".to_string(), "d2".to_string(), false)
            .await
            .unwrap();
        assert!(
            second.is_some(),
            "persistent continuation must match the second produce too"
        );
    }
}
