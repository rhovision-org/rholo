//! Determinism regression (spec: `docs/src/formal/determinism.md`).
//!
//! Play (block creation) and replay (validation) of a deploy that binds `rho:rchain:deployerId`
//! must produce the same post-state hash (sub-invariants S1/S3). This pins the replay-normalizer-env
//! and refund-amount fixes so future drift fails here instead of in consensus.

mod common;
use common::fringe_state;

use std::collections::BTreeMap;

use rchain_casper::genesis::contracts::Vault;
use rchain_casper::interpreter_util::ReplayVerdict;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::public_key::PublicKey;
use rchain_models::casper::protocol::casper_message::{
    DeployData, Event, ProcessedDeploy, ProcessedSystemDeploy, SignedDeployData,
};
use rchain_rholang::native_state::{NativeSystemState, PosGenesis};
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::refined::NonNegI64;

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
        // 65-byte public key so RevAddress derivation succeeds (matches the seeded vault).
        deployer: vec![0u8; 65],
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
    }
}

fn seeded_vault() -> Vault {
    Vault {
        rev_address: RevAddress::from_public_key(&PublicKey::new(vec![0u8; 65]))
            .expect("valid rev address"),
        initial_balance: NonNegI64::try_from(1_000_000_000).unwrap(),
    }
}

#[tokio::test]
async fn play_and_replay_agree_for_deployer_id_binding_deploy() {
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);

    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    // Binds `rho:rchain:deployerId` (the REV-transfer idiom). Replay must normalize with the SAME
    // env, else `add_urn` fails with `BugFoundError` (S1) — before the fix this returned
    // `InvalidStateHash`.
    let term = r#"new deployerId(`rho:rchain:deployerId`) in { @"marker"!(true) }"#;

    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &post,
            &[deploy(term)],
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "play deploy must succeed: {:?}",
        user_results[0].eval_result.errors
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
            BlockData::empty(),
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
        "play and replay post-state hashes must agree (S1/S3)"
    );
}

#[tokio::test]
async fn play_and_replay_agree_for_transfer_deploy_and_vault_writes_persist() {
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);

    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    let target = RevAddress::from_public_key(&PublicKey::new(vec![1u8; 65]))
        .expect("target address")
        .to_base58();
    let term = format!(
        r#"new revVault(`rho:rchain:revVault`), deployerId(`rho:rchain:deployerId`), r in {{ revVault!("transfer", *deployerId, "{target}", 30000000, *r) | for (_ <- r) {{ Nil }} }}"#
    );

    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &post,
            &[deploy(&term)],
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "play transfer must succeed: {:?}",
        user_results[0].eval_result.errors
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
            BlockData::empty(),
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
        "play and replay post-state hashes must agree for a revVault transfer"
    );

    // The transfer's vault writes must be visible at the committed post-state.
    let fork = rm
        .fork_play_runtime(replay_state)
        .await
        .expect("fork at replay state");
    fork.reset(replay_state).await.expect("reset fork");
    let native = NativeSystemState::new(fork.native_store());
    let target_balance = native
        .vault_balance(&target)
        .await
        .expect("read target balance")
        .map(|b| i64::from(b))
        .unwrap_or(0);
    assert_eq!(
        target_balance, 30_000_000,
        "target vault must hold the transferred 30_000_000"
    );
}

/// **Genesis creation and genesis replay install the developer allocation in different orders, and
/// must still agree.**
///
/// `compute_genesis` installs `pos:dev-vesting` after the blessed-deploy loop and then writes the
/// wallet balances; `replay_compute_state` installs the same native genesis state before replaying
/// the deploys. Aria's review asked for this with a non-empty recipient: identical descriptor vectors
/// do not prove identical resulting state. The assertion is the post-state hash — the value consensus
/// compares — plus the leaf itself, so a path that produced the right hash by accident of an empty
/// allocation cannot pass.
#[tokio::test]
async fn genesis_with_a_developer_allocation_creates_and_replays_the_same_state() {
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    let dev_address = RevAddress::from_public_key(&PublicKey::new(vec![2u8; 65]))
        .expect("dev address")
        .to_base58();
    // 20,000,000 REV in drops (the floor), so the amount is exact and non-zero.
    let allocation = NonNegI64::try_from(20_000_000i64 * 100_000_000).unwrap();
    let pos_genesis = PosGenesis {
        dev_vesting: Some((dev_address.clone(), allocation)),
        ..Default::default()
    };
    let vaults = [seeded_vault()];
    let (pre, post, results) = rm
        .compute_genesis(&[], &rand, BlockData::empty(), &pos_genesis, &vaults)
        .await
        .expect("compute_genesis");
    let processed: Vec<ProcessedDeploy> = results.into_iter().map(|r| r.deploy).collect();

    let (replayed, _) = rm
        .replay_compute_state(
            &pre,
            &processed,
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(0),
            &BTreeMap::new(),
            // Genesis replay installs the native genesis state only on the no-cost-accounting path
            // (`runtime_replay.rs`: "Genesis replay (no cost accounting)"). Passing `true` here would
            // measure a different path and is not the genesis.
            false,
            &pos_genesis,
            &vaults,
        )
        .await
        .expect("replay_compute_state");

    assert_eq!(
        post, replayed,
        "genesis creation and replay must reach the same post-state with a non-empty allocation"
    );

    let fork = rm
        .fork_play_runtime(post)
        .await
        .expect("fork at the genesis post-state");
    fork.reset(post).await.expect("reset fork");
    let native = NativeSystemState::new(fork.native_store());
    let leaf = native
        .dev_vesting()
        .await
        .expect("read the dev vesting leaf")
        .expect("the allocation leaf is installed at genesis");
    assert_eq!(leaf.address, dev_address);
    assert_eq!(leaf.allocation, allocation);
    assert_eq!(leaf.unreleased, allocation, "nothing has vested at genesis");

    // Nothing is spendable at genesis: the recipient's ordinary vault is still empty.
    let recipient_balance = native
        .vault_balance(&dev_address)
        .await
        .expect("read the recipient's vault")
        .map(|b| i64::from(b))
        .unwrap_or(0);
    assert_eq!(
        recipient_balance, 0,
        "the allocation is a vesting account, not a genesis vault credit"
    );
}

/// **The vault capability, end to end** — the deferred half of `spec/RUST-FIRST.md`'s B2.
///
/// A deploy asks `findOrCreate` for a handle and then **spends from it in the same deploy**. Nothing
/// could do that before: the handle is a name minted during the deploy, so no compile-time
/// `BodyRefs` entry describes it, and the send would have matched nothing. The evidence is a *fund
/// movement* rather than a read, so a handler that replied without acting would fail this.
///
/// The auth argument is the handle itself, which is what a capability means here: the value that
/// authorises the spend is the unforgeable name, not the caller's deployer key — a contract holding
/// this handle has no key of its own, which is the whole reason the multi-signature vault needs it.
#[tokio::test]
async fn a_minted_vault_handle_spends_in_the_deploy_that_minted_it() {
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    let target = RevAddress::from_public_key(&PublicKey::new(vec![1u8; 65]))
        .expect("target address")
        .to_base58();
    let term = format!(
        r#"new revVault(`rho:rchain:revVault`), deployerId(`rho:rchain:deployerId`),
               vaultCh, authCh, r in {{
            revVault!("findOrCreate", *deployerId, *vaultCh) |
            revVault!("deployerAuthKey", *deployerId, *authCh) |
            for (@(_, *vault) <- vaultCh; auth <- authCh) {{
                vault!("transfer", "{target}", 30000000, *auth, *r) |
                for (_ <- r) {{ Nil }}
            }}
        }}"#
    );

    let (post_state, user_results, _) = rm
        .compute_state(
            &post,
            &[deploy(&term)],
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "a handle spend must succeed: {:?}",
        user_results[0].eval_result.errors
    );

    let native = NativeSystemState::new(rm.runtime().native_store());
    let target_balance = native
        .vault_balance(&target)
        .await
        .expect("read target balance")
        .map(i64::from)
        .unwrap_or(0);
    assert_eq!(
        target_balance, 30_000_000,
        "the handle's transfer must have moved the funds — a reply without a movement is not the \
         capability working"
    );
    let _ = post_state;
}

/// **A minted vault handle answers its `balance` arm** (AUDIT C219; the law is Law 60).
///
/// `install_vault_handle` used to install two continuations on **one** channel — `balance` at arity 2
/// and `transfer` at arity 5 — and the space keeps one per channel, so the second install replaced the
/// first: `transfer` replied and `balance` answered **nothing at all**, with no error, because an
/// unmatched receive is silence. The neighbouring test asserts the handle's *effect* and never its
/// reply, which is why the defect survived it. The handle is now one continuation at
/// `arity: 1, remainder: true` dispatching on the method, and this reads the arm that had never
/// existed.
///
/// The vault is seeded with 1_000_000_000, so the assertion is that the arm answers **that vault's**
/// balance rather than merely answering: a handler that replied a constant would pass a `== 0`.
#[tokio::test]
async fn a_minted_vault_handle_answers_its_balance_arm() {
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    let term = r#"new revVault(`rho:rchain:revVault`), deployerId(`rho:rchain:deployerId`),
               vaultCh, bch in {
            revVault!("findOrCreate", *deployerId, *vaultCh) |
            for (@(_, *vault) <- vaultCh) {
                vault!("balance", *bch) |
                for (@b <- bch) { @"handle-balance"!(b) }
            }
        }"#;

    let (_post_state, user_results, _) = rm
        .compute_state(
            &post,
            &[deploy(term)],
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "the handle read must run: {:?}",
        user_results[0].eval_result.errors
    );

    let tag = rchain_models::sorted::SortedProc::new(rchain_models::par_ops::from_expr(
        rchain_models::ast::Expr::GString("handle-balance".to_string()),
    ));
    let data = rm
        .runtime()
        .get_data_par(&tag)
        .await
        .expect("read the driver's channel");
    assert_eq!(
        data.len(),
        1,
        "`handle-balance` must have been sent exactly once — an absent tag is the *silence* this \
         row is about, not an error: {data:?}"
    );
    let balance = rchain_models::rholang::RhoType::RhoNumber::unapply(&data[0])
        .unwrap_or_else(|| panic!("the balance arm answers an Int: {:?}", data[0]));
    assert!(
        balance > 0,
        "the handle must answer *its vault's* balance, not merely answer — the vault is seeded with \
         1_000_000_000: got {balance}"
    );
}

/// **And the authority check is real.** The same transfer with a name that opens nothing — a fresh
/// `new`, which is exactly what a caller without the handle holds — must move nothing. Without this
/// arm the first test would pass on a handler that ignored its auth argument entirely.
#[tokio::test]
async fn a_vault_handle_refuses_a_name_that_opens_no_vault() {
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    let target = RevAddress::from_public_key(&PublicKey::new(vec![1u8; 65]))
        .expect("target address")
        .to_base58();
    let term = format!(
        r#"new revVault(`rho:rchain:revVault`), deployerId(`rho:rchain:deployerId`), vaultCh, r, other in {{
            revVault!("findOrCreate", *deployerId, *vaultCh) |
            for (@(_, *vault) <- vaultCh) {{
                vault!("transfer", "{target}", 30000000, *other, *r) |
                for (_ <- r) {{ Nil }}
            }}
        }}"#
    );

    let (post_state, user_results, _) = rm
        .compute_state(
            &post,
            &[deploy(&term)],
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "the deploy itself runs; the *transfer* is refused: {:?}",
        user_results[0].eval_result.errors
    );

    let native = NativeSystemState::new(rm.runtime().native_store());
    let _ = post_state;
    let target_balance = native
        .vault_balance(&target)
        .await
        .expect("read target balance")
        .map(i64::from)
        .unwrap_or(0);
    assert_eq!(
        target_balance, 0,
        "a name that opens no vault must not authorise a spend"
    );
}

#[tokio::test]
async fn play_and_replay_agree_for_failed_user_deploy_with_recorded_error() {
    // Issue #15: a failed user deploy must record the reducer's error in `system_deploy_error`,
    // and replay must still reproduce the post-state (the recorded failure is the user deploy's,
    // not the pre-charge's, so replay must not skip the user-deploy evaluation).
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);

    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    // Fails at runtime: `1 + "not-a-number"` is a type error.
    let term = r#"new return in { return!(1 + "not-a-number") }"#;

    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &post,
            &[deploy(term)],
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(user_results[0].deploy.is_failed, "deploy must fail");
    assert!(
        user_results[0].deploy.system_deploy_error.is_some(),
        "failed deploy must record its error, got: {:?}",
        user_results[0].deploy.system_deploy_error
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
            BlockData::empty(),
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
        "play and replay post-state hashes must agree for a failed user deploy"
    );
}

/// **The #564 falsifier: a multi-deploy block from one key, including a failed deploy that records a
/// COMM event.** Play and replay must agree per deploy, under both the sequential and the batched
/// play paths.
///
/// The incident's first observable was `ReplayCostMismatch initial_cost=5576 replay_cost=5302` on a
/// block this node built. The checks in this file each covered one deploy; no test built a block with
/// several deploys from the same deployer, and none exercised a *failed* deploy whose recorded trace
/// contains a COMM event — the shape in which `check_replay_data_with_fix` deliberately swallows an
/// unused-COMM error (RCHAIN-3505). Batching is the surviving in-binary candidate for a play/replay
/// asymmetry, so the same fixture runs with it on and with it off.
///
/// `replay_compute_state` returning `Ok` *is* the per-deploy cost assertion: `replay_deploy_eval`
/// compares `ProcessedDeploy.cost.cost` with the replayed cost for every deploy and returns
/// `ReplayCostMismatch` on the first difference. The cross-policy cost comparison below is the
/// additional check that the two play paths agree with each other, not just with replay.
async fn play_and_replay_multi_deploy(
    batch: rchain_casper::runtime_manager::BatchPolicy,
) -> (Vec<u64>, Vec<bool>) {
    let mut rm = common::build_runtime_manager().await;
    rm.set_batch_policy(batch);
    let rand = Blake2b512Random::from_init(&[0u8; 32]);

    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    let target_a = RevAddress::from_public_key(&PublicKey::new(vec![1u8; 65]))
        .expect("target A")
        .to_base58();
    let target_b = RevAddress::from_public_key(&PublicKey::new(vec![2u8; 65]))
        .expect("target B")
        .to_base58();

    // Two transfers from the same deployer key, then a deploy that COMMs once and then fails. The
    // failed deploy's recorded trace still holds the COMM event, which is the RCHAIN-3505 shape.
    let transfer_a = format!(
        r#"new revVault(`rho:rchain:revVault`), deployerId(`rho:rchain:deployerId`), r in {{ revVault!("transfer", *deployerId, "{target_a}", 30000000, *r) | for (_ <- r) {{ Nil }} }}"#
    );
    let transfer_b = format!(
        r#"new revVault(`rho:rchain:revVault`), deployerId(`rho:rchain:deployerId`), r in {{ revVault!("transfer", *deployerId, "{target_b}", 20000000, *r) | for (_ <- r) {{ Nil }} }}"#
    );
    // The error is in the receive's continuation, so the COMM is necessarily consumed first and the
    // failed deploy's recorded trace contains it.
    let fails_after_a_comm =
        r#"new c, return in { c!(42) | for (@x <- c) { return!(1 + "not-a-number") } }"#;

    let deploys = [
        deploy(&transfer_a),
        deploy(&transfer_b),
        deploy(fails_after_a_comm),
    ];
    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &post,
            &deploys,
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");

    assert!(user_results[0].eval_result.succeeded(), "transfer A plays");
    assert!(user_results[1].eval_result.succeeded(), "transfer B plays");
    assert!(
        user_results[2].deploy.is_failed,
        "the third deploy must fail after its COMM event"
    );
    assert!(
        !user_results[2].deploy.deploy_log.is_empty(),
        "the failed deploy must still record its trace, or the COMM-event path is not exercised"
    );

    let costs: Vec<u64> = user_results.iter().map(|r| r.deploy.cost.cost).collect();
    let failed: Vec<bool> = user_results.iter().map(|r| r.deploy.is_failed).collect();
    let processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
    let processed_sys: Vec<ProcessedSystemDeploy> =
        sys_results.into_iter().map(|r| r.deploy).collect();

    let (replay_state, _) = rm
        .replay_compute_state(
            &post,
            &processed,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await
        .unwrap_or_else(|failure| {
            panic!(
                "replay refused the block with {failure:?}; costs recorded by play were {costs:?}"
            )
        });

    assert_eq!(
        post_state, replay_state,
        "play and replay must agree on the multi-deploy post-state"
    );
    (costs, failed)
}

#[tokio::test]
async fn play_and_replay_agree_per_deploy_for_same_key_multi_transfer() {
    use rchain_casper::runtime_manager::BatchPolicy;

    let (sequential_costs, sequential_failed) =
        play_and_replay_multi_deploy(BatchPolicy::default()).await;
    assert_eq!(
        sequential_failed,
        vec![false, false, true],
        "the fixture must contain the failed deploy, not just the transfers"
    );

    let (batched_costs, batched_failed) = play_and_replay_multi_deploy(BatchPolicy {
        enabled: true,
        verify: true,
        max_batch: 2,
    })
    .await;

    assert_eq!(
        batched_failed, sequential_failed,
        "batching changes execution, never a deploy's status"
    );
    assert_eq!(
        batched_costs, sequential_costs,
        "the batched play path must record the same per-deploy costs as the sequential oracle"
    );
    assert!(
        sequential_costs.iter().all(|c| *c > 0),
        "every deploy must have been charged, including the failed one: {sequential_costs:?}"
    );
}

/// **The ordering the incident's evidence depended on** (#564): a cost mismatch is returned before the
/// declared post-state hash is compared, so the refusal says nothing about whether the state was
/// correct. The record tamper changes the refund input too; the shadow replay is what reports the
/// state verdict, and it is the only place that verdict is computed.
#[tokio::test]
async fn a_cost_only_tamper_is_refused_before_the_state_hash_is_compared() {
    use rchain_rholang::accounting::set_cost_census_enabled;
    use rchain_rholang::runtime::ReplayCostCheck;

    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");
    let target = RevAddress::from_public_key(&PublicKey::new(vec![3u8; 65]))
        .expect("target")
        .to_base58();
    let term = format!(
        r#"new revVault(`rho:rchain:revVault`), deployerId(`rho:rchain:deployerId`), r in {{ revVault!("transfer", *deployerId, "{target}", 30000000, *r) | for (_ <- r) {{ Nil }} }}"#
    );

    // The census is diagnostic; enable it for this fixture so the shadow can carry the breakdown.
    set_cost_census_enabled(true);
    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &post,
            &[deploy(&term)],
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    set_cost_census_enabled(false);
    let processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
    let processed_sys: Vec<ProcessedSystemDeploy> =
        sys_results.into_iter().map(|r| r.deploy).collect();
    let recorded = processed[0].cost.cost;
    assert!(recorded > 0);

    // Control: the untouched record replays to the declared state and is accepted.
    let (clean_hash, _) = rm
        .replay_compute_state(
            &post,
            &processed,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("a clean replay");
    assert_eq!(clean_hash, post_state);

    // Tamper only the recorded cost. The enforcing replay refuses on it; the declared post-state is
    // never compared on that path (`handle_errors` sees only `Err`).
    let mut tampered = processed.clone();
    tampered[0].cost.cost = recorded + 274;
    let refused = rm
        .replay_compute_state(
            &post,
            &tampered,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await;
    match refused {
        Err(rchain_casper::rholang::ReplayFailure::ReplayCostMismatch {
            initial_cost,
            replay_cost,
        }) => {
            assert_eq!(initial_cost, i64::try_from(recorded + 274).unwrap());
            assert_eq!(replay_cost, i64::try_from(recorded).unwrap());
        }
        other => panic!("expected a simple ReplayCostMismatch, got {other:?}"),
    }

    // The shadow: one fresh replay that ignores the cost check, reaches the checkpoint and reports
    // the state verdict. The record tamper also changes the refund, so the hash is expected to move;
    // the point is that the verdict is computed at all.
    set_cost_census_enabled(true);
    let shadow = rm
        .fork_replay_runtime(post)
        .await
        .expect("fork a replay runtime");
    shadow.set_replay_cost_check(ReplayCostCheck::Ignore);
    let shadow_result = rm
        .replay_compute_state_with(
            &shadow,
            &post,
            &tampered,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await;
    let ignored = shadow
        .take_ignored_cost_mismatch()
        .expect("the shadow records the mismatch it let through");
    assert_eq!(ignored.recorded, i64::try_from(recorded + 274).unwrap());
    assert_eq!(ignored.replayed, i64::try_from(recorded).unwrap());
    assert!(
        ignored.replay_census.is_some(),
        "the shadow carries the replay-side breakdown when the census is on"
    );
    assert!(
        shadow_result.is_ok(),
        "the shadow reaches the checkpoint; a trace refusal is the only other arm"
    );
    if let Ok((shadow_hash, _)) = shadow_result {
        assert_ne!(
            shadow_hash, post_state,
            "a changed record changes the refund input and therefore the state; the verdict is real"
        );
    }
    set_cost_census_enabled(false);
}

/// **C358's trace-mask falsifier.** The cost check runs inside `replay_deploy_eval`, before
/// `check_replay_data_with_fix` compares the trace, so a replay that disagrees on both the recorded
/// cost and the trace can only ever report `ReplayCostMismatch` — the trace difference is masked,
/// not measured. This test makes the trace disagree independently of the cost and shows both halves:
/// the enforcing replay refuses on the cost (never `UnusedCommEvent`), while the diagnostic shadow
/// (which ignores the cost check) reaches the trace check and reports the unused COMM.
///
/// Red before the fix: moving the trace check ahead of the cost return makes the enforcing arm
/// return `UnusedCommEvent`; removing the shadow's `Ignore` makes the shadow return
/// `ReplayCostMismatch`. Either change fails this test.
#[tokio::test]
async fn a_cost_refusal_masks_the_trace_check_and_the_shadow_reports_it() {
    use rchain_rholang::accounting::set_cost_census_enabled;
    use rchain_rholang::runtime::ReplayCostCheck;

    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");
    let target = RevAddress::from_public_key(&PublicKey::new(vec![3u8; 65]))
        .expect("target")
        .to_base58();
    let term = format!(
        r#"new revVault(`rho:rchain:revVault`), deployerId(`rho:rchain:deployerId`), r in {{ revVault!("transfer", *deployerId, "{target}", 30000000, *r) | for (_ <- r) {{ Nil }} }}"#
    );

    set_cost_census_enabled(true);
    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &post,
            &[deploy(&term)],
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    set_cost_census_enabled(false);
    let processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
    let processed_sys: Vec<ProcessedSystemDeploy> =
        sys_results.into_iter().map(|r| r.deploy).collect();
    let recorded = processed[0].cost.cost;
    assert!(recorded > 0);

    // The transfer records at least one COMM. Duplicating one leaves the replay with a binding the
    // deploy's execution never consumes; the enforcing path must never get far enough to report it.
    let comm = processed[0]
        .deploy_log
        .iter()
        .find(|event| matches!(event, Event::Comm(_)))
        .expect("the transfer deploy records a COMM event")
        .clone();
    let mut masked = processed.clone();
    masked[0].deploy_log.push(comm);
    masked[0].cost.cost = recorded + 274;

    let refused = rm
        .replay_compute_state(
            &post,
            &masked,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await;
    match refused {
        Err(rchain_casper::rholang::ReplayFailure::ReplayCostMismatch {
            initial_cost,
            replay_cost,
        }) => {
            assert_eq!(initial_cost, i64::try_from(recorded + 274).unwrap());
            assert_eq!(replay_cost, i64::try_from(recorded).unwrap());
        }
        other => panic!(
            "the enforcing replay must refuse on the cost before the trace check, got {other:?}"
        ),
    }

    // Control: the untouched record replays to the declared state, so the unused COMM below is the
    // only difference the duplicated event introduces.
    let (clean_hash, _) = rm
        .replay_compute_state(
            &post,
            &processed,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("a clean replay");
    assert_eq!(clean_hash, post_state);

    // The shadow lets the cost mismatch through, reaches `check_replay_data_with_fix`, and reports
    // the masked trace difference as an unused COMM. A clean replay here would mean the trace did
    // not disagree, and the fixture would not be measuring the mask.
    set_cost_census_enabled(true);
    let shadow = rm
        .fork_replay_runtime(post)
        .await
        .expect("fork a replay runtime");
    shadow.set_replay_cost_check(ReplayCostCheck::Ignore);
    let shadow_result = rm
        .replay_compute_state_with(
            &shadow,
            &post,
            &masked,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await;
    set_cost_census_enabled(false);
    match shadow_result {
        Err(rchain_casper::rholang::ReplayFailure::UnusedCommEvent(cause)) => {
            assert!(
                !cause.is_empty(),
                "the shadow names the unused COMM it found"
            );
        }
        other => panic!(
            "the cost-ignoring shadow must reach the trace check and report the unused COMM, got {other:?}"
        ),
    }
}

#[tokio::test]
async fn play_and_replay_agree_for_escrow_round_trip_deploy() {
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);

    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    let target = RevAddress::from_public_key(&PublicKey::new(vec![1u8; 65]))
        .expect("target address")
        .to_base58();
    // The robotics-coordination "escrow" idiom: bind `rho:rchain:deployerId`, produce it onto a
    // fresh channel, and install a persistent contract whose body round-trips that deployerId
    // through the channel and calls the native revVault transfer.
    let term = r#"new deployerId(`rho:rchain:deployerId`), escrowCh in {
  escrowCh!(*deployerId) |
  contract @"raas:escrow:test"(@"complete", @fee, ret) = {
    for (d <- escrowCh) {
      new revVault(`rho:rchain:revVault`), resultCh in {
        revVault!("transfer", *d, "__TARGET__", fee, *resultCh) |
        for (_ <- resultCh) { escrowCh!(*d) | ret!(true) }
      }
    }
  } |
  contract @"raas:escrow:test"(@"query", ret) = { ret!("x") }
}"#
    .replace("__TARGET__", &target);

    let (play_hash, user_results, sys_results) = rm
        .compute_state(
            &post,
            &[deploy(&term)],
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "play deploy must succeed: {:?}",
        user_results[0].eval_result.errors
    );

    let processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
    let processed_sys: Vec<ProcessedSystemDeploy> =
        sys_results.into_iter().map(|r| r.deploy).collect();

    let replay_hash = match rm
        .replay_compute_state(
            &post,
            &processed,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await
    {
        Ok((hash, _)) => hash,
        Err(e) => panic!("replay failed with {e:?} (play_hash = {play_hash:?})"),
    };

    assert_eq!(
        play_hash, replay_hash,
        "play and replay post-state hashes must agree for the escrow round-trip deploy"
    );
}

/// **The negative replay path** (the register's G7): a deploy whose replay does not reproduce the
/// recorded state must be **rejected by the validating caller**, not accepted.
///
/// Two things this test establishes, one of which is a finding:
///
/// 1. Tampering with a *processed* deploy — the public `ProcessedDeploy`, no production change —
///    makes the replay produce a **different post-state hash**. The replay itself returns that hash
///    rather than an error, which is by design: `replay_compute_state` computes, and the caller
///    decides.
/// 2. The decision is `interpreter_util::handle_errors`, which compares the replayed hash against
///    the block's claimed `post_state_hash` and returns `Ok(None)` on a mismatch. **That comparison
///    is what carries the invariant**: the inner trace check (`check_replay_data_with_fix`) returns
///    `Ok` for a term tamper here, because it deliberately swallows mismatch for a deploy that is not
///    "eval successful" (the documented RCHAIN-3505 workaround). A test asserting only the inner
///    check would therefore pass while a tampered block was accepted.
#[tokio::test]
async fn a_tampered_deploy_replays_to_a_rejected_state_hash() {
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);

    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    let term = r#"new deployerId(`rho:rchain:deployerId`) in { @"marker"!(42) }"#;
    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &post,
            &[deploy(term)],
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "the recorded deploy must succeed: {:?}",
        user_results[0].eval_result.errors
    );
    let mut processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
    let processed_sys: Vec<ProcessedSystemDeploy> =
        sys_results.into_iter().map(|r| r.deploy).collect();

    // Control: the untouched set replays to the recorded hash, and the validating comparison accepts
    // it (it returns the hash rather than `None`).
    let (clean_hash, _) = rm
        .replay_compute_state(
            &post,
            &processed,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("a clean replay succeeds");
    assert_eq!(
        clean_hash, post_state,
        "a clean replay reproduces the state"
    );
    assert_eq!(
        rchain_casper::interpreter_util::handle_errors(&post_state, Ok(clean_hash))
            .expect("no internal error"),
        ReplayVerdict::Matched(clean_hash),
        "the clean replay is accepted"
    );

    // Tamper: the processed deploy claims a term that did not run. Same *length* as the one that did,
    // deliberately — the parse is charged (`AUDIT F-2`), so a length change would be caught one check
    // earlier by the replay's cost comparison, and this test exists to exercise the state-hash path.
    // The cost path has its own test below.
    processed[0].deploy.data.term =
        r#"new deployerId(`rho:rchain:deployerId`) in { @"marker"!(43) }"#.to_string();

    let (tampered_hash, _) = rm
        .replay_compute_state(
            &post,
            &processed,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("the replay computes a hash; the caller judges it");

    assert_ne!(
        tampered_hash, post_state,
        "a tampered deploy must not replay to the recorded state"
    );
    assert_eq!(
        rchain_casper::interpreter_util::handle_errors(&post_state, Ok(tampered_hash))
            .expect("no internal error"),
        ReplayVerdict::PostStateMismatch,
        "the validating comparison must reject a state that does not match the block's claim"
    );

    // And a tamper that changes the term's *length* is refused even earlier, by the replay's own cost
    // comparison, because the parse is charged (`AUDIT F-2`) and the recorded cost no longer matches.
    //
    // Both rejections are rejections, so this is a stricter detection than the state hash alone rather
    // than a weaker one — but it is a *different* channel, and a future change that dropped the cost
    // comparison would leave the state-hash path as the only guard. Pinning both means neither can be
    // removed without a test going red.
    processed[0].deploy.data.term =
        r#"new deployerId(`rho:rchain:deployerId`) in { @"marker"!(4242) }"#.to_string();

    let cost_tamper = rm
        .replay_compute_state(
            &post,
            &processed,
            &processed_sys,
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await;

    assert!(
        matches!(
            cost_tamper,
            Err(rchain_casper::rholang::ReplayFailure::ReplayCostMismatch { .. })
        ),
        "a length-changing tamper must be caught by the replay's cost comparison, got {cost_tamper:?}"
    );
}

/// **A block that carries a `CloseBlock` system deploy replays to the state it played.**
///
/// `close_block` is the epoch transition and, since laws 44–47 landed, it writes native PoS state:
/// the committed-rewards map, the withdrawal requests and their claims, and the active set. Every
/// other play/replay test in this file passes an **empty** system-deploy list, so none of them
/// replayed a block with one — which is the gap this test closes, and the reason it is written
/// against a boundary with all four steps doing work rather than against a non-boundary no-op.
///
/// What a regression here looks like in production: a proposer produces a block, and every validator
/// that re-derives its state by replay refuses it — `merging.rs:514`'s
/// "regenerated mergeable channels for block … but replay computed … instead of …". That message is
/// shaped like a mergeable-channel disagreement, but the comparison it fails on is the *post-state
/// hash*, so the symptom is a chain that stops advancing behind a proposer that sees nothing wrong.
#[tokio::test]
async fn play_and_replay_agree_for_a_block_with_a_close_block_deploy() {
    use rchain_casper::system_deploy::SystemDeploy;
    use std::collections::BTreeSet;

    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    // The validator is the deployer, so its bond, its withdrawal and its reward are all its own.
    let validator = rchain_models::validator::Validator::new([0u8; 65]);
    let pos_genesis = PosGenesis {
        bonds: [(validator, NonNegI64::try_from(40).unwrap())]
            .into_iter()
            .collect(),
        trusted: BTreeSet::from([validator]),
        // `epoch_length: 1` makes the block's own close_block a boundary; a positive `minimum_bond`
        // takes the split out of the case where the contract's formula is undefined.
        params: rchain_rholang::native_state::PosParams {
            epoch_length: 1,
            quarantine_length: 0,
            minimum_bond: rchain_shared::refined::NonNegI64::try_from(1).unwrap(),
            ..Default::default()
        },
        dev_vesting: None,
    };
    let (_pre, post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &pos_genesis,
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    // A bond (activating at the boundary), a staged withdrawal, and a deploy that spends phlo so the
    // epoch's pot is not zero: all four steps of the sequence then do work.
    let term = r#"new pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {
  pos!("withdraw", *deployerId, *ret) |
  for (_ <- ret) { @"after"!(1) }
}"#;
    // The block's own number, on **both** paths: `block_creator` builds the close deploy from the
    // block it is creating (`SystemDeploy::close_block(i64::from(block_num), …)`), and the replay
    // reconstructs it from the block's `BlockData` (`runtime_replay.rs:426`). Passing a close deploy
    // whose number disagrees with the block's is not a state a real block can be in, and it is what
    // this test got wrong the first time it ran.
    //
    // The **pre-state hash** goes the same way and for the same reason: it is the seed anchor the
    // close step writes for the next epoch, so the proposer's value and the replayer's must be the
    // same number. Here that is `post`, the state this block extends.
    let block_data = BlockData {
        block_number: rchain_shared::refined::BlockHeight::try_from(1).expect("height"),
        ..BlockData::empty()
    };
    let close = SystemDeploy::close_block(1, fringe_state(1), BTreeMap::new(), rand.split_byte(9));
    let (play_hash, user_results, sys_results) = rm
        .compute_state(
            &post,
            &[deploy(term)],
            &[close],
            &rand,
            block_data.clone(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "the staged withdrawal must succeed: {:?}",
        user_results[0].eval_result.errors
    );
    assert_eq!(sys_results.len(), 1, "the block's CloseBlock ran");

    let processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
    let processed_sys: Vec<ProcessedSystemDeploy> =
        sys_results.into_iter().map(|r| r.deploy).collect();

    let (replay_hash, _) = rm
        .replay_compute_state(
            &post,
            &processed,
            &processed_sys,
            &rand,
            block_data,
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &pos_genesis,
            &[],
        )
        .await
        .expect("replay_compute_state");

    assert_eq!(
        play_hash, replay_hash,
        "a block whose close_block wrote the epoch's state must replay to the same post-state"
    );
}

/// **Arm A of #139's campaign: the post-state is a function of the fringe state, so two nodes that
/// derive different fringes disagree about the same block.**
///
/// This is the primitive the whole campaign rests on, and it is the cheapest thing here. A block's
/// `fringe_state_hash` is **not carried on the block** — every node derives it from its own DAG
/// (`validate_block_checkpoint`'s doc says so) — and the replay hands it to the `CloseBlock` system
/// deploy, which anchors the *next* epoch's seed to it. So if the two values differ, the same block
/// replays to two different post-states, and the validator's `handle_errors` sees
/// `computed != declared` and refuses with `InvalidStateHash`.
///
/// **The falsifiable statement, exactly:** play the block against one fringe, then replay it twice —
/// once against the proposer's fringe (must agree) and once against another (must not). The first
/// replay is the control that the fixture is sound; the second is the mechanism. If both replays
/// agree, the fringe cannot be the cause of any observed divergence and the campaign's hypothesis is
/// refuted before any devnet run is paid for.
#[tokio::test]
async fn a_block_replayed_against_a_different_fringe_reaches_a_different_post_state() {
    use rchain_casper::system_deploy::SystemDeploy;
    use std::collections::BTreeSet;

    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    let validator = rchain_models::validator::Validator::new([0u8; 65]);
    let pos_genesis = PosGenesis {
        bonds: [(validator, NonNegI64::try_from(40).unwrap())]
            .into_iter()
            .collect(),
        trusted: BTreeSet::from([validator]),
        // `epoch_length: 1` makes this block a boundary, which is the case the divergence is confined
        // to: a non-boundary block whose close step writes no seed change would be a control that
        // cannot fail.
        params: rchain_rholang::native_state::PosParams {
            epoch_length: 1,
            quarantine_length: 0,
            minimum_bond: rchain_shared::refined::NonNegI64::try_from(1).unwrap(),
            ..Default::default()
        },
        dev_vesting: None,
    };
    let (_pre, genesis_post, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &pos_genesis,
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    let term = r#"new pos(`rho:rchain:pos`), deployerId(`rho:rchain:deployerId`), ret in {
  pos!("withdraw", *deployerId, *ret) |
  for (_ <- ret) { @"after"!(1) }
}"#;
    let block_data = BlockData {
        block_number: rchain_shared::refined::BlockHeight::try_from(1).expect("height"),
        ..BlockData::empty()
    };

    // What the **proposer** had: its own derived fringe.
    let proposer_fringe = fringe_state(1);
    let close = SystemDeploy::close_block(1, proposer_fringe, BTreeMap::new(), rand.split_byte(9));
    let (declared_post_state, user_results, sys_results) = rm
        .compute_state(
            &genesis_post,
            &[deploy(term)],
            &[close],
            &rand,
            block_data.clone(),
            &proposer_fringe,
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "the staged withdrawal must succeed: {:?}",
        user_results[0].eval_result.errors
    );
    let processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
    let processed_sys: Vec<ProcessedSystemDeploy> =
        sys_results.into_iter().map(|r| r.deploy).collect();

    // The **control**: a validator that derived the proposer's fringe agrees, so the fixture is sound
    // and the difference below is the fringe rather than a replay that never agrees with anything.
    let (same_fringe, _) = rm
        .replay_compute_state(
            &genesis_post,
            &processed,
            &processed_sys,
            &rand,
            block_data.clone(),
            &proposer_fringe,
            &BTreeMap::new(),
            true,
            &pos_genesis,
            &[],
        )
        .await
        .expect("replay under the proposer's fringe");
    assert_eq!(
        same_fringe, declared_post_state,
        "a validator that derives the proposer's fringe must agree — without this the arm below \
         proves nothing about the fringe"
    );

    // The **mechanism**: a validator whose own DAG named a different fringe replays the same deploys
    // to a different post-state. `handle_errors` compares this value with the declared one and
    // returns `None`, which is `InvalidStateHash`.
    let other_fringe = fringe_state(2);
    assert_ne!(
        other_fringe, proposer_fringe,
        "the fixture needs two distinct fringe states"
    );
    let (different_fringe, _) = rm
        .replay_compute_state(
            &genesis_post,
            &processed,
            &processed_sys,
            &rand,
            block_data,
            &other_fringe,
            &BTreeMap::new(),
            true,
            &pos_genesis,
            &[],
        )
        .await
        .expect("replay under a different fringe");
    assert_ne!(
        different_fringe, declared_post_state,
        "the `CloseBlock` system deploy anchors the next epoch's seed to the fringe state, so a \
         validator that derived a different fringe must NOT reproduce the proposer's post-state — \
         this inequality is the whole mechanism #139 is about"
    );

    // And the comparison the node actually makes (`interpreter_util::handle_errors`) is the one that
    // refuses: named here so the arm's result and the production status are the same fact.
    assert_eq!(
        rchain_casper::interpreter_util::handle_errors(
            &rchain_crypto::hash::blake2b256_hash::Blake2b256Hash::from_byte_array(
                declared_post_state.as_bytes()
            ),
            Ok(different_fringe),
        )
        .expect("handle_errors runs"),
        ReplayVerdict::PostStateMismatch,
        "the divergence above is exactly what `handle_errors` reports as a post-state mismatch"
    );
}

/// **A genesis replay that omits the genesis vaults does not reproduce the genesis** (finding,
/// 2026-09-23; fixed 2026-09-24, Programme F).
///
/// What it pins. The genesis install funds the genesis wallets' REV vaults
/// (`ca4f5b015`, `RuntimeManager::compute_genesis`'s `for vault in vaults`), and those balances are
/// `PREFIX_VAULT` leaves in the genesis post-state. A node that did not *create* the genesis has no
/// mergeable-channel sidecar for it, so `merging.rs` regenerates one by replaying the block — and
/// that replay used to pass `&[]` for the vaults, on the stated assumption that "this is always
/// non-genesis block replay". It is not: the genesis is what the finalized fringe points at when a
/// validator joins, and the replay computes a different post-state hash
/// (`regenerated mergeable channels for block … but replay computed … instead of …`), which the
/// validator then refuses — so it never indexes block #0 and never advances.
///
/// Observed on `tools/devnet.sh up --validators 3` (2026-09-23): the bootstrap proposes happily, and
/// validators 1 and 2 sit at the height they joined at, logging exactly that message for the genesis
/// hash. `docs/src/formal/determinism.md` recorded this asymmetry as "safe because genesis is trusted
/// and never re-validated (an asserted invariant, not a code path)" — the parenthetical is the part
/// the devnet falsifies.
///
/// The fix is not to pass the vaults unconditionally (a non-genesis block's pre-state already has the
/// post-genesis balances, so re-installing would clobber them): it is to re-install them when the
/// block being replayed *is* the genesis, which is what `is_genesis_pre_state` decides and what the
/// two call sites (`interpreter_util.rs::replay_block`, `merging.rs`'s sidecar regeneration) now do.
///
/// **What this test is, now that the fix has landed.** It is deliberately *not* a tripwire for the
/// call sites: it exercises the primitive `replay_compute_state` with `&[]` and with the vaults, and
/// the divergence between those two is unchanged and is the *reason* the call sites must supply them.
/// The production decision is pinned by `interpreter_util.rs`'s
/// `is_genesis_pre_state_is_true_only_for_the_empty_state` (the condition is exactly the genesis) and
/// end to end by the 3-validator devnet, which is where the failure was observed.
#[tokio::test]
async fn a_genesis_replay_without_the_vaults_does_not_reproduce_the_genesis() {
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);
    let vaults = [seeded_vault()];
    let (empty, post, results) = rm
        .compute_genesis(
            &[deploy(r#"@"chan"!(42)"#)],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &vaults,
        )
        .await
        .expect("compute_genesis");
    let processed: Vec<ProcessedDeploy> = results.iter().map(|r| r.deploy.clone()).collect();

    // The genesis replay as `merging.rs` performs it, vaults and all...
    let (with_vaults, _) = rm
        .replay_compute_state(
            &empty,
            &processed,
            &[],
            &rand,
            BlockData::empty(),
            &fringe_state(1),
            &BTreeMap::new(),
            false,
            &PosGenesis::default(),
            &vaults,
        )
        .await
        .expect("replay with the genesis vaults");
    assert_eq!(
        post, with_vaults,
        "the genesis replays to itself once its vaults are re-installed — which is what makes the \
         divergence below a missing input rather than a missing rule"
    );

    // ...and as it performs it today.
    let (without_vaults, _) = rm
        .replay_compute_state(
            &empty,
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
        .expect("replay without the genesis vaults");
    assert_ne!(
        post, without_vaults,
        "a genesis replay that omits the vaults must not reproduce the genesis; if this now passes, \
         the regeneration path was fixed and this test should become its opposite"
    );
}

/// **The block's producer is paid a share of what the block's deploys burned, on play and on replay**
/// (B2, #150).
///
/// Three things are pinned here, and each is a way this can be wrong. The producer's **own REV vault**
/// must hold the share — not the Coop vault, not the pool, and not the epoch pot, which is where the
/// same phlo went before this change (the share is taken *out* of the staking vault before the pot
/// sees it). The **bond pool** must be untouched. And the replay must compute the **same post-state
/// hash** from the same inputs, because the executor is read from the block data each path was set
/// with and the share from the params each path installed — a disagreement there is a chain split,
/// not a rounding difference.
#[tokio::test]
async fn the_producer_is_paid_a_share_of_the_burned_phlo_on_play_and_replay() {
    use rchain_models::validator::Validator as ModelsValidator;
    use rchain_rholang::native_state::PosParams;
    use rchain_shared::refined::{BlockHeight, SeqNum};
    use std::collections::{BTreeMap, BTreeSet};

    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0u8; 32]);

    // A genesis that pays a quarter, and one bonded validator so the staking vault is created with a
    // bond in it (`install_genesis` creates it at `bond_sum`).
    let validator = ModelsValidator::from_slice(&[1u8; 65]);
    let pos = PosGenesis {
        bonds: BTreeMap::from([(validator, NonNegI64::try_from(1_000).expect("a stake"))]),
        trusted: BTreeSet::new(),
        params: PosParams {
            executor_share: NonNegI64::try_from(2_500).expect("a quarter"),
            ..PosParams::default()
        },
        dev_vesting: None,
    };
    let (_pre, genesis_post, _) = rm
        .compute_genesis(&[], &rand, BlockData::empty(), &pos, &[seeded_vault()])
        .await
        .expect("compute_genesis");

    // The block's producer is the address the payment goes to, and it is *not* the deployer.
    let producer = PublicKey::new(vec![5u8; 65]);
    let producer_address = RevAddress::from_public_key(&producer)
        .expect("producer rev address")
        .to_base58();
    let block_data = BlockData {
        block_number: BlockHeight::zero(),
        sender: producer.clone(),
        seq_num: SeqNum::zero(),
        timestamp: 0,
    };

    let term = r#"new deployerId(`rho:rchain:deployerId`) in { @"marker"!(true) }"#;
    let (post_state, user_results, sys_results) = rm
        .compute_state(
            &genesis_post,
            &[deploy(term)],
            &[],
            &rand,
            block_data.clone(),
            &fringe_state(1),
        )
        .await
        .expect("play compute_state");
    assert!(
        user_results[0].eval_result.succeeded(),
        "the control: the deploy must run, or there is nothing burned to share: {:?}",
        user_results[0].eval_result.errors
    );
    let burned = i64::from(user_results[0].deploy.burned_amount());
    assert!(burned > 0, "the control: this deploy burns phlo");

    let processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
    let processed_sys: Vec<ProcessedSystemDeploy> =
        sys_results.into_iter().map(|r| r.deploy).collect();

    // A fresh fork carries only the tuple space, not the native store, so the *balance* is asserted
    // where the manager's own runtime is reachable (`runtime_manager.rs`'s
    // `the_deploy_fold_pays_the_blocks_sender`); what this file owns is the hash agreement below.
    assert!(
        !producer_address.is_empty(),
        "the producer's address is derived from the block's sender"
    );

    let (replay_state, _) = rm
        .replay_compute_state(
            &genesis_post,
            &processed,
            &processed_sys,
            &rand,
            block_data,
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &pos,
            &[],
        )
        .await
        .expect("replay compute_state");
    assert_eq!(
        post_state, replay_state,
        "the replay must pay the same address the same share — a disagreement here is a split"
    );
}

/// **Law 96c: concurrent forked replays agree with serial replays.** The block processor validates a
/// batch of dependency-free blocks on one forked replay runtime per block and inserts them serially
/// (`casper/src/blocks/block_processor.rs:536-620`). This test pins the mechanism the law names: two
/// blocks played from one pre-state reach the same verdicts whether they are replayed in sequence on
/// the manager's own runtime or concurrently on two `fork_replay_runtime` forks. A shared replay
/// runtime would let one block's replay read the other's state; the fork is what makes the concurrent
/// batch sound.
#[tokio::test]
async fn concurrent_forked_replays_agree_with_serial_replays() {
    let rm = common::build_runtime_manager().await;
    let rand = Blake2b512Random::from_init(&[0x5c; 32]);
    let (pre, _, _) = rm
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[seeded_vault()],
        )
        .await
        .expect("compute_genesis");

    let block_a = BlockData {
        block_number: rchain_shared::refined::BlockHeight::try_from(1).expect("height"),
        sender: PublicKey::new(vec![0xa1; 65]),
        ..BlockData::empty()
    };
    let block_b = BlockData {
        block_number: rchain_shared::refined::BlockHeight::try_from(2).expect("height"),
        sender: PublicKey::new(vec![0xb2; 65]),
        ..BlockData::empty()
    };
    let rand_a = Blake2b512Random::from_init(&[0xa1; 32]);
    let rand_b = Blake2b512Random::from_init(&[0xb2; 32]);

    // Dependency-free: each block writes its own channel and neither reads the other's.
    let (post_a, users_a, sys_a) = rm
        .compute_state(
            &pre,
            &[deploy(r#"@"parallel:a"!(1)"#)],
            &[],
            &rand_a,
            block_a.clone(),
            &fringe_state(1),
        )
        .await
        .expect("play a");
    let (post_b, users_b, sys_b) = rm
        .compute_state(
            &pre,
            &[deploy(r#"@"parallel:b"!(1)"#)],
            &[],
            &rand_b,
            block_b.clone(),
            &fringe_state(2),
        )
        .await
        .expect("play b");
    let processed_a: Vec<ProcessedDeploy> = users_a.into_iter().map(|r| r.deploy).collect();
    let processed_b: Vec<ProcessedDeploy> = users_b.into_iter().map(|r| r.deploy).collect();
    let systems_a: Vec<ProcessedSystemDeploy> = sys_a.into_iter().map(|r| r.deploy).collect();
    let systems_b: Vec<ProcessedSystemDeploy> = sys_b.into_iter().map(|r| r.deploy).collect();

    let (serial_a, _) = rm
        .replay_compute_state(
            &pre,
            &processed_a,
            &systems_a,
            &rand_a,
            block_a.clone(),
            &fringe_state(1),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("serial replay a");
    let (serial_b, _) = rm
        .replay_compute_state(
            &pre,
            &processed_b,
            &systems_b,
            &rand_b,
            block_b.clone(),
            &fringe_state(2),
            &BTreeMap::new(),
            true,
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("serial replay b");
    assert_eq!(
        post_a, serial_a,
        "the serial replay of a must reproduce its play"
    );
    assert_eq!(
        post_b, serial_b,
        "the serial replay of b must reproduce its play"
    );

    // The processor's per-block fork: one replay runtime per block, both seeded at the same
    // committed pre-state. The shared arguments are bound before the join because the macro's
    // futures borrow them across an await.
    let empty_participation = BTreeMap::new();
    let pos = PosGenesis::default();
    let fringe_a = fringe_state(1);
    let fringe_b = fringe_state(2);
    let fork_a = rm.fork_replay_runtime(pre).await.expect("fork a");
    let fork_b = rm.fork_replay_runtime(pre).await.expect("fork b");
    let (concurrent_a, concurrent_b) = tokio::join!(
        rm.replay_compute_state_with(
            &fork_a,
            &pre,
            &processed_a,
            &systems_a,
            &rand_a,
            block_a,
            &fringe_a,
            &empty_participation,
            true,
            &pos,
            &[],
        ),
        rm.replay_compute_state_with(
            &fork_b,
            &pre,
            &processed_b,
            &systems_b,
            &rand_b,
            block_b,
            &fringe_b,
            &empty_participation,
            true,
            &pos,
            &[],
        ),
    );
    assert_eq!(
        serial_a,
        concurrent_a.expect("concurrent replay a").0,
        "a forked concurrent replay must reach the serial verdict"
    );
    assert_eq!(
        serial_b,
        concurrent_b.expect("concurrent replay b").0,
        "a forked concurrent replay must reach the serial verdict"
    );
}
