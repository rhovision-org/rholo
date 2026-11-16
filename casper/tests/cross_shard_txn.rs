//! End-to-end cross-shard two-phase-commit test (Laws 26–29).
//!
//! Drives the client-side [`TxnCoordinator`] across two in-process shards (two runtime managers,
//! each a separate tuple space) via a test-double [`DeployService`] that routes a signed deploy to
//! the shard named in `DeployData.shard_id` and answers `listen_for_data_at_name` from that shard's
//! post-state. The per-shard `rho:txn` participant (a native system process) runs the REV escrow;
//! the coordinator's vote collection supplies the all-or-nothing outcome (Law 27).

mod common;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rchain_casper::construct_deploy;
use rchain_casper::genesis::contracts::Vault;
use rchain_casper::protocol::client::DeployService;
use rchain_casper::runtime_manager::RuntimeManager;
use rchain_casper::shard_invoke::{signed_invoke, ShardOutcome};
use rchain_casper::txn_coordinator::{txn_term, TxnCoordinator, TxnLeg};
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_models::casper::protocol::casper_message::{
    ProcessedDeploy, ProcessedSystemDeploy, SignedDeployData,
};
use rchain_models::casper::protocol::deploy_service::{
    BlockQuery, BlocksQuery, BondStatusQuery, ContinuationAtNameQuery, ContinuationsWithBlockInfo,
    DataAtNameQuery, DataWithBlockInfo, DeployExecStatus, FindDeployQuery, IsFinalizedQuery,
    LightBlockInfo, MachineVerifyQuery, VisualizeDagQuery,
};
use rchain_models::normalizer_env::NormalizerEnv;
use rchain_models::rholang::RhoType::{RhoByteArray, RhoDeployId, RhoNumber, RhoString};
use rchain_models::sorted::SortedProc;
use rchain_rholang::native_state::{NativeSystemState, PosGenesis};
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::base16;
use rchain_shared::refined::NonNegI64;

use common::{build_runtime_manager, fringe_state};

fn fixed_rand() -> Blake2b512Random {
    Blake2b512Random::from_init(&[0u8; 32])
}

/// A test-double [`DeployService`]: routes `deploy` to the shard named in the deploy, and answers
/// `listen_for_data_at_name` from the runtime the deploy was submitted to.
struct InProcDeployService {
    managers: HashMap<String, Arc<RuntimeManager>>,
    deployed: Mutex<HashMap<String, Arc<RuntimeManager>>>,
    rand: Blake2b512Random,
}

impl InProcDeployService {
    fn new(rand: Blake2b512Random) -> Self {
        InProcDeployService {
            managers: HashMap::new(),
            deployed: Mutex::new(HashMap::new()),
            rand,
        }
    }

    fn register(&mut self, shard_id: &str, mgr: Arc<RuntimeManager>) {
        self.managers.insert(shard_id.to_string(), mgr);
    }
}

fn light_block() -> LightBlockInfo {
    LightBlockInfo {
        version: 1,
        shard_id: "root".to_string(),
        block_hash: String::new(),
        block_number: 0,
        sender: String::new(),
        seq_num: 0,
        pre_state_hash: String::new(),
        post_state_hash: String::new(),
        justifications: Vec::new(),
        bonds: Vec::new(),
        sig_algorithm: String::new(),
        sig: String::new(),
        block_size: "0".to_string(),
        deploy_count: 0,
        rejected_deploys: Vec::new(),
        timestamp: 0,
    }
}

#[async_trait]
impl DeployService for InProcDeployService {
    async fn deploy(&self, d: &SignedDeployData) -> Result<String, Vec<String>> {
        let sig_hex = base16::encode(&d.sig);
        let mgr = self
            .managers
            .get(&d.data.shard_id)
            .cloned()
            .ok_or_else(|| vec![format!("unknown shard {}", d.data.shard_id)])?;
        let env = NormalizerEnv::new(d);
        let res = mgr
            .runtime()
            .evaluate_with_env(&d.data.term, env.to_env(), &self.rand)
            .await
            .map_err(|e| vec![e.to_string()])?;
        if !res.errors.is_empty() {
            return Err(res.errors.iter().map(|e| e.to_string()).collect());
        }
        self.deployed.lock().unwrap().insert(sig_hex.clone(), mgr);
        Ok(sig_hex)
    }

    async fn listen_for_data_at_name(
        &self,
        q: &DataAtNameQuery,
    ) -> Result<Vec<DataWithBlockInfo>, Vec<String>> {
        let sig = RhoDeployId::unapply(&q.name)
            .ok_or_else(|| vec!["name is not a deploy id".to_string()])?;
        let sig_hex = base16::encode(sig);
        let mgr = self
            .deployed
            .lock()
            .unwrap()
            .get(&sig_hex)
            .cloned()
            .ok_or_else(|| vec![format!("no runtime for deploy {sig_hex}")])?;
        let data = mgr
            .runtime()
            .get_data_par(&SortedProc::new(q.name.clone()))
            .await
            .map_err(|e| vec![e.to_string()])?;
        Ok(vec![DataWithBlockInfo {
            post_block_data: data,
            block: light_block(),
        }])
    }

    async fn deploy_status(&self, _: &FindDeployQuery) -> Result<DeployExecStatus, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
    async fn get_block(&self, _: &BlockQuery) -> Result<String, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
    async fn get_blocks(&self, _: &BlocksQuery) -> Result<String, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
    async fn visualize_dag(&self, _: &VisualizeDagQuery) -> Result<String, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
    async fn machine_verifiable_dag(&self, _: &MachineVerifyQuery) -> Result<String, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
    async fn find_deploy(&self, _: &FindDeployQuery) -> Result<String, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
    async fn listen_for_continuation_at_name(
        &self,
        _: &ContinuationAtNameQuery,
    ) -> Result<Vec<ContinuationsWithBlockInfo>, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
    async fn last_finalized_block(&self) -> Result<String, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
    async fn is_finalized(&self, _: &IsFinalizedQuery) -> Result<String, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
    async fn bond_status(&self, _: &BondStatusQuery) -> Result<String, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
    async fn status(&self) -> Result<String, Vec<String>> {
        Err(vec!["not implemented".to_string()])
    }
}

/// Fund an address's vault on a shard's runtime.
fn fund(rm: &RuntimeManager, address: &str, amount: i64) {
    NativeSystemState::new(rm.runtime().native_store())
        .set_vault_balance(address, NonNegI64::try_from(amount).expect("non-negative"));
}

/// A genesis vault for `address`, funded with `balance`.
fn genesis_vault(address: &str, balance: i64) -> Vault {
    Vault {
        rev_address: RevAddress::parse(address).expect("valid rev address"),
        initial_balance: NonNegI64::try_from(balance).expect("non-negative"),
    }
}

/// Sign `term` into a block-pipeline deploy as `key` (the shape the coordinator submits).
fn signed_deploy(term: &str, key: &PrivateKey, pk: &PublicKey) -> SignedDeployData {
    let signed = signed_invoke(term, key, 0, 1_000_000, 1, 0, "/root").expect("sign deploy");
    SignedDeployData {
        data: signed.data,
        deployer: pk.bytes().to_vec(),
        sig: signed.sig.clone(),
        sig_algorithm: "secp256k1".to_string(),
    }
}

/// Both legs funded and ready ⇒ every shard commits and the escrowed REV moves to the destination.
#[tokio::test]
async fn two_shard_2pc_commits_all() {
    let rand = fixed_rand();
    let (coordinator_sec, coordinator_pub) = construct_deploy::default_key_pair().unwrap();
    let coordinator_addr = RevAddress::from_public_key(&coordinator_pub)
        .unwrap()
        .to_base58();
    // A real REV address: `rho:txn` refuses a destination it cannot decode (C301), so a literal
    // placeholder is no longer a legal fixture.
    let destination = RevAddress::from_public_key(&construct_deploy::default_pub2().unwrap())
        .unwrap()
        .to_base58();

    let shard_a = Arc::new(build_runtime_manager().await);
    let shard_b = Arc::new(build_runtime_manager().await);
    fund(&shard_a, &coordinator_addr, 100);
    fund(&shard_b, &coordinator_addr, 100);

    let mut service = InProcDeployService::new(rand);
    service.register("shard-a", shard_a.clone());
    service.register("shard-b", shard_b.clone());

    let coordinator = TxnCoordinator::new(coordinator_sec, coordinator_pub);
    let legs = vec![
        TxnLeg {
            shard_id: "shard-a".to_string(),
            amount: 30,
            to: destination.clone(),
            // The in-process shards these tests build are genesis-only, so `0` *is* each shard's
            // head here and the leg is anchored correctly. What pins the anchor is the coordinator's
            // own `run_2pc_anchors_each_phase_at_its_legs_height` (AUDIT C166): an anchor at genesis
            // on a chain past `DEPLOY_LIFESPAN` is born expired, silently.
            valid_after_block_number: 0,
        },
        TxnLeg {
            shard_id: "shard-b".to_string(),
            amount: 40,
            to: destination.clone(),
            valid_after_block_number: 0,
        },
    ];
    let outcomes = coordinator
        .run_2pc(&service, b"e2e-txn-commit", &legs)
        .await
        .unwrap();

    // Uniform outcome (Law 27): every phase-two reply is "committed".
    for outcome in &outcomes {
        match outcome {
            ShardOutcome::Value(p) => {
                assert_eq!(RhoString::unapply(p), Some("committed"), "got {p:?}")
            }
            other => panic!("expected committed, got {other:?}"),
        }
    }

    let native_a = NativeSystemState::new(shard_a.runtime().native_store());
    let native_b = NativeSystemState::new(shard_b.runtime().native_store());
    assert_eq!(
        i64::from(native_a.vault_balance(&destination).await.unwrap().unwrap()),
        30
    );
    assert_eq!(
        i64::from(native_b.vault_balance(&destination).await.unwrap().unwrap()),
        40
    );
    assert_eq!(
        i64::from(
            native_a
                .vault_balance(&coordinator_addr)
                .await
                .unwrap()
                .unwrap()
        ),
        70
    );
    assert_eq!(
        i64::from(
            native_b
                .vault_balance(&coordinator_addr)
                .await
                .unwrap()
                .unwrap()
        ),
        60
    );
}

/// One leg underfunded ⇒ the whole transaction aborts: the prepared leg's escrow is returned and no
/// destination balance moves on any shard.
#[tokio::test]
async fn two_shard_2pc_aborts_all_when_a_leg_fails() {
    let rand = fixed_rand();
    let (coordinator_sec, coordinator_pub) = construct_deploy::default_key_pair().unwrap();
    let coordinator_addr = RevAddress::from_public_key(&coordinator_pub)
        .unwrap()
        .to_base58();
    // A real REV address: `rho:txn` refuses a destination it cannot decode (C301), so a literal
    // placeholder is no longer a legal fixture.
    let destination = RevAddress::from_public_key(&construct_deploy::default_pub2().unwrap())
        .unwrap()
        .to_base58();

    let shard_a = Arc::new(build_runtime_manager().await);
    let shard_b = Arc::new(build_runtime_manager().await);
    // shard-b is underfunded: its 40-REV prepare will vote abort.
    fund(&shard_a, &coordinator_addr, 100);
    fund(&shard_b, &coordinator_addr, 10);

    let mut service = InProcDeployService::new(rand);
    service.register("shard-a", shard_a.clone());
    service.register("shard-b", shard_b.clone());

    let coordinator = TxnCoordinator::new(coordinator_sec, coordinator_pub);
    let legs = vec![
        TxnLeg {
            shard_id: "shard-a".to_string(),
            amount: 30,
            to: destination.clone(),
            // The in-process shards these tests build are genesis-only, so `0` *is* each shard's
            // head here and the leg is anchored correctly. What pins the anchor is the coordinator's
            // own `run_2pc_anchors_each_phase_at_its_legs_height` (AUDIT C166): an anchor at genesis
            // on a chain past `DEPLOY_LIFESPAN` is born expired, silently.
            valid_after_block_number: 0,
        },
        TxnLeg {
            shard_id: "shard-b".to_string(),
            amount: 40,
            to: destination.clone(),
            valid_after_block_number: 0,
        },
    ];
    let _outcomes = coordinator
        .run_2pc(&service, b"e2e-txn-abort", &legs)
        .await
        .unwrap();

    // The prepared leg (shard-a) was aborted: its escrow returned, the destination got nothing.
    let native_a = NativeSystemState::new(shard_a.runtime().native_store());
    let native_b = NativeSystemState::new(shard_b.runtime().native_store());
    assert_eq!(
        i64::from(
            native_a
                .vault_balance(&coordinator_addr)
                .await
                .unwrap()
                .unwrap()
        ),
        100
    );
    assert_eq!(
        i64::from(
            native_b
                .vault_balance(&coordinator_addr)
                .await
                .unwrap()
                .unwrap()
        ),
        10
    );
    assert_eq!(native_a.vault_balance(&destination).await.unwrap(), None);
    assert_eq!(native_b.vault_balance(&destination).await.unwrap(), None);
}

/// Law 27 on a **retry**: re-running a transaction whose legs are already committed must not turn
/// it into a mixed outcome.
///
/// A participant is idempotent under `txn_id`, so a second `prepare` on a committed leg replies
/// `"committed"` rather than `"ready"`. Reading that as "not ready" would decide *abort* and
/// compensate the other legs — one shard committed, one aborted. This is the path the gateway's
/// restart recovery makes reachable, so the retry has to be safe.
#[tokio::test]
async fn re_running_a_committed_2pc_stays_uniform() {
    let rand = fixed_rand();
    let (coordinator_sec, coordinator_pub) = construct_deploy::default_key_pair().unwrap();
    let coordinator_addr = RevAddress::from_public_key(&coordinator_pub)
        .unwrap()
        .to_base58();
    // A real REV address: `rho:txn` refuses a destination it cannot decode (C301), so a literal
    // placeholder is no longer a legal fixture.
    let destination = RevAddress::from_public_key(&construct_deploy::default_pub2().unwrap())
        .unwrap()
        .to_base58();

    let shard_a = Arc::new(build_runtime_manager().await);
    let shard_b = Arc::new(build_runtime_manager().await);
    fund(&shard_a, &coordinator_addr, 100);
    fund(&shard_b, &coordinator_addr, 100);

    let mut service = InProcDeployService::new(rand);
    service.register("shard-a", shard_a.clone());
    service.register("shard-b", shard_b.clone());

    let coordinator = TxnCoordinator::new(coordinator_sec, coordinator_pub);
    let legs = vec![
        TxnLeg {
            shard_id: "shard-a".to_string(),
            amount: 30,
            to: destination.clone(),
            // The in-process shards these tests build are genesis-only, so `0` *is* each shard's
            // head here and the leg is anchored correctly. What pins the anchor is the coordinator's
            // own `run_2pc_anchors_each_phase_at_its_legs_height` (AUDIT C166): an anchor at genesis
            // on a chain past `DEPLOY_LIFESPAN` is born expired, silently.
            valid_after_block_number: 0,
        },
        TxnLeg {
            shard_id: "shard-b".to_string(),
            amount: 40,
            to: destination.clone(),
            valid_after_block_number: 0,
        },
    ];
    let txn_id: &[u8] = b"e2e-txn-retry";

    // First run commits both legs.
    let first = coordinator.run_2pc(&service, txn_id, &legs).await.unwrap();
    for outcome in &first {
        match outcome {
            ShardOutcome::Value(p) => assert_eq!(RhoString::unapply(p), Some("committed")),
            other => panic!("expected committed, got {other:?}"),
        }
    }

    // Second run with the same txn id: the legs are already committed, so every leg must still be
    // committed — never compensated.
    let second = coordinator.run_2pc(&service, txn_id, &legs).await.unwrap();
    for outcome in &second {
        match outcome {
            ShardOutcome::Value(p) => assert_eq!(
                RhoString::unapply(p),
                Some("committed"),
                "a retry must not abort an already-committed leg"
            ),
            other => panic!("expected committed on the retry, got {other:?}"),
        }
    }

    // The escrow moved exactly once: 30 and 40 credited, the coordinator debited exactly once.
    let native_a = NativeSystemState::new(shard_a.runtime().native_store());
    let native_b = NativeSystemState::new(shard_b.runtime().native_store());
    assert_eq!(
        i64::from(native_a.vault_balance(&destination).await.unwrap().unwrap()),
        30
    );
    assert_eq!(
        i64::from(native_b.vault_balance(&destination).await.unwrap().unwrap()),
        40
    );
    assert_eq!(
        i64::from(
            native_a
                .vault_balance(&coordinator_addr)
                .await
                .unwrap()
                .unwrap()
        ),
        70
    );
    assert_eq!(
        i64::from(
            native_b
                .vault_balance(&coordinator_addr)
                .await
                .unwrap()
                .unwrap()
        ),
        60
    );
}

/// Law 29 (determinism), on the block path: a `rho:txn` `prepare` + `commit` run through
/// `compute_state` and then replayed through `replay_compute_state` must re-derive the *same*
/// post-state hash. The escrow and the transaction record live in the native mergeable state, which
/// feeds the state hash — so a nondeterministic write on the 2PC path fails here, not in consensus.
#[tokio::test]
async fn two_shard_2pc_replay_rederives_the_post_state_hash() {
    let rand = fixed_rand();
    let (coordinator_sec, coordinator_pub) = construct_deploy::default_key_pair().unwrap();
    let coordinator_addr = RevAddress::from_public_key(&coordinator_pub)
        .unwrap()
        .to_base58();
    // A real REV address: `rho:txn` refuses a destination it cannot decode (C301), so a literal
    // placeholder is no longer a legal fixture.
    let destination = RevAddress::from_public_key(&construct_deploy::default_pub2().unwrap())
        .unwrap()
        .to_base58();
    let txn_id: &[u8] = b"replay-txn";

    let shard = build_runtime_manager().await;
    let (_pre, post, _) = shard
        .compute_genesis(
            &[],
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            // Comfortably above the phase deploys' phlo pre-charge.
            &[genesis_vault(&coordinator_addr, 1_000_000_000)],
        )
        .await
        .expect("compute_genesis");

    // The production phase terms (both phases on one shard), submitted as ordinary signed deploys.
    let prepare = txn_term(
        "prepare",
        txn_id,
        &[
            RhoByteArray::apply(coordinator_pub.bytes().to_vec()),
            RhoNumber::apply(30),
            RhoString::apply(destination.clone()),
        ],
        true,
    )
    .expect("a renderable destination");
    let commit = txn_term("commit", txn_id, &[], true).expect("no arguments to refuse");
    let deploys = [
        signed_deploy(&prepare, &coordinator_sec, &coordinator_pub),
        signed_deploy(&commit, &coordinator_sec, &coordinator_pub),
    ];

    let (post_state, user_results, sys_results) = shard
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
    for (phase, result) in ["prepare", "commit"].iter().zip(&user_results) {
        // `eval_result.succeeded()` alone would pass for a deploy rejected at pre-charge (the
        // reducer never ran), so the rejection flag is the load-bearing half of this check.
        assert!(
            !result.deploy.is_failed,
            "{phase} deploy must be processed, not rejected: {:?}",
            result.deploy.system_deploy_error
        );
        assert!(
            result.eval_result.succeeded(),
            "{phase} deploy must succeed: {:?}",
            result.eval_result.errors
        );
    }

    let processed: Vec<ProcessedDeploy> = user_results.into_iter().map(|r| r.deploy).collect();
    let processed_sys: Vec<ProcessedSystemDeploy> =
        sys_results.into_iter().map(|r| r.deploy).collect();

    let (replay_state, _) = shard
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

    assert_ne!(
        post_state, post,
        "the 2PC phases must actually move state (a rejected deploy would leave the hash unchanged)"
    );
    assert_eq!(
        post_state, replay_state,
        "replay must re-derive the 2PC post-state hash (Law 29)"
    );

    // The replay agreed on a state that actually moved the escrowed REV to the destination.
    let native = NativeSystemState::new(shard.runtime().native_store());
    assert_eq!(
        i64::from(native.vault_balance(&destination).await.unwrap().unwrap()),
        30
    );
}
