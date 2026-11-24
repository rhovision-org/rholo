//! **The divergence halt's acting rule** (F-U26-02/F-U26-03/F-U26-04/F-U26-05, issue #658).
//!
//! A spent restore budget is a divergence the node cannot repair without an anchor the network has
//! not agreed. The rule these tests pin: the durable record is written, the process registry is noted
//! from it, every proposal trigger refuses with `DivergenceHalted`, serving continues, and the only
//! exit is the operator's acknowledgement of the named block — which a fresh process must also obey,
//! because the record is seeded at boot.
//!
//! Two properties were added after the independent review of #667:
//!
//! - **an unreadable record is a halt, not an absence** — a fresh process whose boot read failed
//!   refuses every trigger until a read succeeds (F-U26-04);
//! - **an acknowledgement names the halt it read** — a halt recorded while an older acknowledgement
//!   is in flight is not deleted by it, and survives a restart (F-U26-05).
//!
//! The registry is process-global, so every test takes `TEST_LOCK`; the registry-mutating unit tests
//! live here rather than in `casper/src/divergence_halt.rs`, where they would race the proposer tests
//! in the same lib test binary.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// The proposer's own `BoxFuture` alias is private; this is the same expanded type.
type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::codecs::{
    BlockHashCodec, BlockMessageCodec, BlockMetadataCodec, SignedDeployDataCodec,
};
use rchain_block_storage::dag::dag_storage::{BlockDagStorage, DeployId, DivergenceHalt};
use rchain_casper::api::block_api::{
    DivergenceHaltObservation, ProposeHealth, SelfValidationRefusalSink,
};
use rchain_casper::block_metadata_store::BlockMetadataStore;
use rchain_casper::blocks::proposer::propose_result::{BlockCreatorResult, ProposeStatus};
use rchain_casper::blocks::proposer::proposer::{ProposeSource, Proposer};
use rchain_casper::dag::BlockDagKeyValueStorage;
use rchain_casper::divergence_halt::{self, AckOutcome, HaltCell};
use rchain_casper::multi_parent_casper::ValidateError;
use rchain_casper::validator_identity::ValidatorIdentity;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::SignedDeployData;
use rchain_models::validator::Validator;
use rchain_shared::base16;
use rchain_shared::log::NopLog;
use rchain_shared::store::InMemoryKeyValueStore;
use rchain_shared::typed_store::{
    BytesCodec, KeyValueTypedStore, KeyValueTypedStoreCodec, SharedStore,
};

/// The registry is process-global, so the tests in this file take turns.
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SHARD: &str = "test/divergence-halt";

fn halt_for(byte: u8) -> DivergenceHalt {
    DivergenceHalt {
        at_ms: 1_700_000_000_000,
        block_hash: [byte; 32],
        seq_num: 9,
        episodes: 1,
        durable: true,
    }
}

fn fresh() -> SharedStore {
    Arc::new(tokio::sync::Mutex::new(Box::new(
        InMemoryKeyValueStore::default(),
    )))
}

type Records = Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>>;

fn records() -> Records {
    Arc::new(KeyValueTypedStoreCodec::new(
        fresh(),
        Arc::new(BytesCodec),
        Arc::new(BytesCodec),
    ))
}

/// The production codecs on an in-memory DAG. `with_records` attaches the durable-records store —
/// the `reset-marker` store the node uses for every control record — so the halt is written and read
/// exactly as a restarted process reads it.
async fn build_dag(with_records: bool) -> Arc<BlockDagKeyValueStorage> {
    build_dag_over(if with_records { Some(records()) } else { None }).await
}

async fn build_dag_over(records: Option<Records>) -> Arc<BlockDagKeyValueStorage> {
    let metadata = Arc::new(
        BlockMetadataStore::create(Arc::new(KeyValueTypedStoreCodec::new(
            fresh(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMetadataCodec),
        )))
        .await
        .expect("metadata store"),
    );
    let deploy_index: Arc<dyn KeyValueTypedStore<DeployId, BlockHash>> = Arc::new(
        KeyValueTypedStoreCodec::new(fresh(), Arc::new(BytesCodec), Arc::new(BlockHashCodec)),
    );
    let deploy_store: Arc<dyn KeyValueTypedStore<DeployId, SignedDeployData>> =
        Arc::new(KeyValueTypedStoreCodec::new(
            fresh(),
            Arc::new(BytesCodec),
            Arc::new(SignedDeployDataCodec),
        ));
    let dag = BlockDagKeyValueStorage::create(metadata, deploy_index, deploy_store)
        .await
        .expect("dag storage");
    match records {
        Some(records) => Arc::new(dag.with_reset_marker(records)),
        None => Arc::new(dag),
    }
}

/// A records store whose first `get` returns the snapshot it read, then parks until the test
/// releases it. The interleaving test uses this to open the read→delete window deterministically.
struct GatedRecords {
    inner: Records,
    read_started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    gate_once: AtomicBool,
}

#[async_trait::async_trait]
impl KeyValueTypedStore<Vec<u8>, Vec<u8>> for GatedRecords {
    async fn get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>, String> {
        let out = self.inner.get(keys).await?;
        if self.gate_once.swap(false, Ordering::SeqCst) {
            self.read_started.notify_one();
            self.release.notified().await;
        }
        Ok(out)
    }

    async fn put(&self, pairs: &[(Vec<u8>, Vec<u8>)]) -> Result<(), String> {
        self.inner.put(pairs).await
    }

    async fn delete(&self, keys: &[Vec<u8>]) -> Result<usize, String> {
        self.inner.delete(keys).await
    }

    async fn contains(&self, keys: &[Vec<u8>]) -> Result<Vec<bool>, String> {
        self.inner.contains(keys).await
    }

    async fn to_map(&self) -> Result<std::collections::BTreeMap<Vec<u8>, Vec<u8>>, String> {
        self.inner.to_map().await
    }
}

fn block_store() -> BlockStore {
    Arc::new(KeyValueTypedStoreCodec::new(
        Arc::new(tokio::sync::Mutex::new(Box::new(
            InMemoryKeyValueStore::default(),
        ))),
        Arc::new(BlockHashCodec),
        Arc::new(BlockMessageCodec),
    ))
}

/// A proposer that never builds a block: with the halt standing, the check above `create_block`
/// refuses; once the halt is clear, `check_active = false` stops it at `NotBonded`, which is how the
/// test proves the halt check was passed without needing a creation fixture.
fn proposer() -> Proposer {
    let get_seq: Arc<dyn Fn(Validator) -> BoxFuture<i64> + Send + Sync> =
        Arc::new(|_v| Box::pin(async { 0i64 }));
    let check_active: Arc<
        dyn Fn(&ValidatorIdentity) -> BoxFuture<Result<bool, String>> + Send + Sync,
    > = Arc::new(|_v| Box::pin(async { Ok(false) }));
    let create_block: Arc<
        dyn Fn(
                &ValidatorIdentity,
                ProposeSource,
                rchain_casper::blocks::proposer::propose_result::ProposalMode,
                Option<usize>,
            ) -> BoxFuture<Result<BlockCreatorResult, String>>
            + Send
            + Sync,
    > = Arc::new(|_v, _source, _mode, _limit| {
        Box::pin(async { Err("a halted proposer must not reach create_block".to_string()) })
    });
    let validate: Arc<
        dyn Fn(
                &rchain_models::casper::protocol::casper_message::BlockMessage,
            ) -> BoxFuture<Result<(), ValidateError>>
            + Send
            + Sync,
    > = Arc::new(|_b| Box::pin(async { Ok(()) }));
    let effect: Arc<
        dyn Fn(&rchain_models::casper::protocol::casper_message::BlockMessage) -> BoxFuture<()>
            + Send
            + Sync,
    > = Arc::new(|_b| Box::pin(async {}));
    let validator = ValidatorIdentity::from_hex(
        "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530",
    )
    .expect("a valid validator key");
    Proposer::new(
        get_seq,
        check_active,
        create_block,
        validate,
        effect,
        validator,
        Arc::new(NopLog),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
        false,
        block_store(),
    )
    .with_self_validation_refusal(SelfValidationRefusalSink::default())
}

async fn status_of(source: ProposeSource) -> ProposeStatus {
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let (result, block) = proposer()
        .propose(source, tx)
        .await
        .expect("the halt check returns a status, not an error");
    assert!(block.is_none(), "a halted proposer broadcasts nothing");
    result.propose_status
}

/// **The acting rule, total across triggers and across a restart.** Every call reaches `do_propose`,
/// so the two sources here stand for the timer, the tap, the retry and the explicit route as well.
#[tokio::test]
async fn a_spent_restore_budget_halts_every_proposal_trigger_and_survives_a_restart() {
    let _guard = TEST_LOCK.lock().await;
    divergence_halt::seed(SHARD, None);
    let dag = build_dag(true).await;
    let halt = halt_for(0xaa);
    dag.write_divergence_halt(halt)
        .await
        .expect("durable write");

    let health = ProposeHealth::new().with_shard_id(SHARD);
    health
        .seed_divergence_halt_from_store(dag.as_ref())
        .await
        .expect("a readable record seeds");
    assert!(health.divergence_halted());
    let surface = match health.snapshot().divergence_halt {
        DivergenceHaltObservation::Halted(surface) => surface,
        other => panic!("the surface names the halt, got {other:?}"),
    };
    assert_eq!(surface.block, base16::encode(&[0xaa; 32]));
    assert_eq!(surface.seq_num, 9);
    assert!(surface.durable);

    let expected = base16::encode(&[0xaa; 32]);
    for source in [
        ProposeSource::Automatic,
        ProposeSource::Explicit { is_async: false },
    ] {
        assert_eq!(
            status_of(source).await,
            ProposeStatus::DivergenceHalted {
                block: expected.clone(),
                seq: 9
            },
            "every trigger must be refused while the halt stands"
        );
    }

    // A restart reads the same record and obeys it before its timer is spawned.
    let restarted = ProposeHealth::new().with_shard_id(SHARD);
    restarted
        .seed_divergence_halt_from_store(dag.as_ref())
        .await
        .expect("the record survives a restart");
    assert!(restarted.divergence_halted());
    assert!(matches!(
        status_of(ProposeSource::Automatic).await,
        ProposeStatus::DivergenceHalted { .. }
    ));

    // The acknowledgement clears the durable record and the registry; the next propose passes the
    // halt check and stops at the ordinary `NotBonded` gate.
    let outcome =
        divergence_halt::acknowledge_divergence_halt(dag.as_ref(), SHARD, Some(&expected))
            .await
            .expect("the matching acknowledgement");
    assert_eq!(outcome, AckOutcome::Cleared { block: expected });
    assert!(!health.divergence_halted());
    assert_eq!(
        status_of(ProposeSource::Automatic).await,
        ProposeStatus::NotBonded,
        "after the acknowledgement the proposer reaches the next gate"
    );
    divergence_halt::seed(SHARD, None);
}

/// **The durable record is written before the latch is observed** — and a backend with nowhere to
/// keep the record must not look like one that recorded it.
#[tokio::test]
async fn the_halt_record_is_durable_before_the_latch_is_observed() {
    let _guard = TEST_LOCK.lock().await;
    divergence_halt::seed(SHARD, None);
    let storeless = build_dag(false).await;
    assert!(
        storeless
            .write_divergence_halt(halt_for(0xbb))
            .await
            .is_err(),
        "a storeless DAG refuses the write"
    );
    assert!(
        storeless.read_divergence_halt().await.is_err(),
        "and refuses to claim there is no halt on record"
    );

    // The writer's one path does not abort the halt when the write is refused: it notes the halt
    // with `durable: false`, which is the surface saying the next process will not know about it.
    assert!(
        divergence_halt::record_halt(storeless.as_ref(), SHARD, halt_for(0xbb))
            .await
            .is_err()
    );
    let health = ProposeHealth::new();
    let surface = match health.divergence_halt_status() {
        DivergenceHaltObservation::Halted(surface) => surface,
        other => panic!("a standing halt, got {other:?}"),
    };
    assert!(
        !surface.durable,
        "the surface must not claim the next process will remember it"
    );

    // A boot seed from the same storeless DAG reports the error and does not weaken the live halt.
    let restarted = ProposeHealth::new().with_shard_id(SHARD);
    assert!(restarted
        .seed_divergence_halt_from_store(storeless.as_ref())
        .await
        .is_err());
    assert!(restarted.divergence_halted());
    divergence_halt::seed(SHARD, None);
}

/// **A failed boot read is a halt, not an empty registry.** The fresh-process property: before the
/// seed runs the shard is unknown; a failed seed leaves it unknown; every trigger refuses; the resume
/// is refused until a successful read resolves it.
#[tokio::test]
async fn a_failed_boot_read_stops_every_proposal_trigger_until_a_successful_read() {
    let _guard = TEST_LOCK.lock().await;
    divergence_halt::seed(SHARD, None);
    let storeless = build_dag(false).await;
    let health = ProposeHealth::new().with_shard_id(SHARD);

    // The construction alone is a stop: a seed that is dropped entirely cannot fail open.
    assert!(health.divergence_halted());
    assert!(matches!(
        health.snapshot().divergence_halt,
        DivergenceHaltObservation::Unreadable { .. }
    ));

    assert!(health
        .seed_divergence_halt_from_store(storeless.as_ref())
        .await
        .is_err());
    for source in [
        ProposeSource::Automatic,
        ProposeSource::Explicit { is_async: false },
    ] {
        assert_eq!(
            status_of(source).await,
            ProposeStatus::DivergenceHaltUnreadable {
                shard: SHARD.to_string()
            },
            "an unreadable record refuses every trigger"
        );
    }

    // The operator's resume is refused: it cannot acknowledge a record it cannot read.
    let refused =
        divergence_halt::acknowledge_divergence_halt(storeless.as_ref(), SHARD, None).await;
    assert!(refused.is_err());
    assert!(health.divergence_halted());

    // A successful read of an empty record retires the unknown state.
    let readable = build_dag(true).await;
    health
        .seed_divergence_halt_from_store(readable.as_ref())
        .await
        .expect("a readable empty record");
    assert!(matches!(
        health.snapshot().divergence_halt,
        DivergenceHaltObservation::Clear
    ));
    assert_eq!(
        status_of(ProposeSource::Automatic).await,
        ProposeStatus::NotBonded
    );
    divergence_halt::seed(SHARD, None);
}

/// **An acknowledgement of an older halt cannot clear a newer one.** The newer halt is durable, so a
/// restart still finds it.
#[tokio::test]
async fn an_acknowledgement_of_an_older_halt_does_not_clear_a_newer_halt() {
    let _guard = TEST_LOCK.lock().await;
    divergence_halt::seed(SHARD, None);
    let dag = build_dag(true).await;
    let older = halt_for(0x01);
    let newer = halt_for(0x02);
    dag.write_divergence_halt(older)
        .await
        .expect("the older halt");
    // The newer detection writes its record and notes it; the older acknowledgement is now stale.
    divergence_halt::record_halt(dag.as_ref(), SHARD, newer)
        .await
        .expect("the newer halt");

    let stale = base16::encode(&older.block_hash);
    assert!(
        divergence_halt::acknowledge_divergence_halt(dag.as_ref(), SHARD, Some(&stale))
            .await
            .is_err()
    );
    assert!(
        dag.read_divergence_halt()
            .await
            .expect("readable")
            .is_some(),
        "the newer record stands"
    );
    assert_eq!(
        divergence_halt::for_shard(SHARD)
            .expect("the newer halt")
            .block_hash,
        newer.block_hash
    );

    // A restart reads the newer record.
    divergence_halt::seed(SHARD, None);
    let restarted = ProposeHealth::new().with_shard_id(SHARD);
    restarted
        .seed_divergence_halt_from_store(dag.as_ref())
        .await
        .expect("the newer halt survives a restart");
    assert_eq!(
        match restarted.snapshot().divergence_halt {
            DivergenceHaltObservation::Halted(surface) => surface.block,
            other => panic!("a standing halt, got {other:?}"),
        },
        base16::encode(&newer.block_hash)
    );

    let matching = base16::encode(&newer.block_hash);
    divergence_halt::acknowledge_divergence_halt(dag.as_ref(), SHARD, Some(&matching))
        .await
        .expect("the matching acknowledgement");
    assert!(divergence_halt::for_shard(SHARD).is_none());
    assert!(dag
        .read_divergence_halt()
        .await
        .expect("readable")
        .is_none());
    divergence_halt::seed(SHARD, None);
}

/// **A halt recorded while an acknowledgement is reading survives.** The gated store opens the
/// read→delete window; with the per-shard transition lock the writer runs after the clear and its
/// halt stands, without it the clear deletes the newer halt.
#[tokio::test(flavor = "current_thread")]
async fn an_acknowledgement_cannot_delete_a_halt_recorded_while_it_reads() {
    let _guard = TEST_LOCK.lock().await;
    divergence_halt::seed(SHARD, None);
    let read_started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let records: Records = Arc::new(GatedRecords {
        inner: records(),
        read_started: read_started.clone(),
        release: release.clone(),
        gate_once: AtomicBool::new(true),
    });
    let dag = build_dag_over(Some(records)).await;
    let older = halt_for(0x0a);
    let newer = halt_for(0x0b);
    dag.write_divergence_halt(older)
        .await
        .expect("the older halt");
    let older_ack = base16::encode(&older.block_hash);

    let ack_task = {
        let dag = dag.clone();
        tokio::spawn(async move {
            divergence_halt::acknowledge_divergence_halt(dag.as_ref(), SHARD, Some(&older_ack))
                .await
        })
    };
    // The acknowledgement has read A and is parked inside the clear's read.
    read_started.notified().await;

    let writer = {
        let dag = dag.clone();
        tokio::spawn(async move { divergence_halt::record_halt(dag.as_ref(), SHARD, newer).await })
    };
    // Give the writer its chance to proceed while the read is parked. With the lock it is blocked;
    // without it, it writes B into the window the clear is about to delete.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    release.notify_one();

    ack_task.await.expect("ack task").expect("the older ack");
    writer.await.expect("writer task").expect("the newer halt");

    assert_eq!(
        dag.read_divergence_halt()
            .await
            .expect("readable")
            .expect("the newer halt stands")
            .block_hash,
        newer.block_hash
    );
    assert_eq!(
        divergence_halt::for_shard(SHARD)
            .expect("the newer latch")
            .block_hash,
        newer.block_hash
    );

    // And it survives a restart.
    divergence_halt::seed(SHARD, None);
    let restarted = ProposeHealth::new().with_shard_id(SHARD);
    restarted
        .seed_divergence_halt_from_store(dag.as_ref())
        .await
        .expect("seeded");
    assert!(matches!(
        restarted.snapshot().divergence_halt,
        DivergenceHaltObservation::Halted(ref surface) if surface.block == base16::encode(&newer.block_hash)
    ));
    divergence_halt::seed(SHARD, None);
}

/// **A non-durable halt still has an exit.** With the store empty, the registry's `durable: false`
/// halt requires its acknowledgement; the old code never cleared it, so its only exit was unreachable.
#[tokio::test]
async fn a_non_durable_registry_halt_is_acknowledged_and_cleared() {
    let _guard = TEST_LOCK.lock().await;
    divergence_halt::seed(SHARD, None);
    let dag = build_dag(true).await;
    let mut halt = halt_for(0x0c);
    halt.durable = false;
    divergence_halt::note(SHARD, halt);

    assert!(
        divergence_halt::acknowledge_divergence_halt(dag.as_ref(), SHARD, None)
            .await
            .is_err()
    );
    assert!(divergence_halt::for_shard(SHARD).is_some());

    let matching = base16::encode(&halt.block_hash);
    assert_eq!(
        divergence_halt::acknowledge_divergence_halt(dag.as_ref(), SHARD, Some(&matching))
            .await
            .expect("the matching acknowledgement"),
        AckOutcome::Cleared { block: matching }
    );
    assert!(divergence_halt::for_shard(SHARD).is_none());
    assert!(dag
        .read_divergence_halt()
        .await
        .expect("readable")
        .is_none());
}

/// **A successful read retires the unknown state**, even when the record names no halt.
#[tokio::test]
async fn a_readable_empty_record_resolves_an_unreadable_halt() {
    let _guard = TEST_LOCK.lock().await;
    divergence_halt::seed(SHARD, None);
    divergence_halt::seed_unreadable(SHARD, "the boot read failed");
    let health = ProposeHealth::new().with_shard_id(SHARD);
    assert!(health.divergence_halted());

    let dag = build_dag(true).await;
    assert_eq!(
        divergence_halt::acknowledge_divergence_halt(dag.as_ref(), SHARD, None)
            .await
            .expect("a successful read"),
        AckOutcome::NoHalt
    );
    assert!(!health.divergence_halted());
    assert!(matches!(
        health.snapshot().divergence_halt,
        DivergenceHaltObservation::Clear
    ));
    divergence_halt::seed(SHARD, None);
}

/// **The registry's three transitions**, including the boot seed's refusal to weaken a live halt.
#[tokio::test]
async fn an_unreadable_cell_counts_as_halted_and_a_boot_seed_never_weakens_a_halt() {
    let _guard = TEST_LOCK.lock().await;
    divergence_halt::seed(SHARD, None);
    divergence_halt::seed_unreadable(SHARD, "unreadable");
    assert!(divergence_halt::halted());
    assert!(divergence_halt::for_shard(SHARD).is_none());
    assert!(matches!(
        divergence_halt::state_for_shard(SHARD),
        Some(HaltCell::Unreadable { .. })
    ));

    divergence_halt::note(SHARD, halt_for(0x11));
    assert!(matches!(
        divergence_halt::state_for_shard(SHARD),
        Some(HaltCell::Halted(_))
    ));
    // A boot read that returns an older record must not replace the live halt.
    divergence_halt::seed_boot(SHARD, Some(halt_for(0x12)));
    assert_eq!(
        divergence_halt::for_shard(SHARD)
            .expect("the live halt")
            .block_hash,
        [0x11; 32]
    );

    // From an unreadable cell, the boot read resolves it.
    divergence_halt::seed(SHARD, None);
    divergence_halt::seed_unreadable(SHARD, "unreadable");
    divergence_halt::seed_boot(SHARD, Some(halt_for(0x13)));
    assert_eq!(
        divergence_halt::for_shard(SHARD)
            .expect("resolved")
            .block_hash,
        [0x13; 32]
    );
    divergence_halt::seed(SHARD, None);
}
