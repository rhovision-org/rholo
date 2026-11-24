//! **The multi-signature authorisation shape, and the harness trap that hid it** (#655 / #669 RCA).
//!
//! The reserve spend witness failed with `(false, "invalid auth")` at its block-3 signer. That reply
//! proves the factory method, the alias and the transfer contract all fired; it is `AuthKey.check`
//! that returned false. Two things had to be separated:
//!
//! 1. **The shape is sound.** `deployerAuthKey` mints `(*_multiSigRevVault, deployerPubKeyBytes)` and
//!    the vault's signer list is written by `ProofOfStake::public_keys` as `"<hex>".hexToBytes()`
//!    literals. [`auth_key_matches_a_literal_copy_of_the_deployer_key`] checks a token against the
//!    original bytes, a literal copy, and a different tuple.
//! 2. **The witness failure was the test, not the port.** Every `compute_state` in
//!    `developer_reserve.rs` shared one random seed, so block 2's fresh `authKey` contract name was
//!    the same name block 3 minted, and block 3's `check` was answered by block 2's token — a
//!    different shape. [`a_two_key_vault_accepts_a_literal_signer_in_one_block`] shows the same
//!    literal key list works with the contract when the block has its own seed.
//!
//! Keep both: the first pins the shape, the second is the regression control for the seed trap
//! (see `rholang-test-rng-reuse` in the project memory).

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

const SIGNER_A: u8 = 0x11;
const SIGNER_B: u8 = 0x12;

fn nn(v: i64) -> NonNegI64 {
    NonNegI64::try_from(v).unwrap()
}

/// One seed per evaluate: `new` names derive from it, so a shared seed collides across deploys.
fn seed(n: u8) -> Blake2b512Random {
    Blake2b512Random::from_init(&[n; 32])
}

fn public_key(seed: u8) -> PublicKey {
    Secp256k1
        .to_public(&PrivateKey::new(vec![seed; 32]))
        .expect("a fixed 32-byte scalar is a valid secp256k1 key")
}

fn hex_of(seed: u8) -> String {
    base16::encode(public_key(seed).bytes())
}

fn address_of(pk: &PublicKey) -> String {
    RevAddress::from_public_key(pk)
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

async fn published(rm: &rchain_casper::runtime_manager::RuntimeManager, channel: &str) -> String {
    let ch = SortedProc::new(rchain_models::par_ops::from_expr(
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

/// **`AuthKey.check` accepts the literal copy of the deployer key that `ProofOfStake::public_keys`
/// writes.** A false here would make every reserve signer unusable; a false on a different shape
/// would mean the check proves nothing.
#[test]
fn auth_key_matches_a_literal_copy_of_the_deployer_key() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let pos = proof_of_stake();
        let (_, genesis_post, _) = rm
            .compute_genesis(
                &terms(&pos),
                &seed(7),
                BlockData::empty(),
                &rchain_casper::genesis::build_pos_genesis(&pos, None),
                &[funded(SIGNER_A)],
            )
            .await
            .expect("a plain genesis");

        let term = format!(
            r#"new rl(`rho:registry:lookup`), akCh,
       DeployerIdOps(`rho:rchain:deployerId:ops`), deployerId(`rho:rchain:deployerId`),
       unf, pkCh, keyCh, sameCh, literalCh, otherCh in {{
  rl!(`rho:rchain:authKey`, *akCh) |
  DeployerIdOps!("pubKeyBytes", *deployerId, *pkCh) |
  for (@(_, *AuthKey) <- akCh & @pk <- pkCh) {{
    // `(*unf, pk)` is the shape `MultiSigRevVault.deployerAuthKey` builds.
    AuthKey!("make", (*unf, pk), *keyCh) |
    for (key <- keyCh) {{
      AuthKey!("check", *key, (*unf, pk), *sameCh) |
      AuthKey!("check", *key, (*unf, "{literal}".hexToBytes()), *literalCh) |
      AuthKey!("check", *key, (*unf, "ff".hexToBytes()), *otherCh) |
      for (@same <- sameCh & @literal <- literalCh & @other <- otherCh) {{
        @"same-deployer-bytes"!(same) |
        @"literal-copy"!(literal) |
        @"different-shape"!(other)
      }}
    }}
  }}
}}"#,
            literal = hex_of(SIGNER_A)
        );
        rm.compute_state(
            &genesis_post,
            &[deploy_by(&term, SIGNER_A)],
            &[],
            &seed(1),
            block(1),
            &fringe_state(1),
        )
        .await
        .expect("the shape probe runs");

        let same = published(&rm, "same-deployer-bytes").await;
        let literal = published(&rm, "literal-copy").await;
        let other = published(&rm, "different-shape").await;
        assert!(
            same.contains("GBool(true)"),
            "the token must check against the very bytes it was made from: {same}"
        );
        assert!(
            literal.contains("GBool(true)"),
            "the token must check against a literal copy of the same 65-byte key: {literal}"
        );
        assert!(
            other.contains("GBool(false)"),
            "a different shape must be refused, or the check proves nothing: {other}"
        );
    });
}

/// **A two-key vault created with the reserve's literal key list accepts its own signer.** Same
/// contract and same key encoding as the genesis reserve, created and spent in one block with its own
/// seed. This is the control for the witness: the contract path is sound when names do not collide.
#[test]
fn a_two_key_vault_accepts_a_literal_signer_in_one_block() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let pos = proof_of_stake();
        let (_, genesis_post, _) = rm
            .compute_genesis(
                &terms(&pos),
                &seed(7),
                BlockData::empty(),
                &rchain_casper::genesis::build_pos_genesis(&pos, None),
                &[funded(SIGNER_A), funded(SIGNER_B)],
            )
            .await
            .expect("a plain genesis");

        let target = address_of(&public_key(0x21));
        let term = format!(
            r#"new rl(`rho:registry:lookup`), msvCh,
       deployerId(`rho:rchain:deployerId`), mainVault(`rho:rchain:revVault`),
       createCh, fundCh, authCh, transferCh in {{
  rl!(`rho:rchain:multiSigRevVault`, *msvCh) |
  for (@(_, *MultiSigRevVault) <- msvCh) {{
    MultiSigRevVault!("create",
      ["{a}".hexToBytes(), "{b}".hexToBytes()], [], 2, *createCh) |
    for (@created <- createCh) {{
      @"created"!(created) |
      match created {{
        (true, (*multiSig, revAddr, *revVault)) => {{
          mainVault!("transfer", *deployerId, revAddr, 40000000, *fundCh) |
          for (_ <- fundCh) {{
            MultiSigRevVault!("deployerAuthKey", *deployerId, *authCh) |
            for (auth <- authCh) {{
              multiSig!("transfer", "{target}", 30000000, *auth, *transferCh) |
              for (@result <- transferCh) {{ @"reply"!(result) }}
            }}
          }}
        }}
        other => {{ @"bad-created"!(other) }}
      }}
    }}
  }}
}}"#,
            a = hex_of(SIGNER_A),
            b = hex_of(SIGNER_B),
        );
        rm.compute_state(
            &genesis_post,
            &[deploy_by(&term, SIGNER_A)],
            &[],
            &seed(1),
            block(1),
            &fringe_state(1),
        )
        .await
        .expect("the two-key spend deploy runs");

        let created = published(&rm, "created").await;
        let reply = published(&rm, "reply").await;
        assert!(
            created.contains("ETuple") && created.contains("GBool(true)"),
            "create must succeed with the literal key list: {created}"
        );
        assert!(
            !reply.contains("invalid auth"),
            "signer A's own token must authorise the recorded transfer: {reply}"
        );
        assert!(
            reply.contains("GBool(true)") && reply.contains("GInt(0)"),
            "quorum 2, first signer: the transfer is recorded at nonce 0: {reply}"
        );
    });
}
