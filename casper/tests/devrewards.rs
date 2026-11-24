//! **The DevRewards service, installed as an ordinary post-genesis deploy** (#655).
//!
//! `examples/devrewards/` is the source: `install.rho` wires a beneficiary registry, a treasury
//! holding a fresh unforgeable vault, and a claim-against-a-root settlement. This suite renders the
//! install with a test attestor key, deploys it, and drives the flows a developer and a signer would.
//!
//! Every evaluate gets its own seed: `new`-bound names derive from it, so a shared seed makes later
//! deploys collide with earlier channels (see `casper/tests/developer_reserve.rs::seed`).

mod common;

use rchain_casper::genesis::contracts::{ProofOfStake, Registry, Vault};
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_models::sorted::SortedProc;
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::base16;
use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};

use common::{build_runtime_manager, fringe_state};

const OWNER: u8 = 0x31;
const ATTESTOR: u8 = 0x41;

const INSTALL_SRC: &str = include_str!("../../examples/devrewards/install.rho");
const REGISTRY_SRC: &str = include_str!("../../examples/devrewards/BeneficiaryRegistry.rho");
const TREASURY_SRC: &str = include_str!("../../examples/devrewards/Treasury.rho");
const SETTLEMENT_SRC: &str = include_str!("../../examples/devrewards/Settlement.rho");

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

fn hex_of(pk: &PublicKey) -> String {
    base16::encode(pk.bytes())
}

/// The install term, with the three fragments spliced in and the attestor set rendered. Mirrors
/// `tools/devrewards_install.py`; the markers are the contract between them.
fn render_install(attestors: &[PublicKey], threshold: usize) -> String {
    let mut term = INSTALL_SRC.to_string();
    for (marker, src) in [
        ("// {{BENEFICIARY_REGISTRY}}", REGISTRY_SRC),
        ("// {{TREASURY}}", TREASURY_SRC),
        ("// {{SETTLEMENT}}", SETTLEMENT_SRC),
    ] {
        assert!(term.contains(marker), "install.rho is missing {marker}");
        term = term.replacen(marker, &format!("{}\n|", src.trim_end()), 1);
    }
    let keys = attestors
        .iter()
        .map(|pk| format!("  \"{}\".hexToBytes()", hex_of(pk)))
        .collect::<Vec<_>>()
        .join(",\n");
    term = term.replace("$$ATTESTORS$$", &format!("[\n{keys}\n]"));
    term = term.replace("$$THRESHOLD$$", &threshold.to_string());
    assert!(!term.contains("$$") && !term.contains("{{"));
    term
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

async fn read(rm: &rchain_casper::runtime_manager::RuntimeManager, channel: &str) -> String {
    let ch = SortedProc::new(rchain_models::par_ops::from_expr(
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

#[test]
fn the_devrewards_install_publishes_its_three_contracts_and_the_treasury_address() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let pos = proof_of_stake();
        let (_, genesis_post, _) = rm
            .compute_genesis(
                &terms(&pos),
                &seed(7),
                BlockData::empty(),
                &rchain_casper::genesis::build_pos_genesis(&pos, None),
                &[funded(OWNER), funded(ATTESTOR)],
            )
            .await
            .expect("a plain genesis");

        let install = render_install(&[public_key(ATTESTOR)], 1);
        let (_, results, _) = rm
            .compute_state(
                &genesis_post,
                &[deploy_by(&install, OWNER)],
                &[],
                &seed(1),
                block(1),
                &fringe_state(1),
            )
            .await
            .expect("the install deploy runs");
        let result = results.last().expect("one deploy");
        assert!(
            result.eval_result.succeeded(),
            "the install must run: {:?}",
            result.eval_result.errors
        );

        for channel in [
            "devrewards/registry",
            "devrewards/treasury",
            "devrewards/settlement",
            "devrewards/treasury-address",
        ] {
            let value = read(&rm, channel).await;
            assert!(
                !value.trim().is_empty() && value != "[]",
                "{channel} must be published: {value}"
            );
        }
        let address = read(&rm, "devrewards/treasury-address").await;
        assert!(
            address.contains("GString"),
            "the treasury address is a string: {address}"
        );
        let error = read(&rm, "devrewards/error").await;
        assert_eq!(error.trim(), "[]", "the install reported an error: {error}");
    });
}
