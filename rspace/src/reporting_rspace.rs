//! The reporting replay space (port of `ReportingRspace.scala`).
//!
//! Mirrors `ReportingRspace`: a replay space that also accumulates a human-readable report of the
//! produce/consume/COMM events. The report is a `Seq[Seq[ReportingEvent]]` separated by soft
//! checkpoint (system deploy segments). The Scala overrides `logComm`/`logConsume`/`logProduce`
//! (hooks on `RSpaceOps`) to collect events; the Rust port records them directly in the
//! `Tuplespace::produce`/`consume` methods (the `record_*` helpers below), reconstructing the COMM
//! event from the produce/consume result.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_shared::serialize::Serialize;

use crate::checkpoint::SoftCheckpoint;
use crate::i_replay_space::IReplaySpace;
use crate::i_space::ISpace;
use crate::internal::{Datum, Row, WaitingContinuation};
use crate::native_store::InMemNativeStore;
use crate::replay_rspace::ReplayRSpace;
use crate::trace::Log;
use crate::tuple_space::{ContResult, Result, Tuplespace};
use crate::util::ReplayException;

/// A report entry (port of `ReportingRspace.ReportingEvent`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportingEvent<C, P, A, K> {
    Produce(ReportingProduce<C, A>),
    Consume(ReportingConsume<C, P, K>),
    Comm(ReportingComm<C, P, A, K>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportingProduce<C, A> {
    pub channel: C,
    pub data: A,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportingConsume<C, P, K> {
    pub channels: Vec<C>,
    pub patterns: Vec<P>,
    pub continuation: K,
    pub peeks: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportingComm<C, P, A, K> {
    pub consume: ReportingConsume<C, P, K>,
    pub produces: Vec<ReportingProduce<C, A>>,
}

/// The reporting replay space (port of `ReportingRspace`).
pub struct ReportingRspace<C, P, A, K> {
    replay: Arc<ReplayRSpace<C, P, A, K>>,
    report: RwLock<Vec<Vec<ReportingEvent<C, P, A, K>>>>,
    soft_report: RwLock<Vec<ReportingEvent<C, P, A, K>>>,
}

impl<C, P, A, K> ReportingRspace<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Serialize<P> + Send + Sync + 'static,
    A: Clone + Serialize<A> + Send + Sync + 'static,
    K: Clone + Serialize<K> + Send + Sync + 'static,
{
    pub fn new(replay: Arc<ReplayRSpace<C, P, A, K>>) -> Self {
        ReportingRspace {
            replay,
            report: RwLock::new(Vec::new()),
            soft_report: RwLock::new(Vec::new()),
        }
    }

    /// The native system-contract store (shared with the wrapped replay/play spaces).
    pub fn native_store(&self) -> Arc<InMemNativeStore> {
        self.replay.native_store()
    }

    pub fn record_produce(&self, channel: C, data: A) {
        crate::lock::wlock(&self.soft_report)
            .push(ReportingEvent::Produce(ReportingProduce { channel, data }));
    }

    pub fn record_consume(
        &self,
        channels: Vec<C>,
        patterns: Vec<P>,
        continuation: K,
        peeks: Vec<usize>,
    ) {
        crate::lock::wlock(&self.soft_report).push(ReportingEvent::Consume(ReportingConsume {
            channels,
            patterns,
            continuation,
            peeks,
        }));
    }

    pub fn record_comm(
        &self,
        consume: ReportingConsume<C, P, K>,
        produces: Vec<ReportingProduce<C, A>>,
    ) {
        crate::lock::wlock(&self.soft_report)
            .push(ReportingEvent::Comm(ReportingComm { consume, produces }));
    }

    /// Move the soft report into the report history (port of `collectReport`).
    pub fn collect_report(&self) {
        let mut soft = std::mem::take(&mut *crate::lock::wlock(&self.soft_report));
        if !soft.is_empty() {
            crate::lock::wlock(&self.report).push(std::mem::take(&mut soft));
        }
    }

    /// Drain and return the report (port of `getReport`).
    pub fn get_report(&self) -> Vec<Vec<ReportingEvent<C, P, A, K>>> {
        self.collect_report();
        std::mem::take(&mut *crate::lock::wlock(&self.report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::RSpaceError;
    use crate::factory::create_reporting_rspace;
    use crate::match_::Match;
    use rchain_shared::store_manager::InMemoryStoreManager;

    /// A matcher that matches everything — the same one `factory.rs`'s test uses, so a space built
    /// here is built exactly as the node builds it.
    struct StrMatch;
    impl Match<String, String> for StrMatch {
        fn get(&self, _p: &String, a: &String) -> std::result::Result<Option<String>, RSpaceError> {
            Ok(Some(a.clone()))
        }
    }

    async fn space() -> ReportingRspace<String, String, String, String> {
        create_reporting_rspace::<String, String, String, String>(
            &InMemoryStoreManager::default(),
            Arc::new(StrMatch),
        )
        .await
        .expect("reporting space")
    }

    fn produce_event(channel: &str, data: &str) -> ReportingEvent<String, String, String, String> {
        ReportingEvent::Produce(ReportingProduce {
            channel: channel.to_string(),
            data: data.to_string(),
        })
    }

    /// Recording appends to the **soft** report, which `collect_report` moves into a batch — and a
    /// collect with nothing recorded adds **no** empty batch, so a report never carries a
    /// checkpoint that reported nothing.
    #[tokio::test]
    async fn recording_is_soft_until_collected_and_an_empty_collect_adds_no_batch() {
        let space = space().await;
        assert!(
            space.get_report().is_empty(),
            "a fresh space reports nothing"
        );

        space.record_produce("chan".to_string(), "datum".to_string());
        // The soft report is private; `collect_report` is the only way to see it, and collecting
        // twice must not duplicate the batch.
        space.collect_report();
        space.collect_report();
        let report = space.get_report();
        assert_eq!(report.len(), 1, "one batch, not two and not zero");
        assert_eq!(report[0], vec![produce_event("chan", "datum")]);

        // A collect with an empty soft report does not add a batch…
        space.collect_report();
        assert_eq!(
            space.get_report().len(),
            0,
            "…and `get_report` drained the first"
        );

        // …but a *recorded* event after a drain starts a new batch.
        space.record_produce("chan".to_string(), "second".to_string());
        let report = space.get_report();
        assert_eq!(report.len(), 1);
        assert_eq!(report[0], vec![produce_event("chan", "second")]);
    }

    /// `get_report` **drains**: the node calls it once per block and would otherwise re-report every
    /// event it has ever seen.
    #[tokio::test]
    async fn get_report_drains_the_history() {
        let space = space().await;
        space.record_produce("chan".to_string(), "one".to_string());
        assert_eq!(space.get_report().len(), 1);
        assert_eq!(
            space.get_report(),
            Vec::<Vec<ReportingEvent<String, String, String, String>>>::new(),
            "the second call sees nothing"
        );
    }

    /// The three recorders keep their own fields, in the order they were recorded: a consume and a
    /// COMM are not two spellings of a produce.
    #[tokio::test]
    async fn the_recorders_keep_the_event_kinds_apart_and_in_order() {
        let space = space().await;
        let consume = ReportingConsume {
            channels: vec!["chan".to_string()],
            patterns: vec!["pat".to_string()],
            continuation: "body".to_string(),
            peeks: vec![0],
        };

        space.record_produce("chan".to_string(), "datum".to_string());
        space.record_consume(
            consume.channels.clone(),
            consume.patterns.clone(),
            consume.continuation.clone(),
            consume.peeks.clone(),
        );
        space.record_comm(
            consume.clone(),
            vec![ReportingProduce {
                channel: "chan".to_string(),
                data: "datum".to_string(),
            }],
        );

        let report = space.get_report();
        assert_eq!(report.len(), 1);
        assert_eq!(
            report[0],
            vec![
                produce_event("chan", "datum"),
                ReportingEvent::Consume(consume.clone()),
                ReportingEvent::Comm(ReportingComm {
                    consume,
                    produces: vec![ReportingProduce {
                        channel: "chan".to_string(),
                        data: "datum".to_string(),
                    }],
                }),
            ]
        );
    }

    /// The wrapper's `ISpace` reads are the wrapped space's reads, not views of the report: an empty
    /// space has no data, no continuations, no joins and an empty map, with or without a report.
    #[tokio::test]
    async fn the_space_reads_are_the_wrapped_space_reads() {
        let space = space().await;
        space.record_produce("chan".to_string(), "datum".to_string());

        assert!(space
            .get_data(&"chan".to_string())
            .await
            .expect("data")
            .is_empty());
        assert!(space
            .get_waiting_continuations(&["chan".to_string()])
            .await
            .expect("continuations")
            .is_empty());
        assert!(space
            .get_joins(&"chan".to_string())
            .await
            .expect("joins")
            .is_empty());
        assert!(space.to_map().await.is_empty());

        // …and the report is still there afterwards: reading the space does not collect.
        assert_eq!(space.get_report().len(), 1);
    }

    /// The native store is the **same** store the wrapped spaces use — the registry/PoS/vault state
    /// a system contract writes must be visible to the reporting layer, not a second copy.
    #[tokio::test]
    async fn the_native_store_is_shared_with_the_wrapped_space() {
        let space = space().await;
        assert!(Arc::ptr_eq(&space.native_store(), &space.native_store()));
    }
}

#[async_trait]
impl<C, P, A, K> Tuplespace<C, P, A, K> for ReportingRspace<C, P, A, K>
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
        // Record the consume event (port of `logConsume`).
        let peeks_vec: Vec<usize> = peeks.iter().copied().collect();
        self.record_consume(
            channels.to_vec(),
            patterns.to_vec(),
            continuation.clone(),
            peeks_vec.clone(),
        );

        let result = self
            .replay
            .consume(channels, patterns, continuation, persist, peeks)
            .await?;

        // Record the COMM event if the consume matched produces (port of `logComm`).
        if let Some((cont, results)) = &result {
            let consume = ReportingConsume {
                channels: channels.to_vec(),
                patterns: patterns.to_vec(),
                continuation: cont.continuation.clone(),
                peeks: peeks_vec,
            };
            let produces = results
                .iter()
                .map(|r| ReportingProduce {
                    channel: r.channel.clone(),
                    data: r.matched_datum.clone(),
                })
                .collect();
            self.record_comm(consume, produces);
        }
        Ok(result)
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
        // Record the produce event (port of `logProduce`).
        self.record_produce(channel.clone(), data.clone());

        let result = self
            .replay
            .produce(channel.clone(), data.clone(), persist)
            .await?;

        // Record the COMM event if the produce matched a consume (port of `logComm`).
        if let Some((cont, results)) = &result {
            let consume = ReportingConsume {
                channels: cont.channels.clone(),
                patterns: cont.patterns.clone(),
                continuation: cont.continuation.clone(),
                peeks: if cont.peek { vec![0] } else { vec![] },
            };
            let produces = results
                .iter()
                .map(|r| ReportingProduce {
                    channel: r.channel.clone(),
                    data: r.matched_datum.clone(),
                })
                .collect();
            self.record_comm(consume, produces);
        }
        Ok(result)
    }

    async fn install(
        &self,
        channels: &[C],
        patterns: &[P],
        continuation: K,
    ) -> std::result::Result<Option<(K, Vec<A>)>, crate::errors::RSpaceError> {
        self.replay.install(channels, patterns, continuation).await
    }
}

#[async_trait]
impl<C, P, A, K> ISpace<C, P, A, K> for ReportingRspace<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Serialize<P> + Send + Sync + 'static,
    A: Clone + Serialize<A> + Send + Sync + 'static,
    K: Clone + Serialize<K> + Send + Sync + 'static,
{
    async fn create_checkpoint(
        &self,
    ) -> std::result::Result<crate::checkpoint::Checkpoint, String> {
        let checkpoint = self.replay.create_checkpoint().await?;
        crate::lock::wlock(&self.soft_report).clear();
        crate::lock::wlock(&self.report).clear();
        Ok(checkpoint)
    }

    async fn reset(&self, root: Blake2b256Hash) -> std::result::Result<(), String> {
        self.replay.reset(root).await
    }

    async fn get_data(
        &self,
        channel: &C,
    ) -> std::result::Result<Vec<Datum<A>>, crate::errors::RSpaceError> {
        self.replay.get_data(channel).await
    }

    async fn get_waiting_continuations(
        &self,
        channels: &[C],
    ) -> std::result::Result<Vec<WaitingContinuation<P, K>>, crate::errors::RSpaceError> {
        self.replay.get_waiting_continuations(channels).await
    }

    async fn get_joins(
        &self,
        channel: &C,
    ) -> std::result::Result<Vec<Vec<C>>, crate::errors::RSpaceError> {
        self.replay.get_joins(channel).await
    }

    async fn clear(&self) -> std::result::Result<(), String> {
        self.replay.clear().await
    }

    async fn to_map(&self) -> std::collections::BTreeMap<Vec<C>, Row<P, A, K>> {
        self.replay.to_map().await
    }

    async fn create_soft_checkpoint(&self) -> SoftCheckpoint<C, P, A, K> {
        self.collect_report();
        self.replay.create_soft_checkpoint().await
    }

    async fn revert_to_soft_checkpoint(
        &self,
        checkpoint: SoftCheckpoint<C, P, A, K>,
    ) -> std::result::Result<(), crate::errors::RSpaceError> {
        self.replay.revert_to_soft_checkpoint(checkpoint).await
    }
}

#[async_trait]
impl<C, P, A, K> IReplaySpace<C, P, A, K> for ReportingRspace<C, P, A, K>
where
    C: Ord + Clone + Serialize<C> + Send + Sync + 'static,
    P: Clone + Serialize<P> + Send + Sync + 'static,
    A: Clone + Serialize<A> + Send + Sync + 'static,
    K: Clone + Serialize<K> + Send + Sync + 'static,
{
    async fn rig(&self, log: Log) {
        self.replay.rig(log).await;
    }

    async fn rig_and_reset(
        &self,
        start_root: Blake2b256Hash,
        log: Log,
    ) -> std::result::Result<(), String> {
        self.replay.rig_and_reset(start_root, log).await
    }

    async fn check_replay_data(&self) -> std::result::Result<(), ReplayException> {
        self.replay.check_replay_data().await
    }
}
