//! **A minted REV-vault handle survives the block boundary** (#669).
//!
//! `findOrCreate` mints a native handle and installs its continuation. Before #669 that install went
//! into the hot store's non-checkpointed `installed_continuations`, and the handler lived only in the
//! minting block's dispatcher, so block N+1 answered nothing on the same name. The handle is now a
//! checkpointed persistent consume and the dispatcher rebuilds its handler from the durable
//! `PREFIX_VAULT_HANDLE` record. This test mints the handle in block 1 and reads `balance` on the
//! same name in block 2, **and replays both blocks with cost accounting on**: a minted continuation
//! that charges differently on replay would be a consensus split, so state-hash parity alone is not
//! enough.
//!
//! **Every evaluate gets its own seed.** `new`-bound names are derived from it, so one seed across
//! blocks makes a later deploy's fresh channels collide with an earlier deploy's (see
//! `casper/tests/developer_reserve.rs::seed`). Play and replay of the *same* block deliberately share
//! the seed, as a node would.

mod common;

use std::collections::BTreeMap;

use rchain_casper::genesis::build_pos_genesis;
use rchain_casper::genesis::contracts::{ProofOfStake, Registry, Vault};
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
use rchain_models::casper::protocol::casper_message::{
    DeployData, ProcessedDeploy, ProcessedSystemDeploy, SignedDeployData,
};
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};

use common::{build_runtime_manager, fringe_state};

const OWNER: u8 = 0x31;

fn nn(v: i64) -> NonNegI64 {
    NonNegI64::try_from(v).unwrap()
}

/// One seed per evaluate; see the module docs. Play and replay of one block share it.
fn seed(n: u8) -> Blake2b512Random {
    Blake2b512Random::from_init(&[n; 32])
}

fn public_key(seed: u8) -> PublicKey {
    Secp256k1
        .to_public(&PrivateKey::new(vec![seed; 32]))
        .expect("a fixed 32-byte scalar is a valid secp256k1 key")
}

fn address_of(seed: u8) -> String {
    RevAddress::from_public_key(&public_key(seed))
        .expect("a secp256k1 key has a REV address")
        .to_base58()
}

fn deploy_by(term: &str, seed: u8) -> SignedDeployData {
    SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: term.to_string(),
            timestamp: 0,
            phlo_price: 1,
            phlo_limit: 900_000,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        },
        deployer: public_key(seed).bytes().to_vec(),
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
    }
}

fn funded(seed: u8) -> Vault {
    Vault {
        rev_address: RevAddress::from_public_key(&public_key(seed)).unwrap(),
        initial_balance: nn(1_000_000_000),
    }
}

fn ceremony_identity() -> rchain_casper::validator_identity::ValidatorIdentity {
    let sk = PrivateKey::new(vec![7u8; 32]);
    rchain_casper::validator_identity::ValidatorIdentity {
        public_key: Secp256k1.to_public(&sk).unwrap(),
        private_key: sk,
        sig_algorithm: "secp256k1".to_string(),
    }
}

fn proof_of_stake() -> ProofOfStake {
    ProofOfStake {
        minimum_bond: nn(1),
        maximum_bond: nn(100),
        validators: Vec::new(),
        epoch_length: 1,
        quarantine_length: 1,
        number_of_active_validators: 1,
        executor_share: nn(2_500),
        absence_slack: nn(0),
        participation_grace: nn(0),
        min_phlo_price: nn(0),
        max_number_of_parents: 0,
        pos_multi_sig_public_keys: Vec::new(),
        pos_multi_sig_quorum: 1,
        pos_vault_pub_key: String::new(),
        developer_share: nn(0),
        developer_reserve_public_keys: Vec::new(),
        developer_reserve_quorum: 0,
        dev_tax_vesting_blocks: nn(0),
        liveness_absence_threshold: 0,
        liveness_jail_after_boundaries: 0,
        liveness_jail_epochs: 0,
        liveness_vote_window: 0,
        liveness_slash_severity: 0,
    }
}

fn terms(pos: &ProofOfStake) -> Vec<SignedDeployData> {
    rchain_casper::genesis::default_blessed_terms(
        pos,
        &Registry {
            system_contract_pub_key: String::new(),
        },
        &[],
        "root",
        &ceremony_identity(),
    )
    .expect("the blessed term list builds")
}

fn vaults() -> Vec<Vault> {
    vec![funded(OWNER)]
}

fn block(height: i64) -> BlockData {
    BlockData {
        block_number: BlockHeight::try_from(height).unwrap(),
        sender: PublicKey::new(vec![5u8; 65]),
        seq_num: SeqNum::zero(),
        timestamp: 0,
    }
}

fn with_big_stack<F>(body: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    const STACK: usize = 32 * 1024 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .thread_stack_size(STACK)
                .enable_all()
                .build()
                .expect("a tokio runtime")
                .block_on(body)
        })
        .expect("spawn the test thread")
        .join()
        .expect("the test thread panicked");
}

async fn read(rm: &rchain_casper::runtime_manager::RuntimeManager, channel: &str) -> String {
    let ch = rchain_models::sorted::SortedProc::new(rchain_models::par_ops::from_expr(
        rchain_models::ast::Expr::GString(channel.to_string()),
    ));
    format!(
        "{:?}",
        rm.runtime()
            .get_data_par(&ch)
            .await
            .expect("read the channel")
    )
}

/// Mint a handle in block 1; `balance` on the same name in block 2; replay both with cost accounting.
#[test]
fn a_minted_handle_answers_in_a_later_block() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let pos = proof_of_stake();
        let pos_genesis = build_pos_genesis(&pos, None);
        let vaults = vaults();
        let (_, genesis_post, _) = rm
            .compute_genesis(
                &terms(&pos),
                &seed(7),
                BlockData::empty(),
                &pos_genesis,
                &vaults,
            )
            .await
            .expect("a plain genesis");

        let mint = format!(
            r#"new rv(`rho:rchain:revVault`), c in {{
  rv!("findOrCreate", "{owner}", *c) |
  for (@(_, *h) <- c) {{ @"handle"!(*h) }}
}}"#,
            owner = address_of(OWNER)
        );
        let (post_one, one, sys_one) = rm
            .compute_state(
                &genesis_post,
                &[deploy_by(&mint, OWNER)],
                &[],
                &seed(1),
                block(1),
                &fringe_state(1),
            )
            .await
            .expect("the mint deploy runs");

        let use_it = r#"new balance in {
  for (h <- @"handle") {
    h!("balance", *balance) |
    for (@bal <- balance) { @"seen-balance"!(bal) }
  }
}"#;
        let (post_two, two, sys_two) = rm
            .compute_state(
                &post_one,
                &[deploy_by(&use_it, OWNER)],
                &[],
                &seed(2),
                block(2),
                &fringe_state(2),
            )
            .await
            .expect("the second-block deploy runs");

        // The handle answered `balance` in a later block.
        let balance_reply = read(&rm, "seen-balance").await;
        assert!(
            balance_reply.contains("GInt"),
            "the handle must answer balance in block 2: {balance_reply}"
        );

        // **Play and replay must agree on the state and the charge.** A mismatch is returned as
        // `ReplayCostMismatch` when `with_cost_accounting` is true, so the `.expect` is the assertion.
        let one_deploys: Vec<ProcessedDeploy> = one.into_iter().map(|r| r.deploy).collect();
        let one_sys: Vec<ProcessedSystemDeploy> = sys_one.into_iter().map(|r| r.deploy).collect();
        let (replay_one, _) = rm
            .replay_compute_state(
                &genesis_post,
                &one_deploys,
                &one_sys,
                &seed(1),
                block(1),
                &fringe_state(1),
                &BTreeMap::new(),
                true,
                &pos_genesis,
                &vaults,
            )
            .await
            .expect("the mint block replays at the same cost");
        assert_eq!(
            post_one, replay_one,
            "the mint block must replay to the same state"
        );

        let two_deploys: Vec<ProcessedDeploy> = two.into_iter().map(|r| r.deploy).collect();
        let two_sys: Vec<ProcessedSystemDeploy> = sys_two.into_iter().map(|r| r.deploy).collect();
        let (replay_two, _) = rm
            .replay_compute_state(
                &post_one,
                &two_deploys,
                &two_sys,
                &seed(2),
                block(2),
                &fringe_state(2),
                &BTreeMap::new(),
                true,
                &pos_genesis,
                &vaults,
            )
            .await
            .expect("the balance block replays at the same cost");
        assert_eq!(
            post_two, replay_two,
            "the balance block must replay to the same state"
        );
    });
}
