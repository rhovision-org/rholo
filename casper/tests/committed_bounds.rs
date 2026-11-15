//! Committed validity bounds and the startup/restore boundary, on real persisted state.
mod common;
use rchain_block_storage::approved_store::ApprovedStore;
use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::codecs::{
    BlockHashCodec, BlockMessageCodec, BlockMetadataCodec, SignedDeployDataCodec,
};
use rchain_block_storage::dag::dag_storage::BlockDagStorage;
use rchain_block_storage::syntax::{insert_genesis, put_approved_block, put_block};
use rchain_casper::block_metadata_store::BlockMetadataStore;
use rchain_casper::block_status::BlockStatus;
use rchain_casper::engine::node_launch::{
    check_running_dev_vesting, check_running_validation_bounds,
};
use rchain_casper::interpreter_util::{empty_state_hash_fixed, ShardHealth};
use rchain_casper::multi_parent_casper::{validate, ValidateError};
use rchain_casper::runtime_manager::RuntimeManager;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
use rchain_crypto::signatures::signed::Signed;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{
    BlockMessage, DeployData, FinalizedFringe, PCost, ProcessedDeploy, RholangState,
    SignedDeployData,
};
use rchain_models::validator::Validator;
use rchain_rholang::native_state::{pos_dev_vesting_key, pos_params_key, DevVesting, PosParams};
use rchain_rspace::native_store::PREFIX_POS;
use rchain_shared::log::{Log, NopLog};
use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};
use rchain_shared::store::{InMemoryKeyValueStore, KeyValueStore};
use rchain_shared::store_manager::InMemoryStoreManager;
use rchain_shared::typed_store::{BytesCodec, KeyValueTypedStoreCodec};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
async fn build_dag() -> Arc<rchain_casper::dag::BlockDagKeyValueStorage> {
    fn in_memory() -> Arc<tokio::sync::Mutex<Box<dyn KeyValueStore + Send + Sync>>> {
        Arc::new(tokio::sync::Mutex::new(Box::new(
            InMemoryKeyValueStore::default(),
        )))
    }
    let metadata_store = Arc::new(
        BlockMetadataStore::create(Arc::new(KeyValueTypedStoreCodec::new(
            in_memory(),
            Arc::new(BlockHashCodec),
            Arc::new(BlockMetadataCodec),
        )))
        .await
        .expect("metadata store"),
    );
    let deploy_index: Arc<
        dyn rchain_shared::typed_store::KeyValueTypedStore<
            rchain_block_storage::dag::dag_storage::DeployId,
            BlockHash,
        >,
    > = Arc::new(KeyValueTypedStoreCodec::new(
        in_memory(),
        Arc::new(BytesCodec),
        Arc::new(BlockHashCodec),
    ));
    let deploy_store: Arc<
        dyn rchain_shared::typed_store::KeyValueTypedStore<
            rchain_block_storage::dag::dag_storage::DeployId,
            SignedDeployData,
        >,
    > = Arc::new(KeyValueTypedStoreCodec::new(
        in_memory(),
        Arc::new(BytesCodec),
        Arc::new(SignedDeployDataCodec),
    ));
    Arc::new(
        rchain_casper::dag::BlockDagKeyValueStorage::create(
            metadata_store,
            deploy_index,
            deploy_store,
        )
        .await
        .expect("dag storage"),
    )
}

fn block(byte: u8, height: i64, root: StateHash) -> BlockMessage {
    BlockMessage {
        version: 1,
        shard_id: "root".to_string(),
        block_hash: BlockHash::new([byte; 32]),
        block_number: BlockHeight::try_from(height).unwrap(),
        sender: Validator::new([byte; 65]),
        seq_num: SeqNum::zero(),
        pre_state_hash: root,
        post_state_hash: root,
        justifications: vec![],
        bonds: BTreeMap::new(),
        rejected_deploys: BTreeSet::new(),
        rejected_blocks: BTreeSet::new(),
        rejected_senders: BTreeSet::new(),
        state: RholangState::default(),
        sig_algorithm: "secp256k1".to_string(),
        sig: vec![],
        timestamp: height,
    }
}
fn priced_deploy(price: i64) -> ProcessedDeploy {
    let (secret, _) = Secp256k1.new_key_pair();
    let data = DeployData {
        attachments: vec![],
        term: "Nil".to_string(),
        timestamp: 0,
        phlo_price: price,
        phlo_limit: 100,
        valid_after_block_number: 0,
        shard_id: "root".to_string(),
    };
    let signed = Signed::new(data, &Secp256k1, &secret).unwrap();
    ProcessedDeploy {
        deploy: SignedDeployData {
            data: signed.data,
            deployer: signed.pk.bytes().to_vec(),
            sig: signed.sig,
            sig_algorithm: signed.sig_algorithm.name().to_string(),
        },
        cost: PCost { cost: 0 },
        deploy_log: vec![],
        is_failed: false,
        system_deploy_error: None,
    }
}
async fn state_root(
    runtime: &RuntimeManager,
    params: Option<Vec<u8>>,
    vesting: Option<Vec<u8>>,
) -> StateHash {
    runtime
        .runtime()
        .reset(empty_state_hash_fixed())
        .await
        .unwrap();
    if let Some(bytes) = params {
        runtime
            .runtime()
            .native_store()
            .put(PREFIX_POS, pos_params_key(), bytes);
    }
    if let Some(bytes) = vesting {
        runtime
            .runtime()
            .native_store()
            .put(PREFIX_POS, pos_dev_vesting_key(), bytes);
    }
    runtime
        .runtime()
        .create_checkpoint()
        .await
        .unwrap()
        .root
        .into()
}

async fn params_root(runtime: &RuntimeManager, bytes: Option<Vec<u8>>) -> StateHash {
    state_root(runtime, bytes, None).await
}

/// A DAG with one height-0 block whose post-state is `root`, and an approved fringe of the shape a
/// restore leaves: `hashes: [genesis]` with `state_hash: root` for a joiner, or the creator's empty
/// fringe naming the empty pre-state.
async fn genesis_fixture(
    root: StateHash,
    creator_fringe: bool,
) -> (
    Arc<rchain_casper::dag::BlockDagKeyValueStorage>,
    BlockStore,
    ApprovedStore,
) {
    let dag = build_dag().await;
    let store = blocks();
    let approved = rchain_block_storage::approved_store::create(&InMemoryStoreManager::default())
        .await
        .unwrap();
    let genesis = block(1, 0, root);
    put_block(&store, genesis.clone()).await.unwrap();
    insert_genesis(dag.as_ref(), genesis.clone()).await.unwrap();
    let fringe = if creator_fringe {
        FinalizedFringe {
            hashes: vec![],
            state_hash: empty_state_hash_fixed().into(),
            ancestry: vec![],
        }
    } else {
        FinalizedFringe {
            hashes: vec![genesis.block_hash],
            state_hash: root,
            ancestry: vec![],
        }
    };
    put_approved_block(&approved, fringe).await.unwrap();
    (dag, store, approved)
}

/// A genesis at `genesis_root` and a restored later state at `later_root`.
async fn later_fixture(
    genesis_root: StateHash,
    later_root: StateHash,
) -> (
    Arc<rchain_casper::dag::BlockDagKeyValueStorage>,
    BlockStore,
    ApprovedStore,
) {
    let dag = build_dag().await;
    let store = blocks();
    let approved = rchain_block_storage::approved_store::create(&InMemoryStoreManager::default())
        .await
        .unwrap();
    let genesis = block(1, 0, genesis_root);
    put_block(&store, genesis.clone()).await.unwrap();
    insert_genesis(dag.as_ref(), genesis).await.unwrap();
    // The later block is what makes `later_root` a block post-state, not a fringe-only root; the
    // guard reads the newest block's post-state because that is where the native leaves live.
    let later = block(2, 1, later_root);
    put_block(&store, later.clone()).await.unwrap();
    let mut metadata = rchain_models::block_metadata::BlockMetadata::from_block(&later);
    metadata.validated = true;
    metadata.validation_failed = false;
    dag.insert(metadata, later).await.unwrap();
    put_approved_block(
        &approved,
        FinalizedFringe {
            hashes: vec![BlockHash::new([2; 32])],
            state_hash: later_root,
            ancestry: vec![],
        },
    )
    .await
    .unwrap();
    (dag, store, approved)
}
fn params(floor: i64, parents: i64) -> Vec<u8> {
    PosParams {
        min_phlo_price: NonNegI64::try_from(floor).unwrap(),
        max_number_of_parents: parents,
        ..PosParams::default()
    }
    .encode()
}
fn blocks() -> BlockStore {
    Arc::new(KeyValueTypedStoreCodec::new(
        Arc::new(tokio::sync::Mutex::new(Box::new(
            InMemoryKeyValueStore::default(),
        ))),
        Arc::new(BlockHashCodec),
        Arc::new(BlockMessageCodec),
    ))
}

/// A snapshot-restore shape: the approved fringe names `fringe_params`'s root, while the newest
/// block's post-state carries `block_params`. The two roots are real checkpoints, so reading the
/// wrong one is observable rather than a missing-block accident.
async fn snapshot_fixture(
    runtime: &RuntimeManager,
    fringe_params: Option<Vec<u8>>,
    block_params: Vec<u8>,
) -> (
    Arc<rchain_casper::dag::BlockDagKeyValueStorage>,
    BlockStore,
    rchain_block_storage::approved_store::ApprovedStore,
) {
    let fringe_root = params_root(runtime, fringe_params).await;
    let block_root = params_root(runtime, Some(block_params)).await;
    let dag = build_dag().await;
    let store = blocks();
    let approved = rchain_block_storage::approved_store::create(&InMemoryStoreManager::default())
        .await
        .unwrap();

    let genesis = block(1, 0, block_root);
    put_block(&store, genesis.clone()).await.unwrap();
    insert_genesis(dag.as_ref(), genesis).await.unwrap();
    let latest = block(2, 1, block_root);
    put_block(&store, latest.clone()).await.unwrap();
    let mut metadata = rchain_models::block_metadata::BlockMetadata::from_block(&latest);
    metadata.validated = true;
    metadata.validation_failed = false;
    dag.insert(metadata, latest.clone()).await.unwrap();

    put_approved_block(
        &approved,
        FinalizedFringe {
            hashes: vec![latest.block_hash],
            state_hash: fringe_root,
            ancestry: vec![],
        },
    )
    .await
    .unwrap();
    (dag, store, approved)
}

#[tokio::test]
async fn committed_bounds_control_the_block_path_with_opposing_local_configs() {
    let runtime = common::build_runtime_manager().await;
    let root = params_root(&runtime, Some(params(2, 2))).await;
    let dag = build_dag().await;
    let store = blocks();
    let mut parents = vec![];
    for byte in 1..=3 {
        let parent = block(byte, 0, root);
        parents.push(parent.block_hash);
        put_block(&store, parent.clone()).await.unwrap();
        insert_genesis(dag.as_ref(), parent).await.unwrap();
    }
    let log: Arc<dyn Log> = Arc::new(NopLog);
    let health = ShardHealth::default();
    let index =
        |_| async { Err("a bound refusal must happen before indexing/merge/replay".to_string()) };
    for (floor, cap) in [(0, 0), (99, 1)] {
        for (price, count, expected) in [
            (1, 1, Some(BlockStatus::ContainsLowCostDeploy)),
            (2, 3, Some(BlockStatus::TooManyJustifications)),
            (2, 2, None),
        ] {
            let mut child = block(9, 1, root);
            child.justifications = parents[..count].to_vec();
            child.state.deploys = vec![priced_deploy(price)];
            if let Some(expected) = expected {
                match validate(
                    dag.as_ref(),
                    &store,
                    &runtime,
                    &child,
                    "root",
                    floor,
                    cap,
                    &index,
                    &log,
                    &health,
                )
                .await
                {
                    Err(ValidateError::ValidationFailed(_, actual)) => assert_eq!(actual, expected),
                    other => panic!("expected {expected:?} before replay, got {other:?}"),
                }
            } else {
                let bounds =
                    async { Ok(runtime.committed_validation_bounds(&root).await?.unwrap()) };
                assert_eq!(
                    rchain_casper::validate::block_summary_from_state(
                        dag.as_ref(),
                        &store,
                        &child,
                        "root",
                        100,
                        bounds
                    )
                    .await
                    .unwrap(),
                    Ok(())
                );
            }
        }
    }
    // A peer cannot lower the bounds by naming some other locally available pre-state root.
    let decoy = params_root(&runtime, Some(params(0, 0))).await;
    let mut forged = block(9, 1, decoy);
    forged.justifications = vec![parents[0]];
    forged.state.deploys = vec![priced_deploy(1)];
    assert!(matches!(
        validate(
            dag.as_ref(),
            &store,
            &runtime,
            &forged,
            "root",
            0,
            0,
            &index,
            &log,
            &health
        )
        .await,
        Err(ValidateError::ValidationFailed(
            _,
            BlockStatus::ContainsLowCostDeploy
        ))
    ));

    // Synced records may not have the locally-validated flag; failed parents must not supply
    // their claimed post-state either. Both paths retain the oldest accepted immutable params.
    for (byte, validated, failed) in [(4, false, false), (5, true, true)] {
        let mut parent = block(byte, 1, decoy);
        parent.justifications = vec![parents[0]];
        let mut metadata = rchain_models::block_metadata::BlockMetadata::from_block(&parent);
        metadata.validated = validated;
        metadata.validation_failed = failed;
        put_block(&store, parent.clone()).await.unwrap();
        dag.insert(metadata, parent.clone()).await.unwrap();
        let mut child = block(9, 2, root);
        child.justifications = vec![parent.block_hash];
        child.state.deploys = vec![priced_deploy(1)];
        assert!(matches!(
            validate(
                dag.as_ref(),
                &store,
                &runtime,
                &child,
                "root",
                0,
                0,
                &index,
                &log,
                &health
            )
            .await,
            Err(ValidateError::ValidationFailed(
                _,
                BlockStatus::ContainsLowCostDeploy
            ))
        ));
    }
    // The selected reader is pinned to persisted state, even when the live runtime has another leaf.
    assert_eq!(
        runtime.committed_validation_bounds(&root).await.unwrap(),
        Some((2, 2))
    );
}

#[tokio::test]
async fn restore_guard_checks_committed_zero_legacy_and_bad_records_before_running() {
    let runtime = common::build_runtime_manager().await;
    let dag = build_dag().await;
    let store = blocks();
    let approved = rchain_block_storage::approved_store::create(&InMemoryStoreManager::default())
        .await
        .unwrap();
    let mut cases = vec![
        (Some(params(2, 2)), 2, 2, true),
        (Some(params(2, 2)), 1, 2, false),
        (Some(params(2, 2)), 2, 3, false),
        (Some(params(0, 0)), 0, 0, true),
        (Some(params(0, 0)), 1, 0, false),
        (None, 0, 0, false),
    ];
    let current = params(2, 2);
    for fields in 5..=8 {
        cases.push((Some(current[1..1 + fields * 8].to_vec()), 99, 1, true));
    }
    let mut unknown = current.clone();
    // Format 2 is current; the next unknown format is 3. The truncated record below is 81 bytes —
    // format 1's length — and is refused because this node reads format 2 only.
    unknown[0] = 3;
    cases.push((Some(unknown), 2, 2, false));
    cases.push((Some(current[..current.len() - 8].to_vec()), 2, 2, false));
    for (bytes, floor, cap, allowed) in cases {
        let root = params_root(&runtime, bytes).await;
        put_approved_block(
            &approved,
            FinalizedFringe {
                hashes: vec![BlockHash::new([1; 32])],
                state_hash: root,
                ancestry: vec![],
            },
        )
        .await
        .unwrap();
        let result =
            check_running_validation_bounds(dag.as_ref(), &store, &approved, &runtime, floor, cap)
                .await;
        assert_eq!(
            result.is_ok(),
            allowed,
            "floor {floor}, cap {cap}, result {result:?}"
        );
    }
    // A genesis fringe names the empty pre-state: the guard must inspect genesis post-state.
    let root = params_root(&runtime, Some(params(0, 0))).await;
    let genesis = block(1, 0, root);
    put_block(&store, genesis.clone()).await.unwrap();
    insert_genesis(dag.as_ref(), genesis).await.unwrap();
    put_approved_block(
        &approved,
        FinalizedFringe {
            hashes: vec![],
            state_hash: empty_state_hash_fixed().into(),
            ancestry: vec![],
        },
    )
    .await
    .unwrap();
    assert!(
        check_running_validation_bounds(dag.as_ref(), &store, &approved, &runtime, 0, 0)
            .await
            .is_ok()
    );
    assert!(
        check_running_validation_bounds(dag.as_ref(), &store, &approved, &runtime, 1, 0)
            .await
            .is_err()
    );
}

// A DAG that references a block without its body must not allow the restore guard
// to fall back to the approved fringe's native state, even when the bounds match.
#[tokio::test]
async fn restore_guard_reads_the_post_state_from_metadata_when_the_body_is_missing() {
    // **A metadata-only restore has no body, and that is not a gap** (sync/verification
    // separation): the accepted record carries the block's post-state, and the guard reads it from
    // there. Deleting the body leaves the guard's input intact — a node that could not do this would
    // refuse every checkpoint join. A record with neither a post-state nor a body is the refusal
    // case, and `post_state_of` names it.
    let runtime = common::build_runtime_manager().await;
    let (dag, store, approved) = snapshot_fixture(&runtime, Some(params(2, 2)), params(2, 2)).await;

    let missing = BlockHash::new([2; 32]);
    assert_eq!(store.delete(&[missing]).await.unwrap(), 1);

    check_running_validation_bounds(dag.as_ref(), &store, &approved, &runtime, 2, 2)
        .await
        .expect("the post-state is in the metadata, so a missing body is not a refusal");
}

/// **The restore guard reads the newest block's post-state, not the approved fringe's
/// `state_hash`.**
///
/// A snapshot restore's approved fringe names a *fringe* state; the native leaves (the committed PoS
/// params) are written in a block's post-state. The old guard read the fringe root, found no params
/// leaf and refused a good restore with "PoS params leaf not found in committed state". This test
/// makes the two roots distinct: the fringe root has no leaf, the newest block's post-state carries
/// `(2,2)`. The old guard fails on the first assertion; the fixed guard passes it and still refuses a
/// genuinely different configured bound.
#[tokio::test]
async fn restore_guard_reads_the_newest_block_post_state_not_a_fringe_state() {
    let runtime = common::build_runtime_manager().await;
    let (dag, store, approved) = snapshot_fixture(&runtime, None, params(2, 2)).await;

    assert!(
        check_running_validation_bounds(dag.as_ref(), &store, &approved, &runtime, 2, 2)
            .await
            .is_ok(),
        "the guard must read the block post-state, where the params leaf is"
    );
    assert!(
        check_running_validation_bounds(dag.as_ref(), &store, &approved, &runtime, 1, 2)
            .await
            .is_err(),
        "a different configured bound is still refused"
    );
}

/// **A fringe state with *different* bounds must not be used either.**
///
/// If the guard read the fringe root it would compare `(1,1)` against the configured `(2,2)` and
/// refuse; the block's post-state carries `(2,2)`, so the restore is valid. The second assertion is
/// the other direction: configured `(1,1)` is refused because the committed block state is `(2,2)`.
#[tokio::test]
async fn restore_guard_ignores_a_fringe_state_with_different_bounds() {
    let runtime = common::build_runtime_manager().await;
    let (dag, store, approved) = snapshot_fixture(&runtime, Some(params(1, 1)), params(2, 2)).await;

    assert!(
        check_running_validation_bounds(dag.as_ref(), &store, &approved, &runtime, 2, 2)
            .await
            .is_ok(),
        "the fringe root's different bounds must not be the guard's reading"
    );
    assert!(
        check_running_validation_bounds(dag.as_ref(), &store, &approved, &runtime, 1, 1)
            .await
            .is_err(),
        "the committed block state is (2,2), not the fringe's (1,1)"
    );
}

/// **The restored developer allocation is checked before Running, and only a local claim is
/// enforced.**
///
/// The leaf's own schedule is checked in every case: at the genesis post-state nothing has vested,
/// and at a later state the unreleased amount cannot exceed the allocation. A local claim — the
/// joiner's own bonds/wallets derivation — must match the imported genesis; a joiner with no claim
/// trusts the chain and is not refused. Each case gets its own DAG and approved store, so one
/// acceptance cannot mask another's refusal.
#[tokio::test]
async fn restore_guard_checks_the_developer_allocation_before_running() {
    let runtime = common::build_runtime_manager().await;
    let address = "11112VYAt8rUGNRRZX3eJdgagaAhtWTK8Js7F7X5iqddMVqyDTtYau".to_string();
    let other_address = "11112VYAt8rUGNRRZX3eJdgagaAhtWTK8Js7F7X5iqddMVqyDTtYav".to_string();
    let allocation = 2_000_000_000_000_000i64;
    let leaf = |unreleased: i64| {
        DevVesting {
            address: address.clone(),
            allocation: NonNegI64::try_from(allocation).unwrap(),
            unreleased: NonNegI64::try_from(unreleased).unwrap(),
        }
        .encode()
    };
    let matching_claim = Some((address.clone(), NonNegI64::try_from(allocation).unwrap()));

    // Genesis, both fringe shapes, internally consistent: accepted. The empty fringe is the
    // creator's own; `hashes: [genesis]` is what a genesis-only joiner actually carries.
    for creator in [true, false] {
        let root = state_root(&runtime, None, Some(leaf(allocation))).await;
        let (dag, store, approved) = genesis_fixture(root, creator).await;
        assert!(
            check_running_dev_vesting(dag.as_ref(), &store, &approved, &runtime, None)
                .await
                .is_ok(),
            "a genesis with nothing vested is accepted (creator_fringe={creator})"
        );
    }

    // Genesis with part of the schedule already released: refused. `release_dev_tax` would compute
    // a negative already-released amount from such a leaf and over-credit the recipient.
    let root = state_root(&runtime, None, Some(leaf(allocation / 2))).await;
    let (dag, store, approved) = genesis_fixture(root, false).await;
    let err = check_running_dev_vesting(dag.as_ref(), &store, &approved, &runtime, None)
        .await
        .expect_err("a genesis leaf with a released amount is inconsistent");
    assert!(err.contains("unreleased"), "{err}");

    // A matching local claim is accepted.
    let root = state_root(&runtime, None, Some(leaf(allocation))).await;
    let (dag, store, approved) = genesis_fixture(root, false).await;
    assert!(check_running_dev_vesting(
        dag.as_ref(),
        &store,
        &approved,
        &runtime,
        matching_claim.clone()
    )
    .await
    .is_ok());

    // A local claim that differs in address, or in amount, is refused.
    for claim in [
        Some((other_address, NonNegI64::try_from(allocation).unwrap())),
        Some((
            address.clone(),
            NonNegI64::try_from(allocation - 1).unwrap(),
        )),
    ] {
        let root = state_root(&runtime, None, Some(leaf(allocation))).await;
        let (dag, store, approved) = genesis_fixture(root, false).await;
        let err = check_running_dev_vesting(dag.as_ref(), &store, &approved, &runtime, claim)
            .await
            .expect_err("a local claim that differs from the restored genesis is refused");
        assert!(err.contains("local developer allocation"), "{err}");
    }

    // A local claim against a genesis that carries no allocation is refused.
    let root = state_root(&runtime, None, None).await;
    let (dag, store, approved) = genesis_fixture(root, false).await;
    let err = check_running_dev_vesting(
        dag.as_ref(),
        &store,
        &approved,
        &runtime,
        matching_claim.clone(),
    )
    .await
    .expect_err("a local claim against an allocation-free genesis is refused");
    assert!(err.contains("has none"), "{err}");

    // A later state whose unreleased amount is still within the allocation is accepted.
    let genesis_root = state_root(&runtime, None, Some(leaf(allocation))).await;
    let later_root = state_root(&runtime, None, Some(leaf(allocation / 2))).await;
    let (dag, store, approved) = later_fixture(genesis_root, later_root).await;
    assert!(
        check_running_dev_vesting(dag.as_ref(), &store, &approved, &runtime, None)
            .await
            .is_ok(),
        "a later state may have released part of the allocation"
    );

    // A later state that claims to have released more than the whole allocation is refused.
    let genesis_root = state_root(&runtime, None, Some(leaf(allocation))).await;
    let later_root = state_root(&runtime, None, Some(leaf(allocation + 1))).await;
    let (dag, store, approved) = later_fixture(genesis_root, later_root).await;
    let err = check_running_dev_vesting(dag.as_ref(), &store, &approved, &runtime, None)
        .await
        .expect_err("an over-released later leaf is inconsistent");
    assert!(err.contains("unreleased"), "{err}");
}
