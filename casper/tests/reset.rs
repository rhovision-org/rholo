//! The reset's acting half, driven where it is decided (#320, F-U5-01's acting half, register C274).
//!
//! **What this is for.** The engine half was pinned by reading while the rest of the unit was tested:
//! the loop's third exit (`node_launch::apply`'s `select!`) has no harness — only `node_runtime` calls
//! `apply`, and building the fixture it would need (a transport, an importer, an exporter, a
//! `ShardSpec`, a `NodeRunning`) is a project rather than a test. So the arm's *body* was extracted into
//! `engine::reset::act_on_reset_request`, which takes the four things the decision actually reads — the
//! switch, the two stores and the runtime — and this drives *that*.
//!
//! What it therefore pins: the request is consumed exactly once, the switch gates the act, the act
//! rewinds the DAG and purges the bodies it dropped, and the anchor survives both. What it deliberately
//! does not pin: the five lines of `select!` around the call, which are read rather than run — said out
//! loud here rather than left as an implication that the loop is covered.
//!
//! **The anchor is real.** The fixture uses the runtime manager's own root as the anchor block's
//! post-state, so the runtime reset is the ordinary one (`RhoRuntime::reset` on a root the history
//! knows) and not a fabricated hash — which is what makes the whole act run rather than stopping at the
//! first step.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::codecs::{
    BlockHashCodec, BlockMetadataCodec, SignedDeployDataCodec,
};
use rchain_block_storage::dag::dag_storage::{BlockDagStorage, DeployId, ResetMarker, ResetPhase};
use rchain_casper::block_metadata_store::BlockMetadataStore;
use rchain_casper::dag::BlockDagKeyValueStorage;
use rchain_casper::engine::reset::{act_on_reset_request, resume_after_reset};
use rchain_casper::interpreter_util::{
    reset_request, take_reset_request, ResetReason, ResetRequest,
};
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::block_metadata::{BlockMetadata, SlashSeverity};
use rchain_models::casper::protocol::casper_message::{
    BlockMessage, DeployData, ProcessedDeploy, RholangState, SignedDeployData,
};
use rchain_models::validator::Validator;
use rchain_shared::lmdb::LmdbStoreManager;
use rchain_shared::log::{LogSource, NopLog};
use rchain_shared::refined::BlockHeight;
use rchain_shared::store::InMemoryKeyValueStore;
use rchain_shared::store_manager::database;
use rchain_shared::typed_store::{
    BytesCodec, KeyValueTypedStore, KeyValueTypedStoreCodec, SharedStore,
};

/// Every cell call in this file names one lane (§107's D4): the reset cells are process-wide and
/// keyed by shard, and these tests drive a single shard.
const LANE: &str = "test-shard";

/// A directory owned by one disk-backed fixture. It is created exclusively before the guard owns
/// it, and all LMDB handles are dropped before normal cleanup.
struct ResetDiskDir(std::path::PathBuf);

impl ResetDiskDir {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rholo-n564-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).expect("an exclusive reset fixture directory");
        Self(path)
    }
}

impl Drop for ResetDiskDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Open the production codecs and DAG constructor on an actual LMDB environment. Named databases
/// are stable across close/reopen. The evaluation runtime is still the ordinary test fixture;
/// this helper does not model a running network, a process crash or power-loss durability.
async fn open_reset_disk_stores(
    path: &std::path::Path,
) -> (LmdbStoreManager, Arc<BlockDagKeyValueStorage>, BlockStore) {
    let manager = LmdbStoreManager::new(path, 64 * 1024 * 1024).expect("LMDB environment");
    let metadata = Arc::new(
        BlockMetadataStore::create(Arc::new(
            database(
                &manager,
                "block-metadata",
                Arc::new(BlockHashCodec),
                Arc::new(BlockMetadataCodec),
            )
            .await
            .unwrap(),
        ))
        .await
        .unwrap(),
    );
    let deploy_index = Arc::new(
        database(
            &manager,
            "deploy-index",
            Arc::new(BytesCodec),
            Arc::new(BlockHashCodec),
        )
        .await
        .unwrap(),
    );
    let deploy_store = Arc::new(
        database(
            &manager,
            "deploy-pool",
            Arc::new(BytesCodec),
            Arc::new(SignedDeployDataCodec),
        )
        .await
        .unwrap(),
    );
    let reset_marker = Arc::new(
        database(
            &manager,
            "reset-records",
            Arc::new(BytesCodec),
            Arc::new(BytesCodec),
        )
        .await
        .unwrap(),
    );
    let dag = Arc::new(
        BlockDagKeyValueStorage::create(metadata, deploy_index, deploy_store)
            .await
            .unwrap()
            .with_reset_marker(reset_marker),
    );
    let blocks = rchain_block_storage::block_store::create(&manager)
        .await
        .unwrap();
    (manager, dag, blocks)
}

fn hash(byte: u8) -> BlockHash {
    let mut bytes = [0u8; 32];
    bytes[0] = byte;
    BlockHash::new(bytes)
}

/// A block at `hash` whose post-state is `post_state_hash` — so a fixture can make the anchor carry
/// the runtime's *own* root, which is what lets the reset's last step run instead of failing at it.
fn block(hash: BlockHash, post_state_hash: StateHash) -> BlockMessage {
    BlockMessage {
        version: 1,
        shard_id: "/root".to_string(),
        block_hash: hash,
        block_number: BlockHeight::zero(),
        sender: Validator::new([0u8; 65]),
        seq_num: 0.try_into().unwrap(),
        pre_state_hash: StateHash::new([0u8; 32]),
        post_state_hash,
        justifications: vec![],
        bonds: BTreeMap::new(),
        rejected_deploys: BTreeSet::new(),
        rejected_blocks: BTreeSet::new(),
        rejected_senders: BTreeSet::new(),
        state: RholangState::default(),
        sig_algorithm: "secp256k1".to_string(),
        sig: vec![],
        timestamp: 0,
    }
}

fn meta(hash: BlockHash, parents: &[BlockHash], block_num: i64, seq: i64) -> BlockMetadata {
    BlockMetadata {
        block_hash: hash,
        block_num: BlockHeight::try_from(block_num).unwrap(),
        sender: Validator::new([0u8; 65]),
        seq_num: seq.try_into().unwrap(),
        justifications: parents.iter().copied().collect(),
        bonds_map: BTreeMap::new(),
        validated: true,
        validation_failed: false,
        slashable: false,
        failure_cause: None,
        slash_severity: SlashSeverity::Unspecified,
        restore_attempts: 0,
        fringe: BTreeSet::new(),
        fringe_state_hash: StateHash::new([0u8; 32]),
        pre_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
        post_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
        sig: Vec::new(),
        sig_algorithm: String::new(),
        member_of_fringe: None,
    }
}

/// An empty DAG over in-memory stores. `node_running.rs`'s and `dag.rs`'s test modules each build the
/// same storage for themselves (the helper is `#[cfg(test)]` and so unreachable across modules); this is
/// the third copy, and the reason the duplication is the recorded practice is in `build_dag`'s own note
/// there: widening it into a production constructor would put a test seam in the engine's API.
async fn build_dag() -> Arc<BlockDagKeyValueStorage> {
    let fresh = || -> SharedStore {
        Arc::new(tokio::sync::Mutex::new(Box::new(
            InMemoryKeyValueStore::default(),
        )))
    };
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
    // **The reset marker is part of the fixture** (F-U5-01, C274). A reset refuses to run without
    // somewhere to record itself — the whole point of the record is that a crash mid-reset is
    // repairable — so a test that drives one attaches the store, exactly as the node does.
    let reset_marker: Arc<dyn KeyValueTypedStore<Vec<u8>, Vec<u8>>> = Arc::new(
        KeyValueTypedStoreCodec::new(fresh(), Arc::new(BytesCodec), Arc::new(BytesCodec)),
    );
    Arc::new(
        BlockDagKeyValueStorage::create(metadata, deploy_index, deploy_store)
            .await
            .expect("dag storage")
            .with_reset_marker(reset_marker),
    )
}

async fn build_block_store() -> BlockStore {
    let manager = rchain_shared::store_manager::InMemoryStoreManager::default();
    rchain_block_storage::block_store::create(&manager)
        .await
        .expect("block store")
}

/// The same DAG storage **without** the durable-records store attached — the backend that has nowhere
/// to keep a halt record. It is the fixture for the third state: a read that cannot be answered must
/// read as "unknown", not as "never halted".
async fn build_dag_without_marker_store() -> Arc<BlockDagKeyValueStorage> {
    let fresh = || -> SharedStore {
        Arc::new(tokio::sync::Mutex::new(Box::new(
            InMemoryKeyValueStore::default(),
        )))
    };
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
    Arc::new(
        BlockDagKeyValueStorage::create(metadata, deploy_index, deploy_store)
            .await
            .expect("dag storage"),
    )
}

async fn has(store: &BlockStore, h: BlockHash) -> bool {
    store
        .contains(&[h])
        .await
        .expect("the store answers")
        .first()
        .copied()
        .unwrap_or(false)
}

/// **The acting half, end to end at the level a test can reach.** One test rather than three, because
/// the reset request is a process-wide cell: the first request wins, so a second test's request would
/// be refused while this one's was pending — the same reason the signal's own contract test is one.
#[tokio::test]
async fn a_reset_request_rewinds_the_stores_when_the_switch_is_on_and_does_nothing_when_it_is_off()
{
    // Whatever a previous run left in the cell: this test owns it from here.
    let _ = take_reset_request(LANE);

    let dag = build_dag().await;
    let block_store = build_block_store().await;
    let manager = Arc::new(common::build_runtime_manager().await);
    // **A root the runtime knows**, so the reset's last step is the ordinary one.
    let known_root = manager.get_history_repo().root();
    let root_state = StateHash::new(known_root.to_byte_array());

    // A chain of four: heights 0..3, the tip above the anchor we will name.
    let mut parents: Vec<BlockHash> = Vec::new();
    for n in 0..4i64 {
        let h = hash(n as u8);
        // The block at height 1 carries the runtime's root; the rest are ordinary.
        let post = if n == 1 {
            root_state
        } else {
            StateHash::new([0u8; 32])
        };
        dag.insert(meta(h, &parents, n, n), block(h, post))
            .await
            .expect("the chain inserts");
        block_store
            .put(&[(h, block(h, post))])
            .await
            .expect("the body is stored");
        parents = vec![h];
    }

    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);

    let anchor = hash(1);
    let anchor_height = BlockHeight::try_from(1).unwrap();
    let refused = hash(3);
    let request = ResetRequest {
        anchor,
        anchor_height,
        blocks_above: 2,
        refused,
        reason: ResetReason::DivergenceBudget,
    };

    // --- the switch off --------------------------------------------------------------------------
    // First half: a node whose operator has not opted in must *not* touch its stores, and must consume
    // the request rather than leaving it pending for ever.
    assert!(
        rchain_casper::interpreter_util::request_reset(LANE.to_string(), request.clone()),
        "the request is raised"
    );
    let off = act_on_reset_request(
        LANE,
        false,
        false,
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &block_store,
        &manager,
        &log,
        LogSource::new("test"),
    )
    .await
    .expect("a gated reset is not an error");
    assert!(off.is_none(), "the switch is off, so nothing acted");
    assert_eq!(
        reset_request(LANE),
        None,
        "and the request was consumed, not left pending"
    );
    assert!(
        has(&block_store, refused).await,
        "the tip's body is still there: nothing was rewound"
    );

    // --- the switch on ---------------------------------------------------------------------------
    assert!(
        rchain_casper::interpreter_util::request_reset(LANE.to_string(), request.clone()),
        "a second request is accepted once the first is consumed"
    );
    let acted = act_on_reset_request(
        LANE,
        true,
        false,
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &block_store,
        &manager,
        &log,
        LogSource::new("test"),
    )
    .await
    .expect("the reset runs over healthy stores and a known root");
    assert_eq!(
        acted.map(|r| r.anchor),
        Some(anchor),
        "the caller is told which anchor to catch up from"
    );

    let repr = dag.get_representation().await;
    assert!(!repr.contains(&refused), "the DAG is at the anchor");
    assert!(
        repr.contains(&anchor),
        "and the anchor survives — it is the state replayed from"
    );
    drop(repr);
    assert!(
        !has(&block_store, refused).await,
        "the dropped body is gone, so it can be neither served nor re-injected"
    );
    assert!(
        has(&block_store, anchor).await,
        "the anchor's own body stays — the reset keeps what it rewinds to"
    );
    assert_eq!(
        reset_request(LANE),
        None,
        "taking the request is not undone by acting on it"
    );

    // --- a failure that destroyed nothing leaves the request pending ---------------------------------
    // **Law 73**: the pre-flight read runs before the take, so a reset whose anchor this node cannot
    // read destroys nothing and leaves the request actionable. Under the old order the take ran first
    // and spent it on a failure that had touched no store. A fresh DAG and block store make the anchor
    // genuinely absent rather than already rewound to.
    let bare_dag = build_dag().await;
    let bare_store = build_block_store().await;
    let pending = ResetRequest {
        anchor: hash(1),
        anchor_height: BlockHeight::try_from(1).unwrap(),
        blocks_above: 0,
        refused: hash(2),
        reason: ResetReason::DivergenceBudget,
    };
    assert!(
        rchain_casper::interpreter_util::request_reset(LANE.to_string(), pending.clone()),
        "a request is raised again"
    );
    let unacted = act_on_reset_request(
        LANE,
        true,
        false,
        &(bare_dag.clone() as Arc<dyn BlockDagStorage>),
        &bare_store,
        &manager,
        &log,
        LogSource::new("test"),
    )
    .await
    .expect("a pre-flight refusal is not an error");
    assert!(
        unacted.is_none(),
        "nothing acted — the anchor could not be read"
    );
    assert_eq!(
        reset_request(LANE),
        Some(pending.clone()),
        "and the request is still pending, not spent on a failure that destroyed nothing"
    );
    assert_eq!(
        take_reset_request(LANE),
        Some(pending),
        "and it is still takeable — the detector's next scan can act on it"
    );
}

/// A missing body above the anchor is an incomplete deploy inventory, not an empty one (#564,
/// C366). The body-store enumeration used to skip this block altogether and commit the rewind.
/// Drive the real reset entry point: it must refuse before the marker or any destructive effect.
#[tokio::test]
async fn a_self_replay_reset_refuses_a_dag_block_whose_body_is_missing() {
    const SHARD: &str = "n564-reset-missing-body";
    let _ = take_reset_request(SHARD);
    let dag = build_dag().await;
    let block_store = build_block_store().await;
    let manager = Arc::new(common::build_runtime_manager().await);
    let root = StateHash::new(manager.get_history_repo().root().to_byte_array());
    let mut parents = Vec::new();
    for n in 0..=2i64 {
        let h = hash(n as u8);
        let body = block(h, root);
        dag.insert(meta(h, &parents, n, n), body.clone())
            .await
            .expect("metadata is stored");
        if n != 2 {
            block_store.put(&[(h, body)]).await.expect("body is stored");
        }
        parents = vec![h];
    }
    let request = ResetRequest {
        anchor: hash(1),
        anchor_height: BlockHeight::try_from(1).unwrap(),
        blocks_above: 1,
        refused: hash(2),
        reason: ResetReason::SelfReplayFault,
    };
    assert!(rchain_casper::interpreter_util::request_reset(
        SHARD.to_string(),
        request.clone(),
    ));
    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);
    let result = act_on_reset_request(
        SHARD,
        false,
        true,
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &block_store,
        &manager,
        &log,
        LogSource::new("test"),
    )
    .await;
    assert!(
        result.is_err(),
        "the reset cannot recover deploys from a DAG block whose body is missing"
    );
    assert!(
        result.unwrap_err().contains("body is not in the store"),
        "the error identifies the incomplete inventory"
    );
    assert!(dag.get_representation().await.contains(&hash(2)));
    assert!(dag.read_reset_marker().await.unwrap().is_none());
    assert!(has(&block_store, hash(1)).await);
    assert_eq!(reset_request(SHARD), Some(request.clone()));
    assert_eq!(take_reset_request(SHARD), Some(request));
}

/// The healthy control for C366: returning an unfinalised deploy still precedes the rewind, and
/// preserves the exact signed request. This is a storage fixture, not a signature-validation test.
#[tokio::test]
async fn a_self_replay_reset_returns_the_deploy_before_purging_its_only_block() {
    const SHARD: &str = "n564-reset-preserve-deploy";
    let _ = take_reset_request(SHARD);
    let dag = build_dag().await;
    let block_store = build_block_store().await;
    let manager = Arc::new(common::build_runtime_manager().await);
    let root = StateHash::new(manager.get_history_repo().root().to_byte_array());
    let deploy = SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: "Nil".to_string(),
            timestamp: 1,
            phlo_price: 1,
            phlo_limit: 500_000,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        },
        deployer: vec![0u8; 65],
        sig: vec![42u8; 65],
        sig_algorithm: "secp256k1".to_string(),
    };
    let mut parents = Vec::new();
    for n in 0..=1i64 {
        let h = hash(n as u8);
        let mut body = block(h, root);
        if n == 1 {
            body.state.deploys.push(ProcessedDeploy {
                deploy: deploy.clone(),
                cost: Default::default(),
                deploy_log: Vec::new(),
                is_failed: false,
                system_deploy_error: None,
            });
        }
        dag.insert(meta(h, &parents, n, n), body.clone())
            .await
            .expect("metadata is stored");
        block_store.put(&[(h, body)]).await.expect("body is stored");
        parents = vec![h];
    }
    assert!(!dag.contains_deploy_in_pool(&deploy.sig).await.unwrap());
    let request = ResetRequest {
        anchor: hash(0),
        anchor_height: BlockHeight::zero(),
        blocks_above: 1,
        refused: hash(1),
        reason: ResetReason::SelfReplayFault,
    };
    assert!(rchain_casper::interpreter_util::request_reset(
        SHARD.to_string(),
        request,
    ));
    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);
    assert!(act_on_reset_request(
        SHARD,
        false,
        true,
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &block_store,
        &manager,
        &log,
        LogSource::new("test"),
    )
    .await
    .expect("a complete inventory permits the reset")
    .is_some());
    let pooled = dag.pooled_deploys().await.unwrap();
    assert_eq!(pooled.get(&deploy.sig), Some(&deploy));
    assert!(dag
        .lookup_by_deploy_id(&deploy.sig)
        .await
        .unwrap()
        .is_none());
    assert!(!dag.get_representation().await.contains(&hash(1)));
    assert!(!has(&block_store, hash(1)).await);
    assert!(has(&block_store, hash(0)).await);
    assert!(reset_request(SHARD).is_none());
}

/// C366 on disk: a missing body refuses before mutation, and that refusal survives closing and
/// reopening the stores. After an explicit fixture repair, the real reset returns the exact deploy
/// and persists the purge and completion marker. Signature bytes are opaque test data here.
#[tokio::test]
async fn a_disk_backed_reset_preserves_the_inventory_across_store_reopen() {
    const SHARD: &str = "n564-disk-backed-inventory";
    let _ = take_reset_request(SHARD);
    let directory = ResetDiskDir::new();
    let runtime = Arc::new(common::build_runtime_manager().await);
    let root = StateHash::new(runtime.get_history_repo().root().to_byte_array());
    let (disk, dag, blocks) = open_reset_disk_stores(&directory.0).await;
    let deploy = SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: "Nil".to_string(),
            timestamp: 2,
            phlo_price: 1,
            phlo_limit: 500_000,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        },
        deployer: vec![0u8; 65],
        sig: vec![43u8; 65],
        sig_algorithm: "secp256k1".to_string(),
    };
    let mut parents = Vec::new();
    let mut refused_body = block(hash(2), root);
    for n in 0..=2i64 {
        let h = hash(n as u8);
        let mut body = block(h, root);
        body.block_number = BlockHeight::try_from(n).unwrap();
        if n == 2 {
            body.state.deploys.push(ProcessedDeploy {
                deploy: deploy.clone(),
                cost: Default::default(),
                deploy_log: Vec::new(),
                is_failed: false,
                system_deploy_error: None,
            });
            refused_body = body.clone();
        }
        dag.insert(meta(h, &parents, n, n), body.clone())
            .await
            .unwrap();
        blocks.put(&[(h, body)]).await.unwrap();
        parents = vec![h];
    }
    assert_eq!(blocks.delete(&[hash(2)]).await.unwrap(), 1);
    let request = ResetRequest {
        anchor: hash(0),
        anchor_height: BlockHeight::zero(),
        blocks_above: 2,
        refused: hash(2),
        reason: ResetReason::SelfReplayFault,
    };
    assert!(rchain_casper::interpreter_util::request_reset(
        SHARD.to_string(),
        request.clone(),
    ));
    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);
    let result = act_on_reset_request(
        SHARD,
        false,
        true,
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &blocks,
        &runtime,
        &log,
        LogSource::new("test"),
    )
    .await;
    assert!(
        result.is_err(),
        "a missing disk body is not an empty inventory"
    );
    assert!(dag.read_reset_marker().await.unwrap().is_none());
    assert_eq!(take_reset_request(SHARD), Some(request.clone()));
    drop(blocks);
    drop(dag);
    drop(disk);

    let (disk, dag, blocks) = open_reset_disk_stores(&directory.0).await;
    assert!(dag.get_representation().await.contains(&hash(2)));
    assert_eq!(
        dag.lookup_by_deploy_id(&deploy.sig).await.unwrap(),
        Some(hash(2))
    );
    assert!(!has(&blocks, hash(2)).await);
    assert!(has(&blocks, hash(0)).await);
    assert!(dag.read_reset_marker().await.unwrap().is_none());
    assert!(!dag.contains_deploy_in_pool(&deploy.sig).await.unwrap());

    // An explicit fixture repair is the positive control; it is not an automatic recovery claim.
    blocks.put(&[(hash(2), refused_body)]).await.unwrap();
    assert!(rchain_casper::interpreter_util::request_reset(
        SHARD.to_string(),
        request,
    ));
    assert!(act_on_reset_request(
        SHARD,
        false,
        true,
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &blocks,
        &runtime,
        &log,
        LogSource::new("test"),
    )
    .await
    .unwrap()
    .is_some());
    assert_eq!(
        dag.pooled_deploys().await.unwrap().get(&deploy.sig),
        Some(&deploy)
    );
    drop(blocks);
    drop(dag);
    drop(disk);

    let (disk, dag, blocks) = open_reset_disk_stores(&directory.0).await;
    assert_eq!(
        dag.pooled_deploys().await.unwrap().get(&deploy.sig),
        Some(&deploy)
    );
    assert!(dag
        .lookup_by_deploy_id(&deploy.sig)
        .await
        .unwrap()
        .is_none());
    assert!(!dag.get_representation().await.contains(&hash(2)));
    assert!(!has(&blocks, hash(2)).await);
    assert!(has(&blocks, hash(0)).await);
    let marker = dag.read_reset_marker().await.unwrap().unwrap();
    assert_eq!(marker.anchor, hash(0));
    assert_eq!(marker.phase, ResetPhase::Rewound);
    assert!(reset_request(SHARD).is_none());
    drop(blocks);
    drop(dag);
    drop(disk);
}

/// **The reset's crash window, and the boot guard that closes it** (F-U5-01's owed marker, register
/// C274).
///
/// A reset is two store operations — the DAG's rewind and the removal of the bodies it dropped — and a
/// process that dies between them leaves bodies with no DAG entries. That is not an inert state: the
/// receiver reads such a body as *"stored but not yet validated"* and re-injects it, so the next child
/// of a dropped block re-inserts the dropped block **above the anchor** and undoes the reset. Ordering
/// cannot close the window (the hashes to purge are what the rewind returns) and the reverse order is
/// worse, so the node records what it is doing and this test drives the boot that reads the record.
///
/// **The falsifier, stated.** Without the marker the crash is unrepaired: `resume_after_reset` finds
/// nothing to read and the bodies above the anchor stay in the store, which is what the first two
/// assertions would fail on. The test therefore starts *after* the rewind and *before* the purge —
/// exactly the window — rather than making the crash a fault injection.
///
/// What it does not drive: `node_launch::apply`. The guard is a function of its own for the same reason
/// `act_on_reset_request` is — an `apply`-level fixture needs a transport, an importer, a spec and a
/// `NodeRunning` — so the five lines that call it are read rather than run, said out loud here.
#[tokio::test]
async fn a_crash_after_the_rewind_is_repaired_at_boot_by_the_marker() {
    let dag = build_dag().await;
    let block_store = build_block_store().await;
    let manager = Arc::new(common::build_runtime_manager().await);
    // **A root the runtime knows**, so the guard's *third* effect — the runtime reset (law 72) — is the
    // ordinary one and not a fabricated hash. The anchor carries it, as it does in the act's own test.
    let root_state = StateHash::new(manager.get_history_repo().root().to_byte_array());
    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);
    let source = LogSource::new("test");

    // A chain of four, bodies and metadata together — the state a node is in before it resets.
    let mut parents: Vec<BlockHash> = Vec::new();
    for n in 0..4i64 {
        let h = hash(n as u8);
        let post = if n == 1 {
            root_state
        } else {
            StateHash::new([0u8; 32])
        };
        dag.insert(meta(h, &parents, n, n), block(h, post))
            .await
            .expect("the chain inserts");
        block_store
            .put(&[(h, block(h, post))])
            .await
            .expect("the body is stored");
        parents = vec![h];
    }

    let anchor = hash(1);
    let anchor_height = BlockHeight::try_from(1).unwrap();

    // --- the crash --------------------------------------------------------------------------------
    // The reset wrote its intent and rewound the DAG, and the process died before the bodies went. The
    // two calls are `rewind_and_purge` split at its own seam, so this is the window itself and not a
    // reconstruction of it.
    dag.write_reset_marker(ResetMarker {
        anchor,
        height: anchor_height,
        phase: ResetPhase::Rewinding,
    })
    .await
    .expect("the intent is recorded");
    let dropped = dag
        .drop_above(anchor_height)
        .await
        .expect("the rewind runs");
    assert_eq!(dropped.len(), 2, "heights 2 and 3 went from the DAG");
    assert!(
        has(&block_store, hash(2)).await && has(&block_store, hash(3)).await,
        "and their bodies stayed — the crash window the marker exists for"
    );
    assert!(
        !dag.get_representation().await.contains(&hash(3)),
        "the DAG is already at the anchor: the rewind is durable and the purge is not"
    );

    // --- the boot that reads the record -----------------------------------------------------------
    let resumed = resume_after_reset(
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &block_store,
        &manager,
        &log,
        source,
    )
    .await
    .expect("the guard runs over healthy stores");
    assert_eq!(
        resumed,
        Some(anchor),
        "the guard names the anchor the catch-up walk must start from"
    );
    assert!(
        !has(&block_store, hash(2)).await && !has(&block_store, hash(3)).await,
        "the bodies above the anchor are gone, so the receiver has nothing to re-inject"
    );
    assert!(
        has(&block_store, anchor).await,
        "the anchor's own body stays — the reset keeps what it rewinds to"
    );
    assert!(
        dag.get_representation().await.contains(&anchor),
        "and the anchor survives in the view every dependency check reads"
    );

    // **The phase moved, and that is what stops a second boot from rewinding again.** By the next boot
    // this node may have caught up and produced above the anchor; a guard that re-applied `drop_above`
    // would throw those blocks away. So the completion is recorded, and the second call is a no-op.
    let marker = dag
        .read_reset_marker()
        .await
        .expect("the marker is readable")
        .expect("and still present — the catch-up walk clears it, not the guard");
    assert_eq!(marker.phase, ResetPhase::Rewound);
    assert_eq!((marker.anchor, marker.height), (anchor, anchor_height));

    // A block above the anchor, as a caught-up node would have: the second call must leave it alone.
    let above = hash(9);
    dag.insert(
        meta(above, &[anchor], 2, 9),
        block(above, StateHash::new([0u8; 32])),
    )
    .await
    .expect("the node produces above the anchor again");
    let second = resume_after_reset(
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &block_store,
        &manager,
        &log,
        source,
    )
    .await
    .expect("the guard runs again");
    assert_eq!(second, Some(anchor), "the anchor is still what it names");
    assert!(
        dag.get_representation().await.contains(&above),
        "and nothing was rewound a second time — the one way this guard could do harm"
    );
}

/// **The ordinary boot is untouched.** A node that has never reset has no marker, so the guard answers
/// `None` and the caller reconnects exactly as every build before this one did. Without this the guard
/// could pass its own test while changing the boot path of every node in the network.
#[tokio::test]
async fn a_node_that_never_reset_is_not_touched_by_the_reset_guard() {
    let dag = build_dag().await;
    let block_store = build_block_store().await;
    let manager = Arc::new(common::build_runtime_manager().await);
    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);

    let h = hash(0);
    dag.insert(meta(h, &[], 0, 0), block(h, StateHash::new([0u8; 32])))
        .await
        .expect("the chain inserts");
    block_store
        .put(&[(h, block(h, StateHash::new([0u8; 32])))])
        .await
        .expect("the body is stored");

    let resumed = resume_after_reset(
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &block_store,
        &manager,
        &log,
        LogSource::new("test"),
    )
    .await
    .expect("the guard runs");
    assert_eq!(resumed, None, "no marker, so the ordinary path");
    assert!(
        has(&block_store, h).await,
        "and nothing was purged: the guard has no reason to touch a healthy store"
    );
    assert_eq!(
        dag.read_reset_marker().await.expect("readable"),
        None,
        "and it wrote nothing"
    );
}

/// **The guard now reaches the anchor's *body*, because the third effect it re-applies is the runtime
/// root** (law 72, `spec/Rchain/Durability.lean`). Before the fix a `Rewinding` guard re-applied two
/// store operations and never read a body, so an anchor whose body was absent from the block store was
/// invisible to it; with the fix the repair is total over the record's effects and the absent body is a
/// refusal. Without the runtime step this test fails with `Ok(Some(anchor))` — the exact state a
/// crash-window repair must not hand to the catch-up walk.
#[tokio::test]
async fn the_guard_refuses_a_rewinding_whose_anchor_body_is_absent() {
    let dag = build_dag().await;
    let block_store = build_block_store().await;
    let manager = Arc::new(common::build_runtime_manager().await);
    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);

    let anchor = hash(1);
    let anchor_height = BlockHeight::try_from(1).unwrap();
    // The intent is recorded and nothing above the anchor exists, so the rewind and the purge are
    // no-ops — the only thing that can stop the repair is the missing anchor, which is the point.
    dag.write_reset_marker(ResetMarker {
        anchor,
        height: anchor_height,
        phase: ResetPhase::Rewinding,
    })
    .await
    .expect("the intent is recorded");

    let err = resume_after_reset(
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &block_store,
        &manager,
        &log,
        LogSource::new("test"),
    )
    .await
    .expect_err("the guard refuses an anchor whose body it cannot read");
    assert!(
        err.contains("is not in this node's block store"),
        "the refusal is the reset's own sentence about the anchor: {err}"
    );
}

/// **The shard's stall, merge and give-up counters survive a restart** (W-N10-02).
///
/// The row: `STALL_EPISODES`, `NON_QUIET_MERGE_REPORTS` and `UNRESTORABLE_RECORDS` were process-global
/// statics, so an operator who restarted to clear an alarm was told the shard had never stalled. The
/// three are now one shard's cells, written on a *transition* rather than on every observation, and
/// stored beside the reset marker and the halt record.
///
/// **The falsifiers, stated.** (a) Make `write_health_counters` a no-op: the fresh read is `None`, so
/// the seeded cell is zero and the assertion after the seed fails. (b) Drop the boot seed (make
/// `seed_counters` a no-op): the fresh cell stays at zero. (c) **Persist on the observation instead of
/// on the transition** — the first seven refusals below go through the rule the validation path uses
/// ([`ShardHealth::record_finality_stall`]) and the store must still be empty when they are done, so a
/// variant that writes every time is caught here rather than staying green.
#[tokio::test]
async fn the_shard_health_counters_survive_a_restart() {
    use rchain_block_storage::dag::dag_storage::HealthCounters;
    use rchain_casper::interpreter_util::ShardHealth;

    let dag = build_dag().await;

    // --- before the restart: a sustained stall, a non-quiet merge, a block given up on -------------
    let before = ShardHealth::new();
    let partial = rchain_block_storage::dag::finalizer::NoAdvance::Support {
        supporting: 100,
        total: 400,
        full_partitions: 1,
        candidates: 1,
        partition: 4,
        not_full: Default::default(),
    };
    // Seven refusals are *pacing* on a four-sender net, so nothing is written yet: the transition is
    // what the rule persists, and the store must still be empty. This drives the rule the validation
    // path calls rather than the counter and the write separately, so "persist on every observation"
    // is a mutation this can see.
    for _ in 0..7 {
        before
            .record_finality_stall(Some(&partial), dag.as_ref())
            .await
            .expect("a below-threshold observation records nothing and succeeds");
    }
    assert!(
        dag.read_health_counters()
            .await
            .expect("readable")
            .is_none(),
        "a run that has not reached the threshold is not a transition, so nothing is written yet"
    );
    // The eighth refusal reaches `2 * partition` and *is* the transition.
    before
        .record_finality_stall(Some(&partial), dag.as_ref())
        .await
        .expect("the transition persists");
    before.note_non_quiet_merge_report();
    before.note_unrestorable("deadbeef", 42);
    before
        .persist_counters(dag.as_ref())
        .await
        .expect("the give-up persists");

    // --- the restart ------------------------------------------------------------------------------
    let stored = dag
        .read_health_counters()
        .await
        .expect("the record is readable")
        .expect("a transition was recorded");
    assert_eq!(stored.stall_episodes, 1, "one stall episode");
    assert_eq!(stored.non_quiet_merge_reports, 1);
    assert_eq!(stored.unrestorable_records, 1);

    let after = ShardHealth::new();
    assert_eq!(after.stall_episodes(), 0, "a fresh cell starts at zero");
    assert!(!after.counters_durable());
    after.seed_counters(Ok(Some(stored)));
    assert_eq!(after.stall_episodes(), 1, "the shard's stall history");
    assert_eq!(after.non_quiet_merge_reports(), 1, "its merge history");
    assert_eq!(after.unrestorable_records(), 1, "and what it gave up on");
    // **The current-state cells are NOT seeded**, which is the decision rather than an omission: a
    // restarted shard that observed nothing has no current verdict.
    assert_eq!(after.finality_stall_reason(), None);
    assert!(!after.finality_stall_sustained());
    assert_eq!(after.unrestorable_block(), None);
    assert!(
        after.counters_durable(),
        "a seeded cell says its counts are the shard's"
    );

    // --- and a shard that has never transitioned is **not** "unknown" -------------------------------
    // The third shape of the boot read, and the arm a healthy live node hits: nothing on record means
    // the shard's history is zero, which is a fact the node can vouch for.
    let fresh = ShardHealth::new();
    fresh.seed_counters(Ok(None));
    assert_eq!(fresh.stall_episodes(), 0);
    assert!(
        fresh.counters_durable(),
        "a fresh shard's zero is its own history, not an unknown — the live two-shard arm caught the \
         first version reading `false` here"
    );

    // The read that *failed* is the one that is unknown.
    let unread = ShardHealth::new();
    unread.seed_counters(Err("the store refused".to_string()));
    assert!(!unread.counters_durable());

    // --- and a store that cannot answer reads as **unknown**, not as zero -------------------------
    let storeless = build_dag_without_marker_store().await;
    let unknown = ShardHealth::new();
    assert!(
        unknown.persist_counters(storeless.as_ref()).await.is_err(),
        "a backend with nowhere to keep the counters refuses to claim it did"
    );
    assert!(
        !unknown.counters_durable(),
        "and the surface says so rather than letting the zero read as 'never stalled'"
    );
    let _ = HealthCounters::default();
}

/// **A halt survives the restart, and the next boot reads it** (W-N10-01, F-U10-02).
///
/// The row: the autopropose halt was process-scoped where the fault outlives the process, and the
/// operator's *prescribed* remedy for a halt is a restart — so the remedy was exactly what destroyed
/// the record of the fault. This drives the durable record through a real store, the same way
/// [`a_crash_after_the_rewind_is_repaired_at_boot_by_the_marker`] drives the reset marker.
///
/// **The falsifiers, stated.** (a) Make `write_halt_record` a no-op: the fresh read below is `None`
/// and the boot seed says "not halted", so the assertion after the seed fails. (b) Drop the seed —
/// make `seed_halt_from_store` return `Ok(())` without reading, or delete the call in
/// `setup_shard_runtime`: the cell stays un-halted and the same assertion fails, because this drives
/// the *real* `seed_halt_from_store` over a real store rather than `seed_halt` directly. (c) Revert the
/// timer's gate to `failures` only (`ProposeHealth::park_while_halted`): the obedience is a separate
/// assertion, in `node_runtime.rs::a_halted_timer_does_not_tick_until_the_halt_is_cleared`.
///
/// **The third state, which is the whole point.** A read that *fails* must not become a boot refusal
/// and must not read as "never halted": `seed_halt(None)` leaves `durable: false`, which is the surface
/// saying it cannot vouch for the reading. That arm is asserted here as the `None` case.
/// **A self-replay fault survives the restart, and the next boot obeys it** (#564, U23 F-05/F-11).
///
/// The incident's refusal died with the process: the tracker was an in-process cell and the refused
/// body was never stored, so a restart retried the same deterministic input. This drives the durable
/// `SelfReplayRecord` through the same real store and boot seed as the halt record above; a
/// `ResetRequested`/`Halted` stage must latch the next process, while a `Refused`/`Recovered` record
/// is evidence and must not.
#[tokio::test]
async fn a_self_replay_reset_record_latches_the_next_boot() {
    use rchain_block_storage::dag::dag_storage::{
        SelfReplayRecord, SelfReplayStage, SELF_REPLAY_RECORD_VERSION,
    };
    use rchain_casper::api::block_api::ProposeHealth;

    let dag = build_dag().await;
    let record = |stage| SelfReplayRecord {
        version: SELF_REPLAY_RECORD_VERSION,
        at_ms: 1_700_000_000_000,
        block_hash: [7u8; 32],
        height: 1115,
        seq_num: 3,
        code: "replay-cost-mismatch".to_string(),
        initial_cost: Some(5576),
        replay_cost: Some(5302),
        state_hash_matched: Some(true),
        replayed_state_hash: None,
        deploy_ids: vec![vec![7u8; 65]],
        deploy_count: 1,
        stage,
        reset_anchor: Some([4u8; 32]),
        reset_attempted: stage == SelfReplayStage::ResetRequested,
        body: None,
        body_len: 0,
        durable: false,
    };

    // A refusal that did not reach the node-level rung is evidence, not a stop.
    dag.write_self_replay_record(record(SelfReplayStage::Refused))
        .await
        .expect("the refusal is recorded");
    let health = ProposeHealth::new();
    health
        .seed_self_replay_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    assert!(
        !health.self_replay_halted(),
        "a refused-only record must not halt the next process"
    );

    // The node-level rung does latch.
    dag.write_self_replay_record(record(SelfReplayStage::ResetRequested))
        .await
        .expect("the reset request is recorded");
    let health = ProposeHealth::new();
    health
        .seed_self_replay_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    assert!(
        health.self_replay_halted(),
        "a ResetRequested record must halt the next process, not retry the unchanged input"
    );
    assert!(
        health
            .self_replay_halt_reason()
            .is_some_and(|r| r.contains("ResetRequested")),
        "the seeded reason names the stage"
    );

    // The operator resume clears the record; the next boot is clean.
    dag.clear_self_replay_record()
        .await
        .expect("the record is cleared");
    assert!(dag
        .read_self_replay_record()
        .await
        .expect("the store reads")
        .is_none());
    let health = ProposeHealth::new();
    health
        .seed_self_replay_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    assert!(
        !health.self_replay_halted(),
        "no record means no self-replay halt"
    );

    // A `Recovered` record is evidence, not a stop — including the post-reset recovery the automatic
    // resume writes, which keeps `reset_attempted` true so the two recoveries stay distinguishable.
    let mut recovered = record(SelfReplayStage::Recovered);
    recovered.reset_attempted = true;
    dag.write_self_replay_record(recovered)
        .await
        .expect("the recovery is recorded");
    let health = ProposeHealth::new();
    health
        .seed_self_replay_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    assert!(
        !health.self_replay_halted(),
        "a recovered record must not halt the next process"
    );
    let refusal = health
        .snapshot()
        .self_validation_refusal
        .expect("the recovered record is still evidence");
    assert!(refusal.reset_attempted, "the reset attempt survives");
    assert_eq!(refusal.code, "replay-cost-mismatch");
    assert_eq!(refusal.initial_cost, Some(5576));
    assert_eq!(refusal.replay_cost, Some(5302));
    assert_eq!(refusal.deploy_ids.len(), 1);
}

/// A `SelfReplayRecord` fixture for the resume tests: node-level fault at height 1115, one reset
/// attempted at `anchor`, and (when given) a refused body.
fn self_replay_record_for(
    stage: rchain_block_storage::dag::dag_storage::SelfReplayStage,
    anchor: BlockHash,
    body: Option<Vec<u8>>,
    body_len: usize,
) -> rchain_block_storage::dag::dag_storage::SelfReplayRecord {
    rchain_block_storage::dag::dag_storage::SelfReplayRecord {
        version: rchain_block_storage::dag::dag_storage::SELF_REPLAY_RECORD_VERSION,
        at_ms: 1_700_000_000_000,
        block_hash: [7u8; 32],
        height: 1115,
        seq_num: 3,
        code: "replay-cost-mismatch".to_string(),
        initial_cost: Some(5576),
        replay_cost: Some(5302),
        state_hash_matched: Some(true),
        replayed_state_hash: None,
        deploy_ids: vec![vec![7u8; 65]],
        deploy_count: 1,
        stage,
        reset_anchor: Some(*anchor.as_bytes()),
        reset_attempted: true,
        body,
        body_len,
        durable: false,
    }
}

/// **The reset's own exit: once the walk passes the anchor, the self-replay halt clears without an
/// operator POST** (#564, F-U23-11). The record moves store-first, the body/ids/anchor survive, and
/// a fresh boot is not halted.
#[tokio::test]
async fn a_caught_up_self_replay_reset_resumes_without_an_operator_post() {
    use rchain_block_storage::dag::dag_storage::SelfReplayStage;
    use rchain_casper::api::block_api::ProposeHealth;
    use rchain_casper::engine::reset::resume_self_replay_after_catchup;

    let dag = build_dag().await;
    let anchor = hash(4);
    let body = vec![0xA5u8; 11];
    dag.write_self_replay_record(self_replay_record_for(
        SelfReplayStage::Halted,
        anchor,
        Some(body.clone()),
        body.len(),
    ))
    .await
    .expect("the fault is recorded");

    let health = ProposeHealth::new();
    health
        .seed_self_replay_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    assert!(health.self_replay_halted(), "the boot seed latches");

    let before = health
        .snapshot()
        .self_validation_refusal
        .expect("the refusal is evidence");
    assert!(before.refused_body_available);
    assert_eq!(before.refused_body_len, body.len() as u64);
    assert!(before.reset_attempted);
    assert!(before.reset_anchor.is_some());

    let marker = ResetMarker {
        anchor,
        height: BlockHeight::try_from(10).unwrap(),
        phase: ResetPhase::Rewound,
    };
    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);
    assert!(
        resume_self_replay_after_catchup(dag.as_ref(), &marker, 11, &health, &log).await,
        "caught up past the anchor resumes"
    );
    assert!(
        !health.self_replay_halted(),
        "the latch is cleared without an operator POST"
    );

    let recovered = dag
        .read_self_replay_record()
        .await
        .expect("the store reads")
        .expect("the record is kept as evidence");
    assert_eq!(recovered.stage, SelfReplayStage::Recovered);
    assert!(recovered.reset_attempted);
    assert_eq!(recovered.body.as_deref(), Some(body.as_slice()));
    assert_eq!(recovered.body_len, body.len());

    // A fresh boot is not halted, and the surface still carries the evidence.
    let rebooted = ProposeHealth::new();
    rebooted
        .seed_self_replay_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    assert!(!rebooted.self_replay_halted());
    let after = rebooted
        .snapshot()
        .self_validation_refusal
        .expect("the recovered record is still evidence");
    assert!(after.refused_body_available);
    assert!(after.reset_attempted);

    // Idempotent: the second call sees `Recovered` and does nothing.
    assert!(
        !resume_self_replay_after_catchup(dag.as_ref(), &marker, 12, &health, &log).await,
        "a recovered record is not resumed twice"
    );
}

/// **Below the anchor is not caught up** (#564). The reset's own rule is `frontier > height`; equal
/// is still inside the gap the walk exists to close.
#[tokio::test]
async fn a_self_replay_reset_below_its_anchor_does_not_resume() {
    use rchain_block_storage::dag::dag_storage::SelfReplayStage;
    use rchain_casper::api::block_api::ProposeHealth;
    use rchain_casper::engine::reset::resume_self_replay_after_catchup;

    let dag = build_dag().await;
    let anchor = hash(4);
    dag.write_self_replay_record(self_replay_record_for(
        SelfReplayStage::Halted,
        anchor,
        None,
        0,
    ))
    .await
    .expect("the fault is recorded");
    let health = ProposeHealth::new();
    health
        .seed_self_replay_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    assert!(health.self_replay_halted());

    let marker = ResetMarker {
        anchor,
        height: BlockHeight::try_from(10).unwrap(),
        phase: ResetPhase::Rewound,
    };
    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);
    assert!(
        !resume_self_replay_after_catchup(dag.as_ref(), &marker, 10, &health, &log).await,
        "a frontier at the anchor has not passed it"
    );
    assert!(health.self_replay_halted(), "the halt stands");
}

/// **A reset at another anchor does not clear this fault** (#564). The anchor equality is what keeps
/// a later divergence-budget reset from resuming a self-replay fault it did not fix.
#[tokio::test]
async fn a_marker_from_another_anchors_reset_does_not_clear_the_self_replay_fault() {
    use rchain_block_storage::dag::dag_storage::SelfReplayStage;
    use rchain_casper::api::block_api::ProposeHealth;
    use rchain_casper::engine::reset::resume_self_replay_after_catchup;

    let dag = build_dag().await;
    dag.write_self_replay_record(self_replay_record_for(
        SelfReplayStage::Halted,
        hash(5),
        None,
        0,
    ))
    .await
    .expect("the fault is recorded");
    let health = ProposeHealth::new();
    health
        .seed_self_replay_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    assert!(health.self_replay_halted());

    let marker = ResetMarker {
        anchor: hash(4),
        height: BlockHeight::try_from(10).unwrap(),
        phase: ResetPhase::Rewound,
    };
    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);
    assert!(
        !resume_self_replay_after_catchup(dag.as_ref(), &marker, 11, &health, &log).await,
        "a reset at a different anchor is not this fault's reset"
    );
    assert!(health.self_replay_halted(), "the halt stands");
}

/// **An over-cap body is visible as unavailable, with its true length** (#564, C359/C365). The record
/// keeps `body_len` when the body does not fit, so the surface can say "it existed and was too large"
/// rather than implying there was none.
#[tokio::test]
async fn an_over_cap_refused_body_is_visible_as_unavailable_with_its_length() {
    use rchain_block_storage::dag::dag_storage::SelfReplayStage;
    use rchain_casper::api::block_api::ProposeHealth;

    let dag = build_dag().await;
    dag.write_self_replay_record(self_replay_record_for(
        SelfReplayStage::Halted,
        hash(4),
        None,
        70_000,
    ))
    .await
    .expect("the fault is recorded");
    let health = ProposeHealth::new();
    health
        .seed_self_replay_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    let refusal = health
        .snapshot()
        .self_validation_refusal
        .expect("the record is evidence");
    assert!(!refusal.refused_body_available);
    assert_eq!(refusal.refused_body_len, 70_000);
}

/// **The switch-off path writes no marker, so there is nothing to resume past** (#564, F-U23-11).
/// With `casper.self-replay-reset=false` the request is consumed and the node keeps the halt; the
/// automatic resume is not reachable, and the operator POST is the one way out.
#[tokio::test]
async fn a_self_replay_reset_with_the_switch_off_writes_no_marker() {
    use rchain_block_storage::dag::dag_storage::SelfReplayStage;
    use rchain_casper::api::block_api::ProposeHealth;

    const LANE_OFF: &str = "test-shard-self-replay-off";
    let _ = take_reset_request(LANE_OFF);

    let dag = build_dag().await;
    let block_store = build_block_store().await;
    let manager = Arc::new(common::build_runtime_manager().await);
    let request = ResetRequest {
        anchor: hash(4),
        anchor_height: BlockHeight::try_from(10).unwrap(),
        blocks_above: 2,
        refused: hash(7),
        reason: ResetReason::SelfReplayFault,
    };
    assert!(
        rchain_casper::interpreter_util::request_reset(LANE_OFF.to_string(), request),
        "the fault raises its request"
    );
    let log: Arc<dyn rchain_shared::log::Log> = Arc::new(NopLog);
    let acted = act_on_reset_request(
        LANE_OFF,
        true,
        false,
        &(dag.clone() as Arc<dyn BlockDagStorage>),
        &block_store,
        &manager,
        &log,
        LogSource::new("test"),
    )
    .await
    .expect("a switched-off reset is not an error");
    assert!(acted.is_none(), "the switch is off, so nothing acted");
    assert!(
        dag.read_reset_marker()
            .await
            .expect("the store reads")
            .is_none(),
        "no reset means no marker to catch up past"
    );

    dag.write_self_replay_record(self_replay_record_for(
        SelfReplayStage::Halted,
        hash(4),
        None,
        0,
    ))
    .await
    .expect("the fault is recorded");
    let health = ProposeHealth::new();
    health
        .seed_self_replay_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    assert!(
        health.self_replay_halted(),
        "the halt stands until the operator resumes"
    );
}

#[tokio::test]
async fn a_halt_written_before_the_restart_is_read_at_the_next_boot() {
    use rchain_block_storage::dag::dag_storage::{HaltRecord, HALT_RECORD_KEY};
    use rchain_casper::api::block_api::ProposeHealth;

    let dag = build_dag().await;

    // --- the halt, before the restart -------------------------------------------------------------
    let written = HaltRecord {
        at_ms: 1_700_000_000_000,
        episodes: 2,
        durable: false,
    };
    dag.write_halt_record(written)
        .await
        .expect("the halt reaches the store");

    // --- the next boot, over the same store --------------------------------------------------------
    let read = dag
        .read_halt_record()
        .await
        .expect("the record is readable")
        .expect("a halt was written, so one is on record");
    assert_eq!(
        read.at_ms, written.at_ms,
        "the instant survives the restart"
    );
    assert_eq!(read.episodes, 2, "and the count with it");
    assert!(
        read.durable,
        "a value that came out of the store is durable by construction — the decode sets the flag, \
         so a record read back is never reported as un-vouched"
    );

    // **The boot seed, through the real store** — the same call `setup_shard_runtime` makes, so
    // dropping *that* line (the mutation this arm exists to catch) reddens this assertion.
    let health = ProposeHealth::new();
    assert!(!health.timer_halted(), "a fresh cell is not halted");
    health
        .seed_halt_from_store(dag.as_ref())
        .await
        .expect("a healthy store reads");
    assert!(
        health.timer_halted(),
        "the boot seed makes a restarted node report the halt the flag alone would have lost"
    );
    assert_eq!(
        health.snapshot().autopropose_halt.episodes,
        2,
        "and the episode count is the shard's, not this process's"
    );
    assert_eq!(
        health.snapshot().autopropose_halt.at,
        Some(1_700_000_000_000),
        "and the date the halt was taken, which the surface carries beside the flag"
    );

    // --- the one way out ---------------------------------------------------------------------------
    dag.clear_halt_record()
        .await
        .expect("the clear reaches the store");
    assert!(
        dag.read_halt_record()
            .await
            .expect("readable after a clear")
            .is_none(),
        "the clear is durable — a node that resumed and restarted does not halt again on a stale record"
    );

    // --- the third state: unknown is not empty -----------------------------------------------------
    // A store that cannot be read seeds 'unknown' — `durable: false` — which reads differently from
    // 'never halted'. Driven through the same call, over a DAG with no durable-records store attached.
    let storeless = build_dag_without_marker_store().await;
    let unknown = ProposeHealth::new();
    assert!(
        unknown
            .seed_halt_from_store(storeless.as_ref())
            .await
            .is_err(),
        "a backend with nowhere to keep the record refuses to answer, rather than answering empty"
    );
    assert!(!unknown.timer_halted());
    assert!(
        !unknown.snapshot().autopropose_halt.durable,
        "a read that failed seeds 'unknown', which reads differently from 'never halted'"
    );

    // The reserved key is a property of the store, not of this test: assert it is the one the write
    // used, so a future second key cannot silently split the record from its reader.
    assert_eq!(HALT_RECORD_KEY, b"autopropose-halt");
}

/// **A reset record that contradicts the operator's anchor refuses the boot** (W-N11-01).
///
/// The row: at boot the store's reset record chose where the node rewinds, and an operator-named
/// `--sync-anchor` that disagreed with it was only *warned about* — so on a data directory the
/// operator did not create, the record (a planted one, in the wargame's case) selected the restore
/// point ahead of the operator's stated intent. C274 made the record win on the merits — it is the
/// state the store actually holds — and what was missing is that two contradicting anchors are not a
/// state to pick from in silence. The decision is `boot_anchor_after_reset`: agreement and absence are
/// silent, a contradiction names **both** anchors and chooses neither.
///
/// **What this does not cover, said rather than implied.** The boot path itself — `NodeLaunch::apply`
/// calls this — needs a transport, a runtime and a comm layer, and is not driven here. The call site
/// is one statement the compiler holds; the decision is what the row is about.
#[test]
fn a_reset_record_that_contradicts_the_named_anchor_refuses_boot() {
    use rchain_casper::engine::reset::boot_anchor_after_reset;

    let recorded = BlockHash::new([3u8; 32]);
    let named = BlockHash::new([4u8; 32]);

    assert_eq!(
        boot_anchor_after_reset(Some(recorded), recorded)
            .expect("agreement is not a contradiction"),
        recorded,
        "an anchor that agrees with the record boots on the record, with no complaint"
    );
    assert_eq!(
        boot_anchor_after_reset(None, recorded).expect("nothing named"),
        recorded,
        "with nothing named there is nothing to contradict"
    );

    let err = boot_anchor_after_reset(Some(named), recorded)
        .expect_err("a contradiction is not a choice the node makes for the operator");
    assert!(
        err.contains(&named.to_hex()),
        "both anchors are named: {err}"
    );
    assert!(
        err.contains(&recorded.to_hex()),
        "both anchors are named: {err}"
    );
}
