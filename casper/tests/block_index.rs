//! The merge's **block index**, built against a live runtime (`casper/src/merging.rs`).
//!
//! `BlockIndex::get_block_index` and the `apply` constructors under it read the mergeable-channel
//! sidecar and the block's **pre-state**, so they cannot be pinned from the pure fixtures the file's
//! own test module uses — the classification table in `spec/TEST-COVERAGE.md` called this whole path
//! `harness-bound`, and named the fixture it would need. This is that fixture: the same
//! `common::build_runtime_manager()` the determinism tests use, a block produced by `compute_state`,
//! and the **first** lookup of a block — whose sidecar has never been persisted. That last detail is
//! not incidental: it is exactly the state an LFS-restored or deep-replayed block arrives in, and it
//! is the arm where the index regenerates the sidecar by replaying the block rather than failing.
//!
//! The random seed is derived **from the block** (`BlockRandomSeed`), not chosen by the test, because
//! the regenerate arm replays the block with that same derivation: a test that picked its own seed
//! would produce a block whose replay computes a different state, which is a fixture bug that would
//! look like a merge bug.

mod common;
use common::fringe_state;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use rchain_shared::lock::Unpoison;

/// **The block-index cache is process-global and bounded** (64 entries, `merging.rs`). The tests in
/// this binary share it, so they run one at a time: a parallel test that indexes more blocks than
/// the cache holds evicts this test's entry between its two calls and turns a correct cache hit into
/// a red assertion (CI 2026-11-21, `block_index.rs:422`). Serialising the file's three tests keeps
/// the identity witness deterministic without weakening it.
static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

use rchain_block_storage::block_store::BlockStore;
use rchain_block_storage::dag::codecs::{
    BlockHashCodec, BlockMessageCodec, BlockMetadataCodec, SignedDeployDataCodec,
};
use rchain_casper::block_metadata_store::BlockMetadataStore;
use rchain_casper::block_random_seed::BlockRandomSeed;
use rchain_casper::merging::{BlockIndex, MergeScope};
use rchain_casper::system_deploy::SystemDeploy;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::public_key::PublicKey;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{
    BlockMessage, DeployData, RholangState, SignedDeployData,
};
use rchain_models::validator::Validator;
use rchain_rholang::merging::SidecarRecord;
use rchain_rholang::native_state::PosGenesis;
use rchain_rholang::system_processes::BlockData;
use rchain_shared::refined::NonNegI64;
use rchain_shared::store::{InMemoryKeyValueStore, KeyValueStore};
use rchain_shared::typed_store::{BytesCodec, KeyValueTypedStoreCodec, SharedStore};

/// The storage the node builds, constructed the way `casper/src/dag.rs`'s own tests construct it — the
/// same four stores.
///
/// The block index needs a DAG because the participation a boundary's absence rule reads is *derived*
/// from it rather than carried on the block, and it is used only on the regeneration arm — which is
/// exactly the arm these tests take.
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

use rchain_casper::genesis::contracts::Vault;
use rchain_rholang::util::rev_address::RevAddress;

fn deploy(term: &str) -> SignedDeployData {
    SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: term.to_string(),
            timestamp: 0,
            phlo_price: 1,
            phlo_limit: 500_000,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        },
        deployer: vec![0u8; 65],
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
    }
}

/// The deployer's vault: the deploy above is signed-by-nobody and unfunded, so the block's deploy has
/// to be payable — the same seeded vault `casper/tests/determinism.rs` builds.
fn seeded_vault() -> Vault {
    Vault {
        rev_address: RevAddress::from_public_key(&PublicKey::new(vec![0u8; 65]))
            .expect("valid rev address"),
        initial_balance: NonNegI64::try_from(1_000_000_000).unwrap(),
    }
}

/// A block whose hash and sender are fixed and whose *state* is filled in after the deploy is run —
/// the shell exists first because its seed comes from it.
fn block_shell() -> BlockMessage {
    BlockMessage {
        version: 1,
        shard_id: "root".to_string(),
        block_hash: BlockHash::new([0x11; 32]),
        block_number: 1.try_into().expect("height 1"),
        sender: Validator::new([0u8; 65]),
        seq_num: 1.try_into().expect("seq 1"),
        pre_state_hash: StateHash::new([0u8; 32]),
        post_state_hash: StateHash::new([0u8; 32]),
        // Not empty on purpose: the regenerate arm replays **with cost accounting** iff the block has
        // justifications (`merging.rs`'s `with_cost_accounting = !block.justifications.is_empty()`), and
        // the play this block's state came from charged costs. A block with no justifications is the
        // *other* arm's shape — "an equivalent empty block, nothing to replay" — and replaying it here
        // would compute a state hash that differs from the declared one by exactly the charges.
        justifications: vec![BlockHash::new([0x22; 32])],
        bonds: BTreeMap::new(),
        rejected_deploys: BTreeSet::new(),
        rejected_blocks: BTreeSet::new(),
        rejected_senders: BTreeSet::new(),
        state: RholangState::default(),
        sig_algorithm: "secp256k1".to_string(),
        sig: Vec::new(),
        timestamp: 0,
    }
}

/// **The index's first lookup of a block, whose sidecar does not exist yet.** The load fails with
/// "Mergeable store invalid state hash", and the arm that handles it does not give up: it either
/// treats an empty block as trivially empty or replays the block to regenerate the sidecar, then
/// saves it. Everything under that — the deploy-chain index, its event-log index built from the
/// pre-state, and the block index that carries them — is what this asserts.
///
/// The alternative it rules out is the one that matters operationally: an index that *failed* here
/// would make an LFS-restored block unmergeable, which is the class AUDIT C57's neighbourhood is
/// about.
#[tokio::test]
async fn the_block_index_regenerates_a_missing_sidecar_and_indexes_the_block() {
    let _cache_guard = CACHE_TEST_LOCK.lock().unpoison();
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);

    let (genesis_pre, genesis_post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    // The shell first, so the seed the *block* implies is the seed the deploy is run under.
    let mut block = block_shell();
    block.pre_state_hash = StateHash::new(*genesis_post.as_bytes());
    let block_rand =
        BlockRandomSeed::random_generator_from_block(&block).expect("valid block seed");

    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &genesis_post,
            &[deploy(r#"@"marker"!(1)"#)],
            &[],
            &block_rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "the deploy must succeed or the index has nothing to index: {:?}",
        user_results[0].eval_result.errors
    );

    block.post_state_hash = StateHash::new(*post_state.as_bytes());
    block.state = RholangState {
        deploys: user_results.into_iter().map(|r| r.deploy).collect(),
        system_deploys: sys_results.into_iter().map(|r| r.deploy).collect(),
    };

    let store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
        {
            let shared: SharedStore = Arc::new(tokio::sync::Mutex::new(Box::new(
                InMemoryKeyValueStore::default(),
            )));
            shared
        },
        Arc::new(BlockHashCodec),
        Arc::new(BlockMessageCodec),
    ));
    store
        .put(&[(block.block_hash, block.clone())])
        .await
        .expect("put the block");

    let index = BlockIndex::get_block_index(
        &rm,
        &*build_dag().await,
        &store,
        block.block_hash,
        fringe_state(1),
    )
    .await
    .expect("the block index regenerates the sidecar rather than failing");

    assert_eq!(index.block_hash, block.block_hash);
    assert_eq!(
        index.deploy_chains.len(),
        1,
        "one deploy, one chain: {:?}",
        index.deploy_chains.len()
    );
    let chain = &index.deploy_chains[0];
    assert_eq!(
        chain.host_block,
        Blake2b256Hash::from_byte_array(block.block_hash.as_bytes()),
        "the chain's host block"
    );
    assert_eq!(
        chain.pre_state_hash,
        Blake2b256Hash::from_byte_array(genesis_post.as_bytes()),
        "the chain carries the block's own pre-state"
    );
    assert_eq!(
        chain.post_state_hash,
        Blake2b256Hash::from_byte_array(post_state.as_bytes()),
        "…and its post-state"
    );
    assert_eq!(
        chain.deploys_with_cost.len(),
        1,
        "the deploy is indexed with its cost"
    );
    assert!(
        chain.deploys_with_cost.iter().any(|d| d.cost > 0),
        "and the cost is the one the deploy was charged — which is what the merge's rejection cost is
         computed from, so a zeroed charge would under-report every rejection: {:?}",
        chain.deploys_with_cost
    );
    // **What this fixture does not carry**: the deploy here is unsigned (`sig: Vec::new()`, the same
    // shape `casper/tests/determinism.rs` uses), so its id — derived from the signature — is empty. The
    // *cost* half of `DeployIdWithCost` is fully asserted; the id half is a carrier the fixture leaves
    // empty, and a test that read it could not tell one deploy from another here.

    // The genesis pre-state is not the block's, and the index is not confused about that.
    assert_ne!(
        Blake2b256Hash::from_byte_array(genesis_pre.as_bytes()),
        chain.pre_state_hash
    );
}

/// **A merge of one branch reproduces that branch's post-state — native writes included.**
///
/// The end-to-end falsifier for #74. The block carries a cost-accounted deploy, whose pre-charge and
/// refund move REV (`PREFIX_VAULT`) and whose fee moves the PoS vault (`PREFIX_POS`) — native writes
/// with no tuple-space event to carry them. Merging the branch over the genesis must reconstruct the
/// block's own post-state; if the merge applies only the tuple-space `StateChange`s it reconstructs
/// that state **minus** the native leaves, and the hash differs. Before the fix this test fails with
/// a hash mismatch, which *is* the silent state loss #74 reported.
///
/// It exercises the whole path: the play path's native capture, the sidecar, `get_block_index`'s load
/// of it, and `MergeScope::merge`'s application of it.
#[tokio::test]
async fn a_merge_reproduces_a_branchs_post_state_including_its_native_writes() {
    let _cache_guard = CACHE_TEST_LOCK.lock().unpoison();
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    let (_genesis_pre, genesis_post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    // The shell first, so the block's own seed, sender and seq_num drive both the play and the
    // sidecar key the index will look it up under.
    let mut block = block_shell();
    block.pre_state_hash = StateHash::new(*genesis_post.as_bytes());
    let block_rand =
        BlockRandomSeed::random_generator_from_block(&block).expect("valid block seed");
    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &genesis_post,
            &[deploy(r#"@"marker"!(1)"#)],
            &[],
            &block_rand,
            BlockData::from_block(&block),
            &fringe_state(1),
        )
        .await
        .expect("compute_state");
    block.post_state_hash = StateHash::new(*post_state.as_bytes());
    block.state = RholangState {
        deploys: user_results.into_iter().map(|r| r.deploy).collect(),
        system_deploys: sys_results.into_iter().map(|r| r.deploy).collect(),
    };

    // The play path recorded a native sidecar for this block, under the key the index looks up.
    //
    // **It is legitimately empty, and since AUDIT C207 it must be.** This block writes no native leaf
    // of its own — the deploy's only native effect is its own cost accounting, which moves REV and the
    // staking vault — and cost accounting is exactly what the merge re-derives from the accepted
    // deploys rather than carrying. So "the sidecar is empty" and "the capture is broken" look alike
    // here, which is why the assertion that carries the weight is the hash comparison below: it holds
    // whichever way the split falls, and it is the one that fails if either half is wrong. (The
    // non-empty half is pinned by `deploy_native_write.rs`, whose block writes a leaf of its own.)
    let recorded = rm
        .load_native_changes(
            post_state.as_bytes(),
            block.sender.as_bytes(),
            i64::from(block.seq_num),
        )
        .await
        .expect("a readable native sidecar")
        .expect("a played block records a sidecar");
    // **An empty *attributed* record, and the difference is the point** (#280). A record that is
    // empty because it attributes nothing is not the same value as one written before attribution
    // existed — the second would be a block-level set, which is what the loader refuses. Asserting
    // only "empty" would pass for either.
    match recorded {
        SidecarRecord::Attributed(effects) => assert!(
            effects.is_empty(),
            "a block whose only native effect is cost accounting saves an empty sidecar: the merge \
             re-derives that effect from the accepted deploys (AUDIT C207), and carrying it here is \
             what used to make every user-deploy block overlap every concurrent sibling on `pos:vault`"
        ),
        other => panic!("a played block's sidecar must be attributed; got {other:?}"),
    }

    let store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
        {
            let shared: SharedStore = Arc::new(tokio::sync::Mutex::new(Box::new(
                InMemoryKeyValueStore::default(),
            )));
            shared
        },
        Arc::new(BlockHashCodec),
        Arc::new(BlockMessageCodec),
    ));
    store
        .put(&[(block.block_hash, block.clone())])
        .await
        .expect("put the block");

    let index = BlockIndex::get_block_index(
        &rm,
        &*build_dag().await,
        &store,
        block.block_hash,
        fringe_state(1),
    )
    .await
    .expect("the block index");
    // **On its chain, and empty** (#280). The index no longer has a block-level native set at all:
    // every effect is placed on the chain that carries its deploy, so "the block's own native writes"
    // is now read as the union of its chains' — which for this block is empty for the same reason the
    // sidecar above is (AUDIT C207).
    let indexed: Vec<&rchain_rspace::native_store::NativeStoreAction> = index
        .deploy_chains
        .iter()
        .flat_map(|c| c.native_effects.iter())
        .collect();
    assert!(
        indexed.is_empty(),
        "the index carries the block's *own* native writes and no others, so for this block it is \
         empty — the same first half as the sidecar above, and the same reason (AUDIT C207)"
    );

    // A second request for the same block is a cache hit, and a hit must **share** the entry rather
    // than deep-copy it (#117). A `BlockIndex` carries a `Vec<DeployChainIndex>`, each with its own
    // `EventLogIndex`, and the merge asks for every block of its conflict and final scopes — 711 of
    // 750 requests were hits across one measured devnet stall, which made this copy the workload's
    // dominant allocating site in a heap profile.
    //
    // Asserted on *identity*, not on bytes: this crate graph has no allocation counter, because
    // `#![forbid(unsafe_code)]` rules out a `#[global_allocator]` (see `casper/src/dag.rs`). That
    // makes `Arc::ptr_eq` the available falsifier — restore the old `idx.clone()` on the hit path and
    // this goes red, while still passing every behavioural test in the file.
    let again = BlockIndex::get_block_index(
        &rm,
        &*build_dag().await,
        &store,
        block.block_hash,
        fringe_state(1),
    )
    .await
    .expect("the block index again");
    assert!(
        Arc::ptr_eq(&index, &again),
        "a cache hit must share the cached index, not deep-copy it (#117)"
    );

    // Merge the single branch over the genesis: nothing has finalised, so the branch is the whole
    // conflict scope and the base is the genesis.
    let scope = MergeScope {
        fork_blocks: BTreeSet::new(),
        final_scope: BTreeSet::new(),
        conflict_scope: BTreeSet::from([block.block_hash]),
        ancestry: BTreeMap::new(),
    };
    let block_index = {
        let index = index.clone();
        move |h: BlockHash| {
            let index = index.clone();
            async move {
                if h == index.block_hash {
                    Ok(index)
                } else {
                    Err(format!("no index for {h:?}"))
                }
            }
        }
    };
    let merged = MergeScope::merge(
        &scope,
        Blake2b256Hash::from_byte_array(genesis_post.as_bytes()),
        &rchain_casper::merging::RejectionsMap::new(),
        rm.get_history_repo(),
        &block_index,
        |_| 0,
    )
    .await
    .expect("the merge")
    .state;

    assert_eq!(
        merged,
        Blake2b256Hash::from_byte_array(post_state.as_bytes()),
        "the merge must reproduce the branch's post-state, its native writes included: a match is the \
         fix for #74, and a mismatch is the state loss it reported"
    );
}

/// **AUDIT C201, the half the live arm could not reach: is a slashed validator gone from the bonds a
/// *merged* pre-state reports?**
///
/// The A2 live arm (`spec/audit/evidence/a2-live-equivocation-run.sh`, #150) turned this up: the
/// proposer emitted a `Slash` for the offender on **every** block after the first, 59 of them and
/// still counting, while every one of those blocks carried a bonds map with the offender already gone.
/// The fold that decides who is slashable filters on `bonded`, which is
/// `compute_bonds(pre_state_hash)` — `pos:active` read at the proposer's **merged pre-state**, the one
/// `MergeScope::merge` reconstructs from the native-changes sidecar (#74).
///
/// The measured half is clean: `a_slashed_validator_is_absent_from_the_bonds_at_the_post_state`
/// (in `runtime_manager.rs`) shows the leaf is written and read correctly at a **post-state**. What
/// that cannot see is the merge — so this is the fixture the row asked for, and it splits the question
/// the same way: **build two siblings off one genesis, one of which slashes the victim, merge them, and
/// ask the merged root who is bonded.**
///
/// A pass here means the stale read is the proposer's *hash choice* and not the merge. **A failure is
/// the merge's native reconstruction, and this row closes into a fix there (#74) rather than here** —
/// which is why the assertion names both possibilities rather than only the expected one.
/// **C201's owed fixture, and it passes — which is the answer the row asked for.** The row said a pass
/// would mean "the stale read is in the proposer's hash choice and not in the merge", and that is what
/// it says.
///
/// **What it took to get there is the useful part, because the first two shapes were wrong and their
/// failures looked like the defect.** The merge refuses two accepted writers of one native key that have
/// not seen each other, and every block's cost accounting writes the **staking vault** — so a bare pair
/// of siblings both write it, one chain is rejected, and `reject_whole_blocks` takes that block's
/// **whole** native write set with it, the slash included. That is what the first shapes measured: not a
/// lost write, but a conflict the fixture had given the merge no way to order. **Tell it that the
/// slashing branch has seen the other and the victim is gone**, which is the shape a merge over a chain
/// with an order has.
///
/// **AUDIT C207 removed the need for that workaround, and the fixture now runs as the bare pair.**
/// Cost accounting is composed per accepted deploy instead of travelling in the block's sidecar, so the
/// two siblings conflict on nothing and the merge reconciles them the way it reconciles any two
/// branches. The `ancestry` map is empty on purpose and the test is a falsifier for the fix: restore the
/// old shape — cost accounting back in the sidecar — and the pair conflicts again, one block is
/// rejected whole, and the slash is gone from the merged root.
///
/// The single-branch control is kept because it is what made the two-branch failure legible: without it,
/// a failure says only "something about this merge is wrong" rather than "the merge can fold this branch
/// and something *else* is dropping it".
#[tokio::test]
async fn a_slashed_validator_is_absent_from_the_bonds_at_a_merged_root() {
    let _cache_guard = CACHE_TEST_LOCK.lock().unpoison();
    use rchain_models::block_metadata::SlashSeverity;
    use rchain_rholang::native_state::PosParams;
    use rchain_shared::refined::SeqNum;

    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    let operator = Validator::new([1u8; 65]);
    let victim = Validator::new([2u8; 65]);
    let pos = PosGenesis {
        bonds: BTreeMap::from([
            (operator, NonNegI64::try_from(100).expect("a stake")),
            (victim, NonNegI64::try_from(100).expect("a stake")),
        ]),
        trusted: BTreeSet::new(),
        params: PosParams::default(),
        dev_vesting: None,
        developer_reserve: None,
    };
    let (_pre, genesis_post, _) = rm
        .compute_genesis(&[], &rand, BlockData::empty(), &pos, &[seeded_vault()])
        .await
        .expect("compute_genesis");
    let bonded_before = rm
        .compute_bonds(&StateHash::from_slice(genesis_post.as_bytes()))
        .await
        .expect("bonds at the genesis post-state");
    assert!(
        bonded_before.contains_key(&victim),
        "the control: the victim is bonded before anything is slashed"
    );

    // Two siblings off the same genesis. `a` slashes the victim; `b` is the benign branch that makes
    // the merge a merge rather than a single-branch replay — without it there is nothing for the
    // conflict scope's native writes to be reconciled against.
    let mut a = block_shell();
    a.block_hash = BlockHash::new([0x31; 32]);
    a.sender = operator;
    a.seq_num = SeqNum::try_from(1).expect("seq 1");
    a.pre_state_hash = StateHash::new(*genesis_post.as_bytes());
    let slash = [SystemDeploy::slash(
        &victim,
        SlashSeverity::Malicious,
        None,
        rand.split_byte(0),
    )];
    let a_rand = BlockRandomSeed::random_generator_from_block(&a).expect("valid block seed");
    let (post_a, a_user, a_sys) = rm
        .compute_state(
            &genesis_post,
            &[],
            &slash,
            &a_rand,
            BlockData::from_block(&a),
            &fringe_state(1),
        )
        .await
        .expect("the slashing sibling");
    a.post_state_hash = StateHash::new(*post_a.as_bytes());
    a.state = RholangState {
        deploys: a_user.into_iter().map(|r| r.deploy).collect(),
        system_deploys: a_sys.into_iter().map(|r| r.deploy).collect(),
    };

    let mut b = block_shell();
    b.block_hash = BlockHash::new([0x32; 32]);
    b.sender = victim;
    b.seq_num = SeqNum::try_from(1).expect("seq 1");
    b.pre_state_hash = StateHash::new(*genesis_post.as_bytes());
    let b_rand = BlockRandomSeed::random_generator_from_block(&b).expect("valid block seed");
    let (post_b, b_user, b_sys) = rm
        .compute_state(
            &genesis_post,
            &[deploy(r#"@"other"!(1)"#)],
            &[],
            &b_rand,
            BlockData::from_block(&b),
            &fringe_state(1),
        )
        .await
        .expect("the benign sibling");
    b.post_state_hash = StateHash::new(*post_b.as_bytes());
    b.state = RholangState {
        deploys: b_user.into_iter().map(|r| r.deploy).collect(),
        system_deploys: b_sys.into_iter().map(|r| r.deploy).collect(),
    };

    // Both blocks need an index before the merge can fold their native writes — that is what the index
    // table is for, and rebuilding it is the same call each time.
    let store: BlockStore = Arc::new(KeyValueTypedStoreCodec::new(
        {
            let shared: SharedStore = Arc::new(tokio::sync::Mutex::new(Box::new(
                InMemoryKeyValueStore::default(),
            )));
            shared
        },
        Arc::new(BlockHashCodec),
        Arc::new(BlockMessageCodec),
    ));
    store
        .put(&[(a.block_hash, a.clone()), (b.block_hash, b.clone())])
        .await
        .expect("put both siblings");
    let dag = build_dag().await;
    let index_a = BlockIndex::get_block_index(&rm, &*dag, &store, a.block_hash, fringe_state(1))
        .await
        .expect("the slashing sibling's index");
    let index_b = BlockIndex::get_block_index(&rm, &*dag, &store, b.block_hash, fringe_state(1))
        .await
        .expect("the benign sibling's index");
    let block_index = move |h: BlockHash| {
        let (ia, ib) = (index_a.clone(), index_b.clone());
        async move {
            if h == ia.block_hash {
                Ok(ia)
            } else if h == ib.block_hash {
                Ok(ib)
            } else {
                Err(format!("no index for {h:?}"))
            }
        }
    };

    // **The isolating control, and it is what makes a failure below legible.** Merge the slashing
    // branch *alone* first. If the victim is gone here, the merge can fold this branch's native write
    // and anything the two-branch merge does differently is a *reconciliation* question; if the victim
    // is still here, the branch's own index is what the merge cannot read and the two-branch assertion
    // below would be measuring that instead.
    let alone = MergeScope {
        fork_blocks: BTreeSet::new(),
        final_scope: BTreeSet::new(),
        conflict_scope: BTreeSet::from([a.block_hash]),
        ancestry: BTreeMap::new(),
    };
    let merged_alone = MergeScope::merge(
        &alone,
        Blake2b256Hash::from_byte_array(genesis_post.as_bytes()),
        &rchain_casper::merging::RejectionsMap::new(),
        rm.get_history_repo(),
        &block_index,
        |_| 0,
    )
    .await
    .expect("the merge of the slashing branch alone")
    .state;
    let alone_bonds = rm
        .compute_bonds(&StateHash::from_slice(merged_alone.as_bytes()))
        .await
        .expect("bonds at the single-branch root");
    assert!(
        !alone_bonds.contains_key(&victim),
        "the slashing branch *alone* must carry its write to the merged root — if it does not, the \
         merge cannot read this branch's index and the two-branch arm below is measuring the wrong thing"
    );

    // **And a bare pair of siblings, which is what this fixture could not be before AUDIT C207.**
    //
    // `fold` applies a key's writers ancestors-first and *refuses* two accepted writers that have not
    // seen each other. Every block's cost accounting writes the staking vault, so while that write
    // travelled in the sidecar two unreconciled siblings both wrote it: one chain was rejected, and a
    // rejected chain takes its block's **whole** native write with it, the slash included. The fixture
    // was therefore shaped around that — it had to *tell* the merge that `a` had seen `b`, which is
    // what a merge over a chain with an order has and a bare pair does not.
    //
    // Since the fix, cost accounting is composed per accepted deploy rather than carried, so the
    // sibling pair conflicts on nothing: `a`'s own write is the slash's and `b` has none, and `b`'s
    // charge composes onto `a`'s absolute staking-vault value because neither has seen the other.
    // The `ancestry` map is empty here on purpose — this is now the bare-pair case, and it fails if
    // the merge ever goes back to treating two concurrent charges as competing snapshots.
    let scope = MergeScope {
        fork_blocks: BTreeSet::new(),
        final_scope: BTreeSet::new(),
        conflict_scope: BTreeSet::from([a.block_hash, b.block_hash]),
        ancestry: BTreeMap::new(),
    };
    let merged = MergeScope::merge(
        &scope,
        Blake2b256Hash::from_byte_array(genesis_post.as_bytes()),
        &rchain_casper::merging::RejectionsMap::new(),
        rm.get_history_repo(),
        &block_index,
        |_| 0,
    )
    .await
    .expect("the merge of two siblings")
    .state;

    let bonded_after = rm
        .compute_bonds(&StateHash::from_slice(merged.as_bytes()))
        .await
        .expect("bonds at the merged root");
    assert!(
        bonded_after.contains_key(&operator),
        "the control: the merging validator is untouched by the other branch's slash"
    );
    assert!(
        !bonded_after.contains_key(&victim),
        "a slashed validator must be gone from `pos:active` at a **merged** root — if it is still here \
         the merge's native reconstruction (#74) is the defect, which is what C201's row says this \
         fixture would decide; if it is gone, the live arm's repeated `Slash` is the proposer's hash \
         choice and not the merge"
    );
}
