//! The replay tuple space (Law 11: replay determinism).
//!
//! Mirrors `rspace/src/main/scala/coop/rchain/rspace/ReplayRSpace.scala`. During replay a
//! consume/produce is re-executed against the recorded COMMs in `replayData`; the recomputed COMM
//! must be contained in the recorded trace, and `checkReplayData` fails if any recorded COMM was
//! left unused.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_shared::serialize::Serialize;

use crate::checkpoint::SoftCheckpoint;
use crate::concurrent::two_step_lock::TwoStepLock;
use crate::hashing::stable_hash_provider::hash_channel;
use crate::i_replay_space::IReplaySpace;
use crate::i_space::ISpace;
use crate::internal::{ConsumeCandidate, Datum, ProduceCandidate, Row, WaitingContinuation};
use crate::native_store::InMemNativeStore;
use crate::rspace::RSpace;
use crate::space_matcher::{extract_data_candidates, extract_first_match};
use crate::trace::event::{Comm, Consume, Event, Produce};
use crate::trace::Log;
use crate::tuple_space::{ContResult, Result, Tuplespace};
use crate::util::ReplayException;

/// The recorded replay trace: IO events keyed to the COMMs that reference them (port of
/// `ReplayData`).
#[derive(Clone, Debug, Default)]
pub struct ReplayData {
    by_consume: BTreeMap<Consume, Vec<Comm>>,
    by_produce: BTreeMap<Produce, Vec<Comm>>,
}

impl ReplayData {
    fn comms_for_consume(&self, consume: &Consume) -> Option<&Vec<Comm>> {
        self.by_consume.get(consume)
    }

    fn comms_for_produce(&self, produce: &Produce) -> Option<&Vec<Comm>> {
        self.by_produce.get(produce)
    }

    fn remove_consume_binding(&mut self, consume: &Consume, comm: &Comm) {
        let mut remove_key = false;
        if let Some(comms) = self.by_consume.get_mut(consume) {
            if let Some(pos) = comms.iter().position(|c| c == comm) {
                comms.remove(pos);
            }
            remove_key = comms.is_empty();
        }
        if remove_key {
            self.by_consume.remove(consume);
        }
    }

    fn remove_produce_binding(&mut self, produce: &Produce, comm: &Comm) {
        let mut remove_key = false;
        if let Some(comms) = self.by_produce.get_mut(produce) {
            if let Some(pos) = comms.iter().position(|c| c == comm) {
                comms.remove(pos);
            }
            remove_key = comms.is_empty();
        }
        if remove_key {
            self.by_produce.remove(produce);
        }
    }

    fn is_empty(&self) -> bool {
        self.by_consume.is_empty() && self.by_produce.is_empty()
    }

    fn len(&self) -> usize {
        self.by_consume.values().map(|v| v.len()).sum::<usize>()
            + self.by_produce.values().map(|v| v.len()).sum::<usize>()
    }
}

/// The replay space (port of `ReplayRSpace`).
pub struct ReplayRSpace<C, P, A, K> {
    space: Arc<RSpace<C, P, A, K>>,
    replay_data: RwLock<ReplayData>,
    lock_f: Arc<TwoStepLock<Blake2b256Hash>>,
}

impl<C, P, A, K> ReplayRSpace<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Serialize<P> + Send + Sync + 'static,
    A: Clone + Serialize<A> + Send + Sync + 'static,
    K: Clone + Serialize<K> + Send + Sync + 'static,
{
    pub fn new(space: Arc<RSpace<C, P, A, K>>) -> Self {
        ReplayRSpace {
            space,
            replay_data: RwLock::new(ReplayData::default()),
            lock_f: Arc::new(TwoStepLock::new()),
        }
    }

    /// The native system-contract store (shared with the wrapped play space).
    pub fn native_store(&self) -> Arc<InMemNativeStore> {
        self.space.native_store()
    }

    /// The keys **this** space's lock map is holding — test-only. The replay space acquires through a
    /// `lock_f` of its own, so the play space's count cannot stand in for it (AUDIT C104).
    #[cfg(test)]
    pub(crate) fn lock_map_len(&self) -> usize {
        self.lock_f.lock_map_len()
    }

    /// The native mutations folded into the most recent `create_checkpoint` on the wrapped space.
    pub fn last_native_changes(&self) -> Vec<crate::native_store::NativeStoreAction> {
        self.space.last_native_changes()
    }

    /// The same drain, grouped by the deploy that wrote each slot — what a block's sidecar is built
    /// from (#280).
    pub fn last_native_drain(&self) -> crate::native_store::NativeDrain {
        self.space.last_native_drain()
    }

    /// Build the replay data table from a log (port of `IReplaySpace.rig`). Only IO events that
    /// appear in the log get bound to their COMMs.
    fn build_replay_data(log: &Log) -> ReplayData {
        let mut produces: BTreeSet<Produce> = BTreeSet::new();
        let mut consumes: BTreeSet<Consume> = BTreeSet::new();
        for event in log {
            match event {
                Event::Produce(p) => {
                    produces.insert(p.clone());
                }
                Event::Consume(c) => {
                    consumes.insert(c.clone());
                }
                Event::Comm(_) => {}
            }
        }

        let mut data = ReplayData::default();
        // RSpace prepends events, so restore execution order before binding COMMs.
        // A later pending request may otherwise run before the peek repetition
        // its recorded COMM requires, suppressing work and changing replay cost.
        for event in log.iter().rev() {
            if let Event::Comm(comm) = event {
                if consumes.contains(&comm.consume) {
                    data.by_consume
                        .entry(comm.consume.clone())
                        .or_default()
                        .push(comm.clone());
                }
                for produce in &comm.produces {
                    if produces.contains(produce) {
                        data.by_produce
                            .entry(produce.clone())
                            .or_default()
                            .push(comm.clone());
                    }
                }
            }
        }
        data
    }

    /// Whether a datum participates in `comm` (port of `ReplayRSpace.matches`).
    fn matches(&self, comm: &Comm, datum: &Datum<A>) -> bool {
        let matched = if !comm.produces.contains(&datum.source) {
            false
        } else if datum.persist {
            true
        } else {
            let expected = comm.times_repeated.get(&datum.source).copied().unwrap_or(0);
            let actual = self.space.produce_counter_value(&datum.source);
            actual == expected
        };
        crate::replay_trace_census::note_match(matched);
        matched
    }

    async fn run_matcher_consume(
        &self,
        channels: &[C],
        patterns: &[P],
        comm: &Comm,
    ) -> std::result::Result<Option<Vec<ConsumeCandidate<C, A>>>, crate::errors::RSpaceError> {
        // **Why this does not sort, where play does** (AUDIT R37, and this is the written
        // disposition the row asked for).
        //
        // `RSpace::run_matcher_consume` reads a channel's data and then sorts it by `Datum::source`,
        // "so the sorted-first matching datum is chosen regardless of insertion order". This side
        // leaves store order alone, and it is correct because the two are not choosing among the same
        // set: `matches` above keeps only the data the *recorded* COMM names — `comm.produces` holds
        // the sources, and `times_repeated` the count — so what survives the filter is the recording's
        // choice, not the store's. Play has to sort because it picks among everything on the channel;
        // replay has already been told which datum it is.
        //
        // The order that remains cannot change the answer, and the reason is worth stating because it
        // is the whole argument: **two data with the same `source` are the same datum.**
        // `Produce::apply` (`trace/event.rs:24-36`) hashes `(channel, datum, persistent)`, so the
        // source is a content hash over everything that distinguishes one datum from another — if the
        // filter above admits more than one, they are the same bytes and swapping them is swapping
        // nothing. If that ever stops being true — if `matches` were widened to a predicate admitting
        // *distinct* content — this side would silently pick by store order while play picked by hash,
        // and a replay would diverge from the play it is replaying. That is the fragility R37 names,
        // and it is a property of the filter rather than of this loop.
        let store = self.space.current_store();
        let matcher = self.space.matcher();
        let mut channel_to_indexed_data: BTreeMap<C, Vec<(Datum<A>, i64)>> = BTreeMap::new();
        for c in channels {
            let data_list = store.get_data(c).await?;
            let mut indexed = Vec::new();
            for (i, d) in data_list.into_iter().enumerate() {
                if self.matches(comm, &d) {
                    indexed.push((d, i as i64));
                }
            }
            channel_to_indexed_data.insert(c.clone(), indexed);
        }
        let options = extract_data_candidates(
            &channels
                .iter()
                .cloned()
                .zip(patterns.iter().cloned())
                .collect::<Vec<_>>(),
            &channel_to_indexed_data,
            matcher.as_ref(),
        )?;
        Ok(options.into_iter().collect())
    }

    async fn get_comm_and_consume_candidates(
        &self,
        channels: &[C],
        patterns: &[P],
        comms: &[Comm],
    ) -> std::result::Result<Option<(Comm, Vec<ConsumeCandidate<C, A>>)>, crate::errors::RSpaceError>
    {
        for comm in comms {
            if let Some(candidates) = self.run_matcher_consume(channels, patterns, comm).await? {
                return Ok(Some((comm.clone(), candidates)));
            }
        }
        Ok(None)
    }

    async fn run_matcher_produce(
        &self,
        channel: &C,
        data: &A,
        persist: bool,
        comm: &Comm,
        produce_ref: &Produce,
        grouped_channels: &[Vec<C>],
    ) -> std::result::Result<Option<ProduceCandidate<C, P, A, K>>, crate::errors::RSpaceError> {
        let store = self.space.current_store();
        let matcher = self.space.matcher();
        for channels in grouped_channels {
            let conts = store.get_continuations(channels).await?;
            let match_candidates: Vec<(WaitingContinuation<P, K>, usize)> = conts
                .into_iter()
                .enumerate()
                .filter(|(_, wc)| comm.consume == wc.source)
                .map(|(i, wc)| (wc, i))
                .collect();

            let mut channel_to_indexed_data: BTreeMap<C, Vec<(Datum<A>, i64)>> = BTreeMap::new();
            for c in channels {
                let data_list = store.get_data(c).await?;
                let mut all: Vec<(Datum<A>, i64)> = Vec::new();
                if c == channel {
                    all.push((
                        Datum {
                            a: data.clone(),
                            persist,
                            source: produce_ref.clone(),
                        },
                        -1,
                    ));
                }
                all.extend(
                    data_list
                        .into_iter()
                        .enumerate()
                        .map(|(i, d)| (d, i as i64)),
                );
                let mut indexed = Vec::new();
                for (d, idx) in all {
                    if self.matches(comm, &d) {
                        indexed.push((d, idx));
                    }
                }
                channel_to_indexed_data.insert(c.clone(), indexed);
            }

            if let Some(pc) = extract_first_match(
                channels,
                &match_candidates,
                &channel_to_indexed_data,
                matcher.as_ref(),
            )? {
                return Ok(Some(pc));
            }
        }
        Ok(None)
    }

    async fn get_comm_or_produce_candidate(
        &self,
        channel: &C,
        data: &A,
        persist: bool,
        comms: &[Comm],
        produce_ref: &Produce,
        grouped_channels: &[Vec<C>],
    ) -> std::result::Result<Option<(Comm, ProduceCandidate<C, P, A, K>)>, crate::errors::RSpaceError>
    {
        for comm in comms {
            if let Some(pc) = self
                .run_matcher_produce(channel, data, persist, comm, produce_ref, grouped_channels)
                .await?
            {
                return Ok(Some((comm.clone(), pc)));
            }
        }
        Ok(None)
    }

    fn remove_bindings_for(&self, comm: &Comm) {
        let mut data = crate::lock::wlock(&self.replay_data);
        data.remove_consume_binding(&comm.consume, comm);
        for produce in &comm.produces {
            data.remove_produce_binding(produce, comm);
        }
    }

    async fn locked_consume(
        &self,
        channels: &[C],
        patterns: &[P],
        continuation: K,
        persist: bool,
        peeks: BTreeSet<usize>,
        consume_ref: Consume,
    ) -> std::result::Result<
        Option<(ContResult<C, P, K>, Vec<Result<C, A>>)>,
        crate::errors::RSpaceError,
    > {
        let wk = WaitingContinuation {
            patterns: patterns.to_vec(),
            continuation,
            persist,
            peeks: peeks.clone(),
            source: consume_ref.clone(),
        };
        let comms = crate::lock::rlock(&self.replay_data)
            .comms_for_consume(&consume_ref)
            .cloned();
        match comms {
            None => self.space.store_waiting_continuation(channels, wk).await,
            Some(comms) => match self
                .get_comm_and_consume_candidates(channels, patterns, &comms)
                .await?
            {
                None => self.space.store_waiting_continuation(channels, wk).await,
                Some((_comm, data_candidates)) => {
                    let comm_ref = Comm::apply(&data_candidates, consume_ref, peeks, |ps| {
                        self.space.produce_counters(ps)
                    });
                    if !comms.contains(&comm_ref) {
                        return Err(crate::errors::RSpaceError::ReplayCommNotInTrace);
                    }
                    self.space.store_persistent_data(&data_candidates).await?;
                    self.remove_bindings_for(&comm_ref);
                    self.space.wrap_result(channels, &wk, &data_candidates)
                }
            },
        }
    }

    async fn locked_produce(
        &self,
        channel: C,
        data: A,
        persist: bool,
        produce_ref: Produce,
    ) -> std::result::Result<
        Option<(ContResult<C, P, K>, Vec<Result<C, A>>)>,
        crate::errors::RSpaceError,
    > {
        let grouped_channels = self.space.current_store().get_joins(&channel).await?;
        if !persist {
            self.space.increment_produce_counter(&produce_ref);
        }
        let comms = crate::lock::rlock(&self.replay_data)
            .comms_for_produce(&produce_ref)
            .cloned();
        match comms {
            None => {
                self.space
                    .store_data(&channel, data, persist, produce_ref)
                    .await
            }
            Some(comms) => match self
                .get_comm_or_produce_candidate(
                    &channel,
                    &data,
                    persist,
                    &comms,
                    &produce_ref,
                    &grouped_channels,
                )
                .await?
            {
                None => {
                    self.space
                        .store_data(&channel, data, persist, produce_ref)
                        .await
                }
                Some((_comm, pc)) => self.handle_match(pc, &comms).await,
            },
        }
    }

    async fn handle_match(
        &self,
        pc: ProduceCandidate<C, P, A, K>,
        comms: &[Comm],
    ) -> std::result::Result<
        Option<(ContResult<C, P, K>, Vec<Result<C, A>>)>,
        crate::errors::RSpaceError,
    > {
        let ProduceCandidate {
            channels,
            continuation: wk,
            continuation_index,
            data_candidates,
        } = pc;
        let consume_ref = wk.source.clone();
        let comm_ref = Comm::apply(&data_candidates, consume_ref, wk.peeks.clone(), |ps| {
            self.space.produce_counters(ps)
        });
        if !comms.contains(&comm_ref) {
            return Err(crate::errors::RSpaceError::ReplayCommNotInTrace);
        }
        if !wk.persist {
            let store = self.space.current_store();
            store
                .remove_continuation(&channels, continuation_index)
                .await?;
            self.space
                .remove_matched_datum_and_join(&channels, &data_candidates)
                .await?;
        } else {
            // Keep the join records of a persistent continuation (it stays installed); only the
            // matched data is consumed — mirrors `RSpace::process_match_found` (issues #21/#22).
            self.space.store_persistent_data(&data_candidates).await?;
        }
        self.remove_bindings_for(&comm_ref);
        self.space.wrap_result(&channels, &wk, &data_candidates)
    }
}

#[async_trait]
impl<C, P, A, K> Tuplespace<C, P, A, K> for ReplayRSpace<C, P, A, K>
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
    ) -> std::result::Result<
        Option<(ContResult<C, P, K>, Vec<Result<C, A>>)>,
        crate::errors::RSpaceError,
    > {
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

    async fn produce(
        &self,
        channel: C,
        data: A,
        persist: bool,
    ) -> std::result::Result<
        Option<(ContResult<C, P, K>, Vec<Result<C, A>>)>,
        crate::errors::RSpaceError,
    > {
        let produce_ref = Produce::apply(&channel, &data, persist);
        let thunk = self.locked_produce(channel.clone(), data, persist, produce_ref);
        let phase_two = {
            let store = self.space.current_store();
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
        self.space.install(channels, patterns, continuation).await
    }
}

#[async_trait]
impl<C, P, A, K> ISpace<C, P, A, K> for ReplayRSpace<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Serialize<P> + Send + Sync + 'static,
    A: Clone + Serialize<A> + Send + Sync + 'static,
    K: Clone + Serialize<K> + Send + Sync + 'static,
{
    async fn create_checkpoint(
        &self,
    ) -> std::result::Result<crate::checkpoint::Checkpoint, String> {
        self.check_replay_data().await.map_err(|e| e.to_string())?;
        self.space.create_checkpoint().await
    }

    async fn reset(&self, root: Blake2b256Hash) -> std::result::Result<(), String> {
        // The replay space owns a lock map of its own, so the inner `reset` cleaning *its* map is not
        // enough — this is the second call the Scala makes through `TwoStepLock.cleanUp`'s composition
        // (AUDIT C104).
        self.lock_f.clean_up();
        self.space.reset(root).await
    }

    async fn get_data(
        &self,
        channel: &C,
    ) -> std::result::Result<Vec<Datum<A>>, crate::errors::RSpaceError> {
        self.space.get_data(channel).await
    }

    async fn get_waiting_continuations(
        &self,
        channels: &[C],
    ) -> std::result::Result<Vec<WaitingContinuation<P, K>>, crate::errors::RSpaceError> {
        self.space.get_waiting_continuations(channels).await
    }

    async fn get_joins(
        &self,
        channel: &C,
    ) -> std::result::Result<Vec<Vec<C>>, crate::errors::RSpaceError> {
        self.space.get_joins(channel).await
    }

    async fn clear(&self) -> std::result::Result<(), String> {
        self.space.clear().await
    }

    async fn to_map(&self) -> BTreeMap<Vec<C>, Row<P, A, K>> {
        self.space.to_map().await
    }

    async fn create_soft_checkpoint(&self) -> SoftCheckpoint<C, P, A, K> {
        self.space.create_soft_checkpoint().await
    }

    async fn revert_to_soft_checkpoint(
        &self,
        checkpoint: SoftCheckpoint<C, P, A, K>,
    ) -> std::result::Result<(), crate::errors::RSpaceError> {
        self.space.revert_to_soft_checkpoint(checkpoint).await
    }
}

#[async_trait]
impl<C, P, A, K> IReplaySpace<C, P, A, K> for ReplayRSpace<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Serialize<P> + Send + Sync + 'static,
    A: Clone + Serialize<A> + Send + Sync + 'static,
    K: Clone + Serialize<K> + Send + Sync + 'static,
{
    async fn rig(&self, log: Log) {
        let comms = log
            .iter()
            .filter(|event| matches!(event, Event::Comm(_)))
            .count();
        crate::replay_trace_census::note_rigged(comms);
        *crate::lock::wlock(&self.replay_data) = Self::build_replay_data(&log);
    }

    async fn rig_and_reset(
        &self,
        start_root: Blake2b256Hash,
        log: Log,
    ) -> std::result::Result<(), String> {
        self.rig(log).await;
        self.reset(start_root).await
    }

    async fn check_replay_data(&self) -> std::result::Result<(), ReplayException> {
        let data = crate::lock::rlock(&self.replay_data);
        if data.is_empty() {
            Ok(())
        } else {
            crate::replay_trace_census::note_leftover(data.len());
            Err(ReplayException(format!(
                "Unused COMM event: replayData multimap has {} elements left",
                data.len()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::RSpaceError;
    use std::sync::Arc;

    use rchain_shared::store_manager::InMemoryStoreManager;

    use crate::factory::create_history_repository;
    use crate::hot_store::InMemHotStore;
    use crate::i_replay_space::IReplaySpace;
    use crate::i_space::ISpace;
    use crate::match_::Match;
    use crate::tuple_space::Tuplespace;

    /// A trivial matcher: any pattern matches any datum, which is enough to produce events.
    struct StrMatch;
    impl Match<String, String> for StrMatch {
        fn get(&self, _p: &String, a: &String) -> std::result::Result<Option<String>, RSpaceError> {
            Ok(Some(a.clone()))
        }
    }

    async fn play_and_replay() -> (
        Arc<RSpace<String, String, String, String>>,
        ReplayRSpace<String, String, String, String>,
    ) {
        let manager = InMemoryStoreManager::default();
        let history = create_history_repository::<String, String, String, String>(
            &manager,
            "replay-internals-test",
        )
        .await
        .expect("history repository");
        let reader = history.get_history_reader(history.root()).await;
        let hot = Arc::new(InMemHotStore::new(reader.base()));
        RSpace::create_with_replay(history, hot, Arc::new(StrMatch))
    }

    /// **The replay space cleans its *own* lock map.** It acquires through a `lock_f` of its own, so
    /// the play space's `reset` cleaning *its* map is only half of what the Scala's
    /// `TwoStepLock.cleanUp` composes — this is the other half (AUDIT C104). Without the call in
    /// `ReplayRSpace::reset`, the count below stays non-zero after the reset.
    #[tokio::test]
    async fn reset_releases_the_replay_spaces_own_channel_locks() {
        let (play, replay) = play_and_replay().await;
        play.produce("c".to_string(), "data".to_string(), false)
            .await
            .expect("play produce");
        let recorded = play.create_soft_checkpoint().await.log;
        let root = play.create_checkpoint().await.expect("checkpoint").root;

        replay.rig_and_reset(root, recorded).await.expect("rig");
        replay
            .produce("c".to_string(), "data".to_string(), false)
            .await
            .expect("replay produce");
        assert!(
            replay.lock_map_len() > 0,
            "the replay space's own produce is what populates its map"
        );

        replay.reset(root).await.expect("reset");
        assert_eq!(
            replay.lock_map_len(),
            0,
            "reset must clean the replay space's own channel locks"
        );
    }

    /// The rig/check pair at its own level: a replay space rigged with the play space's recorded
    /// trace re-runs the same op and the check passes. `check_replay_data` is what `replay_compute_state`
    /// calls per deploy, so a break here surfaces as a replay failure on every block.
    #[tokio::test]
    async fn a_rigged_replay_matches_its_recorded_trace() {
        let (play, replay) = play_and_replay().await;

        play.produce("c".to_string(), "data".to_string(), false)
            .await
            .expect("play produce");
        let recorded = play.create_soft_checkpoint().await.log;
        let root = play.create_checkpoint().await.expect("checkpoint").root;

        replay.rig_and_reset(root, recorded).await.expect("rig");
        replay
            .produce("c".to_string(), "data".to_string(), false)
            .await
            .expect("replay produce");
        assert!(
            replay.check_replay_data().await.is_ok(),
            "a replay rigged with its own trace must check clean"
        );
    }

    /// **The negative half, in the direction the check actually works.** `check_replay_data`
    /// reports rigged **COMM** events that the replay never produced (`unused_comm_event`), not
    /// missing ones — so the failing shape is a rig that *expects* a match the replay does not make.
    ///
    /// This is worth being precise about: a test written the other way round (rig empty, replay
    /// produces something) passes, and would have given false confidence that the check catches
    /// divergence.
    #[tokio::test]
    async fn a_rig_whose_comm_never_happens_is_reported() {
        let (play, replay) = play_and_replay().await;

        // Record a *match*: a waiting consume, then a produce that joins with it. Together they are
        // one COMM event in the play space's trace.
        play.consume(
            &["c".to_string()],
            &["p".to_string()],
            "k".to_string(),
            false,
            BTreeSet::new(),
        )
        .await
        .expect("play consume");
        play.produce("c".to_string(), "data".to_string(), false)
            .await
            .expect("play produce");
        let recorded = play.create_soft_checkpoint().await.log;
        let root = play.create_checkpoint().await.expect("checkpoint").root;
        assert!(
            !recorded.is_empty(),
            "the recorded trace must contain the COMM for this test to mean anything"
        );

        // Rig the replay with that trace but replay only the *produce*: the produce finds no waiting
        // consume, so the COMM the rig expects never happens.
        replay.rig_and_reset(root, recorded).await.expect("rig");
        replay
            .produce("c".to_string(), "data".to_string(), false)
            .await
            .expect("replay produce");

        assert!(
            replay.check_replay_data().await.is_err(),
            "a rigged COMM that the replay never produced must be reported"
        );
    }
}
