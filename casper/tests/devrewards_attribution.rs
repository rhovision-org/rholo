//! **Attribution rests on two event-log facts** (#655): two byte-identical contract installs by
//! different deploys log **distinct** consume hashes, and an invocation's COMM resolves to exactly
//! one of them. Without the first, the off-chain linker cannot tell the installs apart; without the
//! second, a burn could be attributed twice. `tools/devrewards_manifest.py` is the consumer.

mod common;

use rchain_casper::genesis::contracts::{ProofOfStake, Registry, Vault};
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
use rchain_models::casper::protocol::casper_message::{
    DeployData, Event, ProcessedDeploy, SignedDeployData,
};
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};

use common::{build_runtime_manager, fringe_state};

const A: u8 = 0x51;
const B: u8 = 0x52;
const C: u8 = 0x53;

fn nn(v: i64) -> NonNegI64 {
    NonNegI64::try_from(v).unwrap()
}

fn seed(n: u8) -> Blake2b512Random {
    Blake2b512Random::from_init(&[n; 32])
}

fn public_key(seed: u8) -> PublicKey {
    Secp256k1
        .to_public(&PrivateKey::new(vec![seed; 32]))
        .expect("a fixed 32-byte scalar is a valid secp256k1 key")
}

fn deploy_by(term: &str, seed: u8) -> SignedDeployData {
    SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: term.to_string(),
            timestamp: 0,
            phlo_price: 1,
            phlo_limit: 5_000_000,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        },
        deployer: public_key(seed).bytes().to_vec(),
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
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

fn funded(seed: u8) -> Vault {
    Vault {
        rev_address: RevAddress::from_public_key(&public_key(seed)).unwrap(),
        initial_balance: nn(1_000_000_000),
    }
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

/// A byte-identical install term: one persistent contract, published on `out`.
fn install_term(out: &str) -> String {
    format!(
        r#"new c in {{
  contract c(@x) = {{ @"called"!(x) }} |
  @"{out}"!(*c)
}}"#
    )
}

fn consume_hashes(pd: &ProcessedDeploy) -> Vec<Vec<u8>> {
    pd.deploy_log
        .iter()
        .filter_map(|event| match event {
            Event::Consume(c) => Some(c.hash.clone()),
            _ => None,
        })
        .collect()
}

fn comm_consume_hashes(pd: &ProcessedDeploy) -> Vec<Vec<u8>> {
    pd.deploy_log
        .iter()
        .filter_map(|event| match event {
            Event::Comm(c) => Some(c.consume.hash.clone()),
            _ => None,
        })
        .collect()
}

/// **Two identical installs have distinct consume hashes, and one invocation resolves to exactly
/// one of them.**
#[test]
fn two_identical_installs_are_distinguishable_and_an_invocation_resolves_to_one() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let pos = proof_of_stake();
        let (_, genesis_post, _) = rm
            .compute_genesis(
                &terms(&pos),
                &seed(7),
                BlockData::empty(),
                &rchain_casper::genesis::build_pos_genesis(&pos, None),
                &[funded(A), funded(B), funded(C)],
            )
            .await
            .expect("a plain genesis");

        let (post1, first, _) = rm
            .compute_state(
                &genesis_post,
                &[deploy_by(&install_term("contract-1"), A)],
                &[],
                &seed(1),
                block(1),
                &fringe_state(1),
            )
            .await
            .expect("install 1");
        let (post2, second, _) = rm
            .compute_state(
                &post1,
                &[deploy_by(&install_term("contract-2"), B)],
                &[],
                &seed(2),
                block(2),
                &fringe_state(2),
            )
            .await
            .expect("install 2");
        let (_, third, _) = rm
            .compute_state(
                &post2,
                &[deploy_by(
                    r#"new ok in { for (c <<- @"contract-1") { c!(1) | @"called"!(true) } }"#,
                    C,
                )],
                &[],
                &seed(3),
                block(3),
                &fringe_state(3),
            )
            .await
            .expect("the invocation");

        assert!(first[0].eval_result.succeeded());
        assert!(second[0].eval_result.succeeded());
        assert!(third[0].eval_result.succeeded());

        let h1 = consume_hashes(&first[0].deploy);
        let h2 = consume_hashes(&second[0].deploy);
        assert!(!h1.is_empty(), "install 1 logs its consume: {h1:?}");
        assert!(!h2.is_empty(), "install 2 logs its consume: {h2:?}");
        assert!(
            h1.iter().all(|h| !h2.contains(h)),
            "the two identical installs must log distinct consume hashes: {h1:?} vs {h2:?}"
        );

        let invoked = comm_consume_hashes(&third[0].deploy);
        assert!(
            !invoked.is_empty(),
            "the invocation logs a COMM: {:?}",
            third[0].deploy.deploy_log
        );
        let resolved = invoked
            .iter()
            .filter(|h| h1.contains(h) || h2.contains(h))
            .count();
        assert_eq!(
            resolved, 1,
            "an invocation resolves to exactly one installation: invoked {invoked:?}, \
             installs {h1:?} / {h2:?}"
        );
        assert!(
            invoked.iter().any(|h| h1.contains(h)),
            "the invocation consumed the first installation: {invoked:?} vs {h1:?}"
        );
        assert!(
            invoked.iter().all(|h| !h2.contains(h)),
            "the invocation must not also resolve to the second installation: {invoked:?} vs {h2:?}"
        );
    });
}
