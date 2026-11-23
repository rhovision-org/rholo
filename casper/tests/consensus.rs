//! End-to-end consensus-pipeline integration tests (genesis → block → replay).

mod common;

use std::collections::{BTreeMap, BTreeSet};

use rchain_casper::block_status::BlockStatus;
use rchain_casper::genesis::contracts::Vault;
use rchain_casper::runtime_manager::RuntimeManager;
use rchain_casper::system_deploy::SystemDeploy;
use rchain_casper::validate::bonds_cache;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::public_key::PublicKey;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{
    BlockMessage, DeployData, ProcessedDeploy, ProcessedSystemDeploy, RholangState,
    SignedDeployData,
};
use rchain_models::validator::Validator;
use rchain_rholang::native_state::{
    previous_seed_anchor, NativeSystemState, PosGenesis, PosParams,
};
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::refined::NonNegI64;

use common::{build_runtime_manager, fringe_state};

fn fixed_rand() -> Blake2b512Random {
    Blake2b512Random::from_init(&[0u8; 32])
}

/// The per-block data for a synthetic block at `height` — which must be the same number the block's
/// close deploy carries.
///
/// The two are not decoration: `close_block` writes the *next* epoch's active-set seed labelled with
/// the epoch index derived from the block number, so a close deploy numbered 1 played against
/// `BlockData::empty()` (height 0) writes a different seed than the replayer derives, and the two
/// post-state hashes diverge. That is what this test did until the seed writer landed and turned the
/// disagreement into a visible mismatch. **A real block cannot be in that state** — the proposer and
/// the replayer both read the number from the block — which is why the fix is to make the fixture
/// consistent rather than to weaken the rule.
fn block_data(height: i64) -> BlockData {
    BlockData {
        block_number: rchain_shared::refined::BlockHeight::try_from(height).expect("height"),
        ..BlockData::empty()
    }
}

/// A minimal signed deploy with the given term (signature verification is deferred to the
/// deploy-acceptance path, so the sig/deployer fields are left empty here).
fn deploy(term: &str) -> SignedDeployData {
    deploy_with_limit(term, 90_000)
}

fn deploy_with_limit(term: &str, limit: i64) -> SignedDeployData {
    SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: term.to_string(),
            timestamp: 0,
            phlo_price: 1,
            phlo_limit: limit,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        },
        deployer: vec![0u8; 32],
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
    }
}

#[tokio::test]
async fn genesis_deploy_replay_recomputes_state() {
    let rm = build_runtime_manager().await;
    let rand = fixed_rand();
    let (pre, post, results) = rm
        .compute_genesis(
            &[deploy(r#"@"chan"!(42)"#)],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("compute_genesis");
    assert_eq!(results.len(), 1);
    assert!(results[0].eval_result.succeeded(), "deploy should succeed");

    // Law 11: replay recomputes the same post-state hash from the recorded log.
    let processed: Vec<ProcessedDeploy> = results.iter().map(|r| r.deploy.clone()).collect();
    let (replay_post, _) = rm
        .replay_compute_state(
            &pre,
            &processed,
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            false,
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("replay_compute_state");
    assert_eq!(
        post, replay_post,
        "replay must reproduce the play post-state"
    );
}

#[tokio::test]
async fn empty_state_hash_fixed_matches_runtime() {
    let rm = build_runtime_manager().await;
    let hash = rm
        .runtime()
        .empty_state_hash()
        .await
        .expect("empty state hash");
    assert_eq!(
        hash,
        rchain_casper::interpreter_util::empty_state_hash_fixed(),
        "the hard-coded genesis pre-state hash must match the computed empty state"
    );
}

#[tokio::test]
async fn deploy_exceeding_phlo_limit_fails_and_next_runs() {
    let rm = build_runtime_manager().await;
    let rand = fixed_rand();
    let starving = deploy_with_limit(r#"@"chan"!(42)"#, 1);
    let normal = deploy(r#"@"chan2"!(43)"#);

    let (_, _, results) = rm
        .compute_genesis(
            &[starving, normal],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("compute_genesis");

    assert!(
        results[0].deploy.is_failed,
        "phlo-exhausted deploy must be failed"
    );
    assert!(
        results[0].eval_result.errors.iter().any(|e| matches!(
            e,
            rchain_rholang::errors::RholangError::OutOfPhlogistonsError
        )),
        "failure must be an OutOfPhlogistonsError"
    );

    // The next deploy still runs: the per-deploy phlo `set` resets the balance.
    assert!(
        !results[1].deploy.is_failed,
        "subsequent deploy must succeed"
    );
}

#[tokio::test]
async fn replay_matches_play_for_persistent_and_peek() {
    let rm = build_runtime_manager().await;
    let rand = fixed_rand();
    // A non-trivial deploy: persistent send + peek receive (Law 11 replay must reproduce the play
    // post-state, not just a single trivial send).
    let term = r#"new c in { c!!(42) | for (@x <<- c) { @"out"!(x) } }"#;
    let (pre, post, results) = rm
        .compute_genesis(
            &[deploy(term)],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("compute_genesis");
    assert!(results[0].eval_result.succeeded(), "deploy should succeed");

    let processed: Vec<ProcessedDeploy> = results.iter().map(|r| r.deploy.clone()).collect();
    let (replay_post, _) = rm
        .replay_compute_state(
            &pre,
            &processed,
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            false,
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("replay_compute_state");
    assert_eq!(
        post, replay_post,
        "replay must reproduce the play post-state"
    );
}

/// A signed deploy with an explicit 65-byte deployer key (the bond path derives the validator from
/// the deployer id).
fn deploy_with_key(term: &str, deployer: Vec<u8>) -> SignedDeployData {
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
        deployer,
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
    }
}

/// The dynamic-validator lifecycle end to end: a trusted observer bonds, its stake enters the pool,
/// it becomes an active validator, and replay reproduces the same post-state.
#[tokio::test]
async fn bond_deploy_updates_the_active_validator_set() {
    let rm = build_runtime_manager().await;
    let rand = fixed_rand();
    let deployer = Validator::new([0u8; 65]);
    let rev_address =
        RevAddress::from_public_key(&PublicKey::new(vec![0u8; 65])).expect("valid rev address");
    let pos_genesis = PosGenesis {
        bonds: std::collections::BTreeMap::new(),
        trusted: BTreeSet::from([deployer]),
        params: PosParams {
            minimum_bond: rchain_shared::refined::NonNegI64::try_from(1).unwrap(),
            ..PosParams::default()
        },
        dev_vesting: None,
        developer_reserve: None,
    };
    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &pos_genesis,
            &[Vault {
                rev_address,
                initial_balance: NonNegI64::try_from(1_000_000_000).unwrap(),
            }],
        )
        .await
        .expect("compute_genesis");

    let term = r#"new pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {
  pos!("bond", [*deployerId, 30, *ret]) |
  for (_ <- ret) { Nil }
}"#;
    // The block's closing system deploy, which `block_creator` appends to every block. It is what
    // makes the bond *active*: the contract's `bond` only joins the pool (`Pos.rhox:355`), and the
    // active set is recomputed inside `closeBlock` (`:546`) — an epoch boundary, which with these
    // permissive parameters every block is. Without it the deploy would pool the stake and leave the
    // validator out of the consensus set.
    let close = SystemDeploy::close_block(
        1,
        fringe_state(1),
        BTreeMap::new(),
        fixed_rand().split_byte(2),
    );
    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &post,
            &[deploy_with_key(term, vec![0u8; 65])],
            &[close],
            &rand,
            block_data(1),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "bond deploy must succeed: {:?}",
        user_results[0].eval_result.errors
    );

    let post_state_hash = StateHash::from_slice(post_state.as_bytes());
    assert!(
        rm.compute_bonds(&post_state_hash)
            .await
            .unwrap()
            .contains_key(&deployer),
        "the bonded observer is now an active validator"
    );

    let processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
    let processed_sys: Vec<ProcessedSystemDeploy> =
        sys_results.into_iter().map(|r| r.deploy).collect();
    let (replay_state, _) = rm
        .replay_compute_state(
            &post,
            &processed,
            &processed_sys,
            &rand,
            block_data(1),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("replay compute_state");
    assert_eq!(
        post_state, replay_state,
        "replay must reproduce the bond post-state"
    );
}

/// The admission flow as a network performs it: a trustee admits a key in one block, and that key bonds in
/// a later block. This is the flow the testnet could not complete — `trust` reported success and the next
/// block's `bond` answered "Validator is not trusted" — and the single-block test above cannot see it,
/// because there the trustee is both the deployer and the genesis bond.
#[tokio::test]
async fn a_trustee_admits_an_observer_and_it_bonds_in_the_next_block() {
    let rm = build_runtime_manager().await;
    let rand = fixed_rand();
    let trustee = Validator::new([1u8; 65]);
    let newcomer = Validator::new([2u8; 65]);
    let trustee_address =
        RevAddress::from_public_key(&PublicKey::new(vec![1u8; 65])).expect("trustee rev address");
    let newcomer_address =
        RevAddress::from_public_key(&PublicKey::new(vec![2u8; 65])).expect("newcomer rev address");
    let vaults = vec![
        Vault {
            rev_address: trustee_address,
            initial_balance: NonNegI64::try_from(1_000_000).unwrap(),
        },
        Vault {
            rev_address: newcomer_address,
            initial_balance: NonNegI64::try_from(1_000_000).unwrap(),
        },
    ];
    let pos_genesis = PosGenesis {
        bonds: [(trustee, NonNegI64::try_from(1000).unwrap())]
            .into_iter()
            .collect(),
        trusted: BTreeSet::from([trustee]),
        params: PosParams {
            minimum_bond: rchain_shared::refined::NonNegI64::try_from(1).unwrap(),
            ..PosParams::default()
        },
        dev_vesting: None,
        developer_reserve: None,
    };
    let (_pre, genesis_post, _) = rm
        .compute_genesis(&[], &rand, BlockData::empty(), &pos_genesis, &vaults)
        .await
        .expect("compute_genesis");

    // Block 1: the trustee admits the newcomer. Nothing about the newcomer is in the genesis.
    let newcomer_hex = rchain_shared::base16::encode(newcomer.as_bytes());
    let trust_term = format!(
        "new pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {{\n  \
         pos!(\"trust\", [*deployerId, \"{newcomer_hex}\".hexToBytes(), *ret]) | for (_ <- ret) {{ Nil }}\n}}"
    );
    let (state1, user1, _) = rm
        .compute_state(
            &genesis_post,
            &[deploy_with_key(&trust_term, vec![1u8; 65])],
            &[SystemDeploy::close_block(
                1,
                fringe_state(1),
                BTreeMap::new(),
                fixed_rand().split_byte(2),
            )],
            &rand,
            block_data(1),
            &fringe_state(1),
        )
        .await
        .expect("play block 1");
    assert!(
        user1[0].eval_result.succeeded(),
        "the trust deploy must succeed: {:?}",
        user1[0].eval_result.errors
    );

    // Block 2: the newcomer bonds, in a later block with a pre-state that should carry the trust.
    let bond_term = "new pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {\n  \
                     pos!(\"bond\", [*deployerId, 100, *ret]) | for (_ <- ret) { Nil }\n}";
    let (state2, user2, _) = rm
        .compute_state(
            &state1,
            &[deploy_with_key(bond_term, vec![2u8; 65])],
            &[SystemDeploy::close_block(
                2,
                fringe_state(1),
                BTreeMap::new(),
                fixed_rand().split_byte(3),
            )],
            &rand,
            block_data(2),
            &fringe_state(1),
        )
        .await
        .expect("play block 2");
    assert!(
        user2[0].eval_result.succeeded(),
        "the bond deploy must succeed: {:?}",
        user2[0].eval_result.errors
    );
    assert!(
        rm.compute_bonds(&StateHash::from_slice(state2.as_bytes()))
            .await
            .unwrap()
            .contains_key(&newcomer),
        "the admitted newcomer is now an active validator"
    );
}

/// **Law 16d — the bond cache a block carries is the PoS state, and it is the *active* set it is.**
///
/// Two arms, because the row's statement is an equality and an equality can fail in two places.
///
/// **The first arm is which leaf the state is read from.** `RuntimeManager::compute_bonds` must read
/// `pos:active` — the consensus set — and not `pos:bonds`, the full pool. The two differ exactly when
/// `number_of_active_validators` caps the pool, so the fixture caps it and the maps have different
/// sizes: a `compute_bonds` reading the pool returns three entries where the active set has two.
/// This is the mutation the row's own note describes, and nothing pinned it before this test — the
/// existing bond tests here only assert `contains_key`, which a pool reading satisfies too.
///
/// **The second arm is the equality itself**, which the validator side runs in `validate::bonds_cache`:
/// the carried map must equal the one the state produces, **and a map that differs must be refused**.
/// The refusal half is what makes this able to fail — an assertion that only says "the agreeing case is
/// accepted" is satisfied by a comparison that accepts everything, which is the defect this sweep found
/// in a dozen other rows.
#[tokio::test]
async fn the_bond_cache_is_the_active_pos_state_and_a_differing_one_is_refused() {
    let rm = build_runtime_manager().await;
    let rand = fixed_rand();
    let (v1, v2, v3) = (
        Validator::new([1u8; 65]),
        Validator::new([2u8; 65]),
        Validator::new([3u8; 65]),
    );
    let pos_genesis = PosGenesis {
        bonds: [(v1, 10i64), (v2, 20), (v3, 30)]
            .into_iter()
            .map(|(v, stake)| (v, NonNegI64::try_from(stake).expect("positive")))
            .collect(),
        trusted: BTreeSet::new(),
        params: PosParams {
            minimum_bond: rchain_shared::refined::NonNegI64::try_from(1).unwrap(),
            number_of_active_validators: 2,
            ..PosParams::default()
        },
        dev_vesting: None,
        developer_reserve: None,
    };
    let (_pre, post, _) = rm
        .compute_genesis(&[], &rand, BlockData::empty(), &pos_genesis, &[])
        .await
        .expect("compute_genesis");
    let post_state_hash = StateHash::from_slice(post.as_bytes());

    // Arm 1 — the state side: the active set, and not the pool.
    let cache = rm
        .compute_bonds(&post_state_hash)
        .await
        .expect("compute_bonds at the post state");
    assert_eq!(
        cache,
        pos_genesis.active_bonds(),
        "the bonds a block carries are the state's *active* set"
    );
    assert_eq!(cache.len(), 2, "the cap selects two of the three bonded");
    assert_ne!(
        cache, pos_genesis.bonds,
        "…which is a different map from the pool, and that difference is the whole distinction"
    );

    // Arm 2 — the cache side: an agreeing map is accepted, a differing one is refused.
    let mut block = BlockMessage {
        version: 1,
        shard_id: "root".to_string(),
        block_hash: BlockHash::new([0xab; 32]),
        block_number: 10.try_into().unwrap(),
        sender: Validator::new([0x11; 65]),
        seq_num: 0.try_into().unwrap(),
        pre_state_hash: post_state_hash.clone(),
        post_state_hash,
        justifications: vec![],
        bonds: cache,
        rejected_deploys: BTreeSet::new(),
        rejected_blocks: BTreeSet::new(),
        rejected_senders: BTreeSet::new(),
        state: RholangState {
            deploys: vec![],
            system_deploys: vec![],
        },
        sig_algorithm: "secp256k1".to_string(),
        sig: vec![1],
        timestamp: 0,
    };
    assert!(
        matches!(
            bonds_cache(&rm, &block).await.expect("bonds_cache runs"),
            Ok(())
        ),
        "a block whose bond cache is the state's active set is accepted"
    );

    block.bonds.insert(
        Validator::new([9u8; 65]),
        NonNegI64::try_from(1).expect("positive"),
    );
    assert!(
        matches!(
            bonds_cache(&rm, &block).await.expect("bonds_cache runs"),
            Err(BlockStatus::InvalidBondsCache)
        ),
        "a bond cache that differs from the state is refused rather than accepted"
    );
}

/// One block played on one node: the state it reaches, the active set the state carries, and the
/// processed deploys a replayer would need.
struct Played {
    post: Blake2b256Hash,
    bonds: BTreeMap<Validator, NonNegI64>,
    user: Vec<ProcessedDeploy>,
    sys: Vec<ProcessedSystemDeploy>,
}

/// Play one block at `height` on `rm` from `pre`: one user deploy, and the close deploy
/// `block_creator` appends to every block.
///
/// The close deploy carries `fringe`, the state hash of the last finalised fringe as of this block —
/// the entropy the seed writer folds into the *next* epoch's seed. Play and replay must be given the
/// same value, which is what the parameter is for here and what every caller deriving it from the DAG
/// gives them in production: nothing about it is published on the block.
async fn play_block(
    rm: &RuntimeManager,
    pre: Blake2b256Hash,
    height: i64,
    term: &str,
    rand: &Blake2b512Random,
    fringe: &Blake2b256Hash,
) -> Played {
    let close = SystemDeploy::close_block(
        height,
        *fringe,
        BTreeMap::new(),
        rand.split_byte(u8::try_from(height).expect("a test height fits a byte")),
    );
    let (post, user, sys) = rm
        .compute_state(
            &pre,
            &[deploy_with_key(term, vec![0u8; 65])],
            &[close],
            rand,
            block_data(height),
            fringe,
        )
        .await
        .expect("play the block");
    assert!(
        user[0].eval_result.succeeded(),
        "the block's deploy must succeed, or it writes nothing and the caller's state comparisons \
         are between two identical states: {:?}",
        user[0].eval_result.errors
    );
    let bonds = rm
        .compute_bonds(&StateHash::from_slice(post.as_bytes()))
        .await
        .expect("the block's own post-state carries a bonds map");
    Played {
        post,
        bonds,
        user: user.into_iter().map(|r| r.deploy).collect(),
        sys: sys.into_iter().map(|r| r.deploy).collect(),
    }
}

/// **The draw, end to end: it is the state's, not the block's — and where the two rules differ.**
///
/// Two nodes with the same genesis play the same chain and then diverge at block 3 (one deploy
/// differs). Block 4 is the next epoch boundary, so it is where the two selection rules predict
/// different things:
///
/// - **This rule.** The seed block 4 draws with is the `pos:epoch_seed` leaf written at block 2, from
///   block 2's *pre-state* — so it is the same on both nodes, and the drawn set must be identical even
///   though their pre-states differ.
/// - **The rule this replaced.** The seed was `hash(shard_id, block_number, sender, pre_state_hash)`
///   computed at the moment of use, so block 4's seed was a function of block 4's *own* pre-state —
///   which differs between the nodes. The set would then differ in five of six draws.
///
/// **Six independent trials, because one boundary is one draw.** A single trial would leave the old
/// rule a 1-in-6 chance of coincidence; over six, a rule that reads the drawing block's pre-state
/// survives with probability 6⁻⁶. The trials are the test's power, not padding — and the falsification
/// is measured, not asserted: with `close_block` reading its own `pre_state_hash` as the seed (the rule
/// this replaces, and nothing else changed), this test fails on **trial 0** at the trial-0 assertion
/// below.
///
/// **Why the deploy inside the boundary block is also varied, and what that does not prove.** Block 4's
/// deploy differs too, so the two post-states differ and the assertion cannot be satisfied by two
/// identical states. That half is *not* discriminating, and the first version of this test learned it
/// the hard way: it varied **only** the boundary block's own deploy and asserted the set held — and it
/// passed unchanged under the removed rule, because a proposer's deploy does not move the block's
/// pre-state and the old rule read only the pre-state. A test that passes under the defect it names is
/// not evidence, so the discriminating perturbation is the *earlier* block.
///
/// What else this pins that no unit test can:
///
/// - **The draw is a function of the chain, not of the process.** A seed carrying any local state — a
///   clock, a thread id, a fresh random — would give the two nodes different post-states at height 1.
/// - **The set is drawn through the whole pipeline**: `compute_state` → `close_block` → the checkpoint
///   → the `pos:active` leaf → `compute_bonds`, read back at the block's own post-state by the same
///   call `validate::bonds_cache` uses on the receiving side.
/// - **The seed leaf survives the checkpoint**, which is the realistic way to get this wrong — and that
///   is the replay arm at the end, not a separate test.
#[tokio::test]
async fn the_drawn_active_set_is_the_states_and_not_the_boundary_blocks() {
    let rand = fixed_rand();
    let pool: Vec<Validator> = (1u8..=4).map(|i| Validator::new([i; 65])).collect();
    // Four bonded, two active, a boundary every second block, and a funded deployer — see below for why
    // the funding is load-bearing.
    let pos_genesis = PosGenesis {
        bonds: pool
            .iter()
            .map(|v| (*v, NonNegI64::try_from(10).expect("positive")))
            .collect(),
        trusted: BTreeSet::new(),
        params: PosParams {
            number_of_active_validators: 2,
            epoch_length: 2,
            ..PosParams::default()
        },
        dev_vesting: None,
        developer_reserve: None,
    };
    // The deployer must be funded: an unfunded deployer's pre-charge fails, a failed deploy writes
    // nothing to the runtime state, and every "these two states differ" assertion below would then be
    // false — a test that cannot fail. (Measured: that is what the first version did.)
    let deployer_address =
        RevAddress::from_public_key(&PublicKey::new(vec![0u8; 65])).expect("valid rev address");
    let vaults = [Vault {
        rev_address: deployer_address,
        initial_balance: NonNegI64::try_from(1_000_000_000).expect("positive"),
    }];

    for trial in 0..6 {
        let (a, b) = (build_runtime_manager().await, build_runtime_manager().await);
        let mut a_state = a
            .compute_genesis(&[], &rand, BlockData::empty(), &pos_genesis, &vaults)
            .await
            .expect("compute_genesis")
            .1;
        let mut b_state = b
            .compute_genesis(&[], &rand, BlockData::empty(), &pos_genesis, &vaults)
            .await
            .expect("compute_genesis")
            .1;
        assert_eq!(
            a_state, b_state,
            "two nodes installing the same genesis must agree — the genesis seed is a constant, not a \
             config value, so there is nothing here to disagree about"
        );

        // Blocks 1 and 2: identical on both nodes. Height 2 is the first boundary, and it writes the
        // seed height 4 reads — captured below *while the store is at height 2*, because reading it
        // after height 4 would return the leaf height 4 wrote.
        let mut seed_at_2 = None;
        for height in 1..=2i64 {
            let term = format!("@\"t{trial}h{height}\"!(1)");
            let pa = play_block(&a, a_state, height, &term, &rand, &fringe_state(1)).await;
            let pb = play_block(&b, b_state, height, &term, &rand, &fringe_state(1)).await;
            assert_eq!(
                pa.post, pb.post,
                "trial {trial}: two nodes playing the same chain must reach the same state at height \
                 {height}"
            );
            assert_eq!(
                pa.bonds, pb.bonds,
                "trial {trial}: …and must draw the same set"
            );
            assert_eq!(pa.bonds.len(), 2, "the cap draws two of the four bonded");
            assert!(
                pa.bonds.keys().all(|v| pos_genesis.bonds.contains_key(v)),
                "the drawn set is a subset of the pool"
            );
            a_state = pa.post;
            b_state = pb.post;
            if height == 2 {
                let native = NativeSystemState::new(a.runtime().native_store());
                seed_at_2 = native.epoch_seed().await.expect("read");
            }
        }

        // Height 3: **not** a boundary, and where the two nodes diverge. This is the perturbation that
        // discriminates — it moves height 4's pre-state without moving the seed written at height 2.
        let pa3 = play_block(
            &a,
            a_state,
            3,
            &format!("@\"t{trial}h3a\"!(1)"),
            &rand,
            &fringe_state(1),
        )
        .await;
        let pb3 = play_block(
            &b,
            b_state,
            3,
            &format!("@\"t{trial}h3b\"!(2)"),
            &rand,
            &fringe_state(1),
        )
        .await;
        assert_ne!(
            pa3.post, pb3.post,
            "trial {trial}: the diverging deploys must reach different states"
        );

        // Height 4: the next boundary. Its own deploy differs too, so the states differ — but the draw
        // must not.
        let pa4 = play_block(
            &a,
            pa3.post,
            4,
            &format!("@\"t{trial}h4a\"!(1)"),
            &rand,
            &fringe_state(1),
        )
        .await;
        let pb4 = play_block(
            &b,
            pb3.post,
            4,
            &format!("@\"t{trial}h4b\"!(2)"),
            &rand,
            &fringe_state(1),
        )
        .await;
        assert_ne!(
            pa4.post, pb4.post,
            "trial {trial}: the boundary blocks differ, so their states must"
        );
        assert_eq!(
            pa4.bonds, pb4.bonds,
            "trial {trial}: the set drawn at a boundary must come from the seed its *previous* \
             boundary wrote, not from anything this block's own proposer varies — which is what the \
             rule this replaces got wrong"
        );
        assert_eq!(pa4.bonds.len(), 2);

        if trial == 0 {
            // The writer ran, and it is labelled for the epoch that will read it: `4 / 2 + 1`. Its two
            // anchors are pinned exactly, because "it wrote *something*" is satisfied by a writer that
            // drops the entropy input entirely — and the second anchor is the whole of the design: the
            // fringe state hash, which this proposer did not choose.
            let seed_at_2 = seed_at_2.expect("height 2 is a boundary and must have seeded epoch 3");
            let native = NativeSystemState::new(a.runtime().native_store());
            let seed = native
                .epoch_seed()
                .await
                .expect("read the seed leaf")
                .expect("a boundary must leave a seed behind for the next one");
            assert_eq!(seed.epoch, 3);
            assert_eq!(
                seed.anchors,
                vec![
                    previous_seed_anchor(&seed_at_2),
                    fringe_state(1),
                ],
                "the seed written at a boundary anchors on the previous seed and on the fringe state \
                 hash this block extends — in that order"
            );

            // Law 11 across the boundary. The replay path reads the seed leaf out of the state it
            // replayed, so a leaf written outside the checkpoint draws something else here and this
            // hash diverges.
            let (replayed, _) = a
                .replay_compute_state(
                    &pa3.post,
                    &pa4.user,
                    &pa4.sys,
                    &rand,
                    block_data(4),
                    &fringe_state(1),
                    &BTreeMap::new(),
                    true,
                    &pos_genesis,
                    &vaults,
                )
                .await
                .expect("replay the boundary block");
            assert_eq!(
                replayed, pa4.post,
                "replay must reproduce the play post-state across an epoch boundary"
            );
        }
    }
}
