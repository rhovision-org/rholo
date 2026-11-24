//! **The divergence halt's acting rule** (F-U26-02/F-U26-03, issue #658).
//!
//! A spent restore budget is a divergence the node cannot repair without an anchor the network has
//! not agreed. The rule these tests pin: the durable record is written, the process registry is noted
//! from it, every proposal trigger refuses with `DivergenceHalted`, serving continues, and the only
//! exit is the operator's acknowledgement of the named block — which a fresh process must also obey,
//! because the record is seeded at boot.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

/// The proposer's own `BoxFuture` alias is private; this is the same expanded type.
type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::codecs::{
    BlockHashCodec, BlockMessageCodec, BlockMetadataCodec, SignedDeployDataCodec,
};
use rchain_block_storage::dag::dag_storage::{BlockDagStorage, DeployId, DivergenceHalt};
use rchain_casper::api::block_api::{ProposeHealth, SelfValidationRefusalSink};
use rchain_casper::block_metadata_store::BlockMetadataStore;
use rchain_casper::blocks::proposer::propose_result::{BlockCreatorResult, ProposeStatus};
use rchain_casper::blocks::proposer::proposer::{ProposeSource, Proposer};
use rchain_casper::dag::BlockDagKeyValueStorage;
use rchain_casper::divergence_halt;
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

/// The production codecs on an in-memory DAG. `with_records` attaches the durable-records store —
/// the `reset-marker` store the node uses for every control record — so the halt is written and read
/// exactly as a restarted process reads it.
async fn build_dag(with_records: bool) -> Arc<BlockDagKeyValueStorage> {
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
    if with_records {
        let records: Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>> = Arc::new(
            KeyValueTypedStoreCodec::new(fresh(), Arc::new(BytesCodec), Arc::new(BytesCodec)),
        );
        Arc::new(dag.with_reset_marker(records))
    } else {
        Arc::new(dag)
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

async fn status_of(proposer: &Proposer) -> ProposeStatus {
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let (result, block) = proposer
        .propose(ProposeSource::Automatic, tx)
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
    let surface = health
        .snapshot()
        .divergence_halt
        .expect("the surface names the halt");
    assert_eq!(surface.block, base16::encode(&[0xaa; 32]));
    assert_eq!(surface.seq_num, 9);
    assert!(surface.durable);

    let expected = base16::encode(&[0xaa; 32]);
    for source in [
        ProposeSource::Automatic,
        ProposeSource::Explicit { is_async: false },
    ] {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let (result, block) = proposer()
            .propose(source, tx)
            .await
            .expect("a halted propose is a status");
        assert!(block.is_none());
        assert_eq!(
            result.propose_status,
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
        status_of(&proposer()).await,
        ProposeStatus::DivergenceHalted { .. }
    ));

    // The acknowledgement clears the durable record and the registry; the next propose passes the
    // halt check and stops at the ordinary `NotBonded` gate.
    let live = dag
        .read_divergence_halt()
        .await
        .expect("a readable record")
        .expect("the halt");
    divergence_halt::acknowledgement(Some(&base16::encode(&live.block_hash)), &live)
        .expect("the correct acknowledgement");
    dag.clear_divergence_halt().await.expect("durable clear");
    health.clear_divergence_halt();
    assert!(!health.divergence_halted());
    assert_eq!(
        status_of(&proposer()).await,
        ProposeStatus::NotBonded,
        "after the acknowledgement the proposer reaches the next gate"
    );
    divergence_halt::seed(SHARD, None);
}

/// **A backend with nowhere to keep the record must not look like one that recorded it** — and the
/// halt still stands, with `durable: false` saying the next process will not know about it.
#[tokio::test]
async fn the_halt_record_is_durable_before_the_latch_is_observed() {
    let _guard = TEST_LOCK.lock().await;
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

    // The caller's policy: the refused write does not abort the halt — it makes the surface say the
    // halt is not durable. The registry is noted after the write returns.
    let mut halt = halt_for(0xbb);
    halt.durable = false;
    divergence_halt::note(SHARD, halt);
    let health = ProposeHealth::new().with_shard_id(SHARD);
    assert!(health.divergence_halted());
    let surface = health.snapshot().divergence_halt.expect("a standing halt");
    assert!(
        !surface.durable,
        "the surface must not claim the next process will remember it"
    );

    // A boot seed from the same storeless DAG leaves the registry alone and reports the error.
    let restarted = ProposeHealth::new().with_shard_id(SHARD);
    assert!(restarted
        .seed_divergence_halt_from_store(storeless.as_ref())
        .await
        .is_err());
    divergence_halt::seed(SHARD, None);
}

/// **The acknowledgement names the block.** A resume that does not match it is refused and the halt
/// stands; the matching one clears it.
#[tokio::test]
async fn an_acknowledgement_for_another_block_does_not_clear_the_halt() {
    let _guard = TEST_LOCK.lock().await;
    let halt = halt_for(0xcc);
    divergence_halt::note(SHARD, halt);
    assert!(divergence_halt::acknowledgement(None, &halt).is_err());
    assert!(divergence_halt::acknowledgement(Some("deadbeef"), &halt).is_err());
    assert!(
        divergence_halt::for_shard(SHARD).is_some(),
        "the halt stands"
    );

    let expected = base16::encode(&halt.block_hash);
    divergence_halt::acknowledgement(Some(&expected), &halt).expect("the matching ack");
    divergence_halt::clear(SHARD);
    assert!(divergence_halt::for_shard(SHARD).is_none());
}

/// **The acknowledged block clears the halt and resumes** — the same path the route takes, without a
/// transport.
#[tokio::test]
async fn the_acknowledged_block_clears_the_halt_and_resumes() {
    let _guard = TEST_LOCK.lock().await;
    let dag = build_dag(true).await;
    let halt = halt_for(0xdd);
    dag.write_divergence_halt(halt)
        .await
        .expect("durable write");
    let health = ProposeHealth::new().with_shard_id(SHARD);
    health
        .seed_divergence_halt_from_store(dag.as_ref())
        .await
        .expect("seeded");
    let live = dag
        .read_divergence_halt()
        .await
        .expect("readable")
        .expect("the halt");
    assert!(divergence_halt::acknowledgement(Some("00"), &live).is_err());
    assert!(health.divergence_halted(), "a wrong ack changes nothing");

    let expected = base16::encode(&live.block_hash);
    divergence_halt::acknowledgement(Some(&expected), &live).expect("the matching ack");
    dag.clear_divergence_halt()
        .await
        .expect("store-first clear");
    health.clear_divergence_halt();
    assert!(!health.divergence_halted());
    assert!(dag
        .read_divergence_halt()
        .await
        .expect("readable")
        .is_none());
    assert_eq!(status_of(&proposer()).await, ProposeStatus::NotBonded);
    divergence_halt::seed(SHARD, None);
}
