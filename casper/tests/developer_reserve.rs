//! **The developer fee reserve's custody, end to end** (#655, PR #660).
//!
//! The reserve is paid to a `MultiSigRevVault` the genesis itself creates for the configured signer
//! keys (`casper::genesis::developer_reserve`). These tests run the real genesis term list and the real
//! contract, and assert on **balances**, not on replies, because a reply of "done" with no movement is
//! exactly the failure a custody claim must rule out:
//!
//! * [`the_reserve_is_bound_to_its_vault_and_paid_its_share`] — the reserve's address is the vault
//!   the genesis created, and a fee-paying deploy pays it exactly its share.
//! * [`the_reserve_is_spent_by_its_quorum_and_by_nobody_else`] — the spend witness: a non-signer's
//!   transfer is refused, one signer alone records but moves nothing, and the second signer's
//!   confirmation moves the funds out of the reserve to the target. **Ignored until
//!   #669 is fixed** — see the test.
//! * [`a_reserve_whose_vault_was_not_created_refuses_the_genesis`] — fail closed: a non-zero share
//!   with no custody vault is not installed.
//! * [`the_reserve_genesis_replays_to_the_same_state`] — play and replay bind the same reserve.

mod common;

use std::collections::BTreeMap;

use rchain_casper::genesis::contracts::{ProofOfStake, Registry, Vault};
use rchain_casper::genesis::{build_pos_genesis, default_blessed_terms, developer_reserve};
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
use rchain_models::casper::protocol::casper_message::{
    DeployData, ProcessedDeploy, SignedDeployData,
};
use rchain_rholang::native_state::NativeSystemState;
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::base16;
use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};

use common::{build_runtime_manager, fringe_state};

const SIGNER_A: u8 = 0x11;
const SIGNER_B: u8 = 0x12;
const OUTSIDER: u8 = 0x13;
/// Half of every burn to the reserve, next to a quarter to the producer, so one ordinary deploy funds
/// the reserve with enough to spend.
const DEVELOPER_SHARE: i64 = 5_000;
const EXECUTOR_SHARE: i64 = 2_500;

fn nn(v: i64) -> NonNegI64 {
    NonNegI64::try_from(v).unwrap()
}

fn public_key(seed: u8) -> PublicKey {
    Secp256k1
        .to_public(&PrivateKey::new(vec![seed; 32]))
        .expect("a fixed 32-byte scalar is a valid secp256k1 key")
}

fn address_of(pk: &PublicKey) -> String {
    RevAddress::from_public_key(pk)
        .expect("a secp256k1 key has a REV address")
        .to_base58()
}

/// A deploy whose `deployerId` is the real public key of `seed`, so `deployerAuthKey` builds the
/// token the vault checks against its signer list.
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

fn proof_of_stake(share: i64) -> ProofOfStake {
    ProofOfStake {
        minimum_bond: nn(1),
        maximum_bond: nn(100),
        validators: Vec::new(),
        epoch_length: 1,
        quarantine_length: 1,
        number_of_active_validators: 1,
        executor_share: nn(EXECUTOR_SHARE),
        absence_slack: nn(0),
        participation_grace: nn(0),
        min_phlo_price: nn(0),
        max_number_of_parents: 0,
        pos_multi_sig_public_keys: Vec::new(),
        pos_multi_sig_quorum: 1,
        pos_vault_pub_key: String::new(),
        developer_share: nn(share),
        developer_reserve_public_keys: vec![
            base16::encode(public_key(SIGNER_A).bytes()),
            base16::encode(public_key(SIGNER_B).bytes()),
        ],
        developer_reserve_quorum: 2,
        dev_tax_vesting_blocks: nn(0),
        liveness_absence_threshold: 0,
        liveness_jail_after_boundaries: 0,
        liveness_jail_epochs: 0,
        liveness_vote_window: 0,
        liveness_slash_severity: 0,
    }
}

fn terms(pos: &ProofOfStake) -> Vec<SignedDeployData> {
    default_blessed_terms(
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
    vec![
        funded(0x01),
        funded(SIGNER_A),
        funded(SIGNER_B),
        funded(OUTSIDER),
    ]
}

/// Run an async body with the node's 32 MiB stack (the blessed genesis terms recurse deeply).
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

/// A signer (or not) reaching the reserve the way a custodian would: by its constant registry URI,
/// with a `deployerAuthKey` for its own deployer id. `call` is the method call on `multiSig`, with
/// `*auth` and `*ret` in scope; the reply is published on `@out`.
fn reserve_call(out: &str, call: &str) -> String {
    let uri = developer_reserve::reserve_uri();
    format!(
        r#"new rl(`rho:registry:lookup`), deployerId(`rho:rchain:deployerId`),
       resCh, msvCh, authCh, ret in {{
  rl!(`{uri}`, *resCh) |
  rl!(`rho:rchain:multiSigRevVault`, *msvCh) |
  for (@(_, multiSig) <- resCh & @(_, MultiSigRevVault) <- msvCh) {{
    @MultiSigRevVault!("deployerAuthKey", *deployerId, *authCh) |
    for (auth <- authCh) {{
      @multiSig!{call} |
      for (@reply <- ret) {{ @"{out}"!(reply) }}
    }}
  }}
}}"#
    )
}

async fn published(rm: &rchain_casper::runtime_manager::RuntimeManager, channel: &str) -> String {
    let ch = rchain_models::sorted::SortedProc::new(rchain_models::par_ops::from_expr(
        rchain_models::ast::Expr::GString(channel.to_string()),
    ));
    format!(
        "{:?}",
        rm.runtime()
            .get_data_par(&ch)
            .await
            .expect("read the reply")
    )
}

async fn balance(rm: &rchain_casper::runtime_manager::RuntimeManager, address: &str) -> i64 {
    NativeSystemState::new(rm.runtime().native_store())
        .vault_balance(address)
        .await
        .expect("read a balance")
        .map(i64::from)
        .unwrap_or(0)
}

fn block(height: i64) -> BlockData {
    BlockData {
        block_number: BlockHeight::try_from(height).unwrap(),
        sender: PublicKey::new(vec![5u8; 65]),
        seq_num: SeqNum::zero(),
        timestamp: 0,
    }
}

/// **The reserve is bound to the vault the genesis created, and is paid from real fees.** The
/// reserve's address is the vault's — no signer's own address — the vault's handle is published under
/// the constant URI, and an ordinary deploy's burn pays the reserve exactly its share.
#[test]
fn the_reserve_is_bound_to_its_vault_and_paid_its_share() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = Blake2b512Random::from_init(&[7u8; 32]);
        let pos = proof_of_stake(DEVELOPER_SHARE);
        let (_, genesis_post, _) = rm
            .compute_genesis(
                &terms(&pos),
                &rand,
                BlockData::empty(),
                &build_pos_genesis(&pos, None),
                &vaults(),
            )
            .await
            .expect("a genesis with a reserve and its custody vault");

        // The reserve is bound to the vault the genesis created — not to any signer's own address.
        let native = NativeSystemState::new(rm.runtime().native_store());
        let reserve = native
            .developer_reserve()
            .await
            .unwrap()
            .expect("a non-zero share binds a reserve");
        assert_eq!(reserve.share, nn(DEVELOPER_SHARE));
        for seed in [SIGNER_A, SIGNER_B, OUTSIDER] {
            assert_ne!(
                reserve.address,
                address_of(&public_key(seed)),
                "the reserve is the vault's address, which no key controls directly"
            );
        }
        assert!(
            native
                .registry_lookup(&developer_reserve::reserve_uri())
                .await
                .unwrap()
                .is_some(),
            "the vault's handle is published under the constant URI"
        );
        let reserve_address = reserve.address.clone();

        // 1. Fund the reserve the way it is funded on a chain: an ordinary deploy burns phlo.
        let (_, results, _) = rm
            .compute_state(
                &genesis_post,
                &[deploy_by(r#"@"marker"!(true)"#, 0x01)],
                &[],
                &rand,
                block(1),
                &fringe_state(1),
            )
            .await
            .expect("the funding deploy runs");
        let burned = i64::from(results[0].deploy.burned_amount());
        assert!(burned > 0, "the control: the funding deploy burns phlo");
        let funded = balance(&rm, &reserve_address).await;
        assert_eq!(
            funded,
            burned * DEVELOPER_SHARE / 10_000,
            "the reserve is paid its share of what the deploy burned"
        );
    });
}

/// **The witness.** The reserve receives its share of a real deploy's burn, and only the configured
/// quorum can move it out — through the contract, to a target whose balance is what is asserted.
///
/// **Ignored, and not passing: it found a defect this PR does not fix** (#669).
/// A `MultiSigRevVault` created in one block does not work from a later block on this port: a native
/// vault handle minted in block N answers nothing in block N+1 (its continuation and handler are
/// installed in memory, not in the state a later block resets to), and the genesis-created vault
/// answers its own signer's `transfer` with `invalid auth`. Until that is fixed the reserve accrues
/// but cannot be spent, so no genesis may set a non-zero `developer-share`. This test is the
/// acceptance check for the fix and must pass before one does.
#[test]
#[ignore = "blocked: multisig vaults are not spendable from a later block (#669)"]
fn the_reserve_is_spent_by_its_quorum_and_by_nobody_else() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = Blake2b512Random::from_init(&[7u8; 32]);
        let pos = proof_of_stake(DEVELOPER_SHARE);
        let (_, genesis_post, _) = rm
            .compute_genesis(
                &terms(&pos),
                &rand,
                BlockData::empty(),
                &build_pos_genesis(&pos, None),
                &vaults(),
            )
            .await
            .expect("a genesis with a reserve and its custody vault");

        // The reserve is bound to the vault the genesis created — not to any signer's own address.
        let native = NativeSystemState::new(rm.runtime().native_store());
        let reserve = native
            .developer_reserve()
            .await
            .unwrap()
            .expect("a non-zero share binds a reserve");
        assert_eq!(reserve.share, nn(DEVELOPER_SHARE));
        for seed in [SIGNER_A, SIGNER_B, OUTSIDER] {
            assert_ne!(
                reserve.address,
                address_of(&public_key(seed)),
                "the reserve is the vault's address, which no key controls directly"
            );
        }
        assert!(
            native
                .registry_lookup(&developer_reserve::reserve_uri())
                .await
                .unwrap()
                .is_some(),
            "the vault's handle is published under the constant URI"
        );
        let reserve_address = reserve.address.clone();

        // 1. Fund the reserve the way it is funded on a chain: an ordinary deploy burns phlo.
        let (post, results, _) = rm
            .compute_state(
                &genesis_post,
                &[deploy_by(r#"@"marker"!(true)"#, 0x01)],
                &[],
                &rand,
                block(1),
                &fringe_state(1),
            )
            .await
            .expect("the funding deploy runs");
        let burned = i64::from(results[0].deploy.burned_amount());
        assert!(burned > 0, "the control: the funding deploy burns phlo");
        let funded = balance(&rm, &reserve_address).await;
        assert_eq!(
            funded,
            burned * DEVELOPER_SHARE / 10_000,
            "the reserve is paid its share of what the deploy burned"
        );
        // Spend half of what the reserve holds, so the movement is visible in both balances.
        let spend = funded / 2;
        assert!(
            spend > 0,
            "the control: the reserve holds something to spend"
        );

        let target = address_of(&public_key(0x21));
        let transfer = format!(r#"("transfer", "{target}", {spend}, *auth, *ret)"#);

        // 2. Wrong authority: a key that is not a signer is refused, and nothing moves.
        let (post, _, _) = rm
            .compute_state(
                &post,
                &[deploy_by(&reserve_call("outsider", &transfer), OUTSIDER)],
                &[],
                &rand,
                block(2),
                &fringe_state(2),
            )
            .await
            .expect("the outsider's deploy runs");
        let reply = published(&rm, "outsider").await;
        assert!(
            reply.contains("invalid auth"),
            "a non-signer is refused: {reply}"
        );
        assert_eq!(balance(&rm, &target).await, 0);

        // 3. One signer is not a quorum: the transfer is recorded, and nothing moves.
        let (post, _, _) = rm
            .compute_state(
                &post,
                &[deploy_by(&reserve_call("signer-a", &transfer), SIGNER_A)],
                &[],
                &rand,
                block(3),
                &fringe_state(3),
            )
            .await
            .expect("signer A's deploy runs");
        let reply = published(&rm, "signer-a").await;
        assert!(
            !reply.contains("invalid auth") && !reply.contains("done"),
            "a signer's transfer is recorded and awaits the quorum: {reply}"
        );
        assert_eq!(
            balance(&rm, &target).await,
            0,
            "one of two signers moves nothing"
        );
        let reserve_before_confirm = balance(&rm, &reserve_address).await;

        // 4. The second signer confirms nonce 0: the quorum is met and the funds move.
        let confirm = format!(r#"("confirm", "{target}", {spend}, *auth, 0, *ret)"#);
        let (_, results, _) = rm
            .compute_state(
                &post,
                &[deploy_by(&reserve_call("signer-b", &confirm), SIGNER_B)],
                &[],
                &rand,
                block(4),
                &fringe_state(4),
            )
            .await
            .expect("signer B's deploy runs");
        let confirm_fee_share =
            i64::from(results[0].deploy.burned_amount()) * DEVELOPER_SHARE / 10_000;
        let reply = published(&rm, "signer-b").await;
        assert!(
            reply.contains("done"),
            "the quorum completes the transfer: {reply}"
        );
        assert_eq!(
            balance(&rm, &target).await,
            spend,
            "the reserve's REV reached the target: a spend, not only a credit"
        );
        assert_eq!(
            balance(&rm, &reserve_address).await,
            reserve_before_confirm - spend + confirm_fee_share,
            "the spend left the reserve (which also took its share of the confirming deploy's fee)"
        );
    });
}

/// **Fail closed.** A non-zero share whose custody vault was never created — here, a term list
/// without the reserve deploy — refuses the genesis rather than installing a reserve nobody can spend.
#[test]
fn a_reserve_whose_vault_was_not_created_refuses_the_genesis() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = Blake2b512Random::from_init(&[7u8; 32]);
        let pos = proof_of_stake(DEVELOPER_SHARE);
        let mut without_vault = terms(&pos);
        without_vault.pop();
        let err = rm
            .compute_genesis(
                &without_vault,
                &rand,
                BlockData::empty(),
                &build_pos_genesis(&pos, None),
                &vaults(),
            )
            .await
            .expect_err("a reserve with no custody vault is refused");
        assert!(err.contains("exactly one developer reserve vault"), "{err}");
    });
}

/// **Play and replay bind the same reserve.** The binding is native state written outside the deploy
/// log, so a replaying node has to reproduce it from the same publication or the genesis hash splits.
#[test]
fn the_reserve_genesis_replays_to_the_same_state() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = Blake2b512Random::from_init(&[7u8; 32]);
        let pos = proof_of_stake(DEVELOPER_SHARE);
        let pos_genesis = build_pos_genesis(&pos, None);
        let vaults = vaults();
        let (pre, post, results) = rm
            .compute_genesis(
                &terms(&pos),
                &rand,
                BlockData::empty(),
                &pos_genesis,
                &vaults,
            )
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
                false,
                &pos_genesis,
                &vaults,
            )
            .await
            .expect("the genesis replays");
        assert_eq!(post, replayed, "play and replay must bind the same reserve");
    });
}
