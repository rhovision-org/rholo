//! **The DevRewards service flows** (#655): registry versioning, activation, claim-once, conflicts,
//! and a retryable under-funded claim. The install itself is exercised in `devrewards.rs`.
//!
//! Every evaluate gets its own seed: `new`-bound names derive from it, so a shared seed makes later
//! deploys collide with earlier channels (see `casper/tests/developer_reserve.rs::seed`).

mod common;

use std::collections::BTreeMap;

use rchain_casper::genesis::contracts::{ProofOfStake, Registry, Vault};
use rchain_crypto::hash::blake2b256;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
use rchain_models::casper::protocol::casper_message::{
    DeployData, ProcessedDeploy, ProcessedSystemDeploy, SignedDeployData,
};
use rchain_models::sorted::SortedProc;
use rchain_rholang::native_state::NativeSystemState;
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::base16;
use rchain_shared::refined::{BlockHeight, NonNegI64, SeqNum};

use common::{build_runtime_manager, fringe_state};

const OWNER: u8 = 0x31;
const OTHER: u8 = 0x32;
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

fn hx(bytes: &[u8]) -> String {
    base16::encode(bytes)
}

fn address_of(pk: &PublicKey) -> String {
    RevAddress::from_public_key(pk)
        .expect("a secp256k1 key has a REV address")
        .to_base58()
}

/// Mirrors `tools/devrewards_install.py`; the marker strings are the contract between them.
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

fn vaults() -> Vec<Vault> {
    vec![funded(OWNER), funded(ATTESTOR), funded(OTHER)]
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

async fn read_string(rm: &rchain_casper::runtime_manager::RuntimeManager, channel: &str) -> String {
    let text = read(rm, channel).await;
    let start = text
        .find("GString(\"")
        .unwrap_or_else(|| panic!("{channel} holds no GString: {text}"))
        + "GString(\"".len();
    let rest = &text[start..];
    let end = rest
        .find("\")")
        .unwrap_or_else(|| panic!("{channel} has an unterminated GString: {text}"));
    rest[..end].to_string()
}

async fn vault_balance(rm: &rchain_casper::runtime_manager::RuntimeManager, address: &str) -> i64 {
    NativeSystemState::new(rm.runtime().native_store())
        .vault_balance(address)
        .await
        .expect("read a balance")
        .map(i64::from)
        .unwrap_or(0)
}

async fn step_by(
    rm: &rchain_casper::runtime_manager::RuntimeManager,
    pre: &Blake2b256Hash,
    term: &str,
    n: u8,
    deployer: u8,
) -> Blake2b256Hash {
    let (post, results, _) = rm
        .compute_state(
            pre,
            &[deploy_by(term, deployer)],
            &[],
            &seed(n),
            block(i64::from(n)),
            &fringe_state(n),
        )
        .await
        .expect("the step deploy runs");
    assert!(
        results[0].eval_result.succeeded(),
        "step {n} must run: {:?}",
        results[0].eval_result.errors
    );
    post
}

async fn step(
    rm: &rchain_casper::runtime_manager::RuntimeManager,
    pre: &Blake2b256Hash,
    term: &str,
    n: u8,
) -> Blake2b256Hash {
    step_by(rm, pre, term, n, OWNER).await
}

async fn installed(rm: &rchain_casper::runtime_manager::RuntimeManager) -> Blake2b256Hash {
    let pos = proof_of_stake();
    let pos_genesis = rchain_casper::genesis::build_pos_genesis(&pos, None);
    let (_, genesis_post, _) = rm
        .compute_genesis(
            &terms(&pos),
            &seed(7),
            BlockData::empty(),
            &pos_genesis,
            &vaults(),
        )
        .await
        .expect("a plain genesis");
    step(
        rm,
        &genesis_post,
        &render_install(&[public_key(ATTESTOR)], 1),
        1,
    )
    .await
}

// --- the encoding, mirrored from Settlement.rho --------------------------------------------------

/// `blake2b256(0x00 || key || amount_i64_le)`.
fn leaf(key: &[u8], amount: i64) -> Vec<u8> {
    let mut data = vec![0u8];
    data.extend_from_slice(key);
    data.extend_from_slice(&(amount as u64).to_le_bytes());
    blake2b256::hash(&data)
}

fn manifest_digest(epoch: u64, anchor: &[u8], root: &[u8]) -> Vec<u8> {
    let mut data = b"devrewards-manifest-v1".to_vec();
    data.extend_from_slice(&epoch.to_le_bytes());
    data.extend_from_slice(anchor);
    data.extend_from_slice(root);
    blake2b256::hash(&data)
}

fn activation_digest(address: &str) -> Vec<u8> {
    let mut data = b"devrewards-activate-v1".to_vec();
    data.extend_from_slice(address.as_bytes());
    blake2b256::hash(&data)
}

fn attestation_by(seed: u8, digest: &[u8]) -> String {
    let sig = Secp256k1
        .sign(digest, PrivateKey::new(vec![seed; 32]).bytes())
        .expect("the test attestor signs");
    format!(
        "(\"{}\".hexToBytes(), \"{}\".hexToBytes())",
        hex_of(&public_key(seed)),
        hx(&sig)
    )
}

fn attestation(digest: &[u8]) -> String {
    attestation_by(ATTESTOR, digest)
}

// --- the calls ----------------------------------------------------------------------------------

fn register_term(beneficiary: &str, out: &str) -> String {
    format!(
        r#"new ret, deployerId(`rho:rchain:deployerId`) in {{
  for (r <<- @"devrewards/registry") {{
    r!("register", *deployerId, "{beneficiary}", *ret) |
    for (@ok <- ret) {{ @"{out}"!(ok) }}
  }}
}}"#
    )
}

fn beneficiary_term(key: &[u8], as_of: i64, out: &str) -> String {
    format!(
        r#"new ret in {{
  for (r <<- @"devrewards/registry") {{
    r!("beneficiaryOf", "{key}".hexToBytes(), {as_of}, *ret) |
    for (@b <- ret) {{ @"{out}"!(b) }}
  }}
}}"#,
        key = hx(key)
    )
}

fn activate_term(signature: &str, out: &str) -> String {
    format!(
        r#"new ret in {{
  for (t <<- @"devrewards/treasury") {{
    t!("activate", [{signature}], *ret) |
    for (@r <- ret) {{ @"{out}"!(r) }}
  }}
}}"#
    )
}

fn accept_term(
    epoch: i64,
    anchor_height: i64,
    anchor: &[u8],
    root: &[u8],
    total: i64,
    signature: &str,
    out: &str,
) -> String {
    format!(
        r#"new ret in {{
  for (s <<- @"devrewards/settlement") {{
    s!("accept", {epoch}, {anchor_height}, "{anchor}".hexToBytes(), "{root}".hexToBytes(), {total}, [{signature}], *ret) |
    for (@r <- ret) {{ @"{out}"!(r) }}
  }}
}}"#,
        anchor = hx(anchor),
        root = hx(root)
    )
}

fn claim_term(epoch: i64, key: &[u8], amount: i64, proof: &str, out: &str) -> String {
    format!(
        r#"new ret in {{
  for (s <<- @"devrewards/settlement") {{
    s!("claim", {epoch}, "{key}".hexToBytes(), {amount}, {proof}, *ret) |
    for (@r <- ret) {{ @"{out}"!(r) }}
  }}
}}"#,
        key = hx(key)
    )
}

fn conflicts_term(out: &str) -> String {
    format!(
        r#"new ret in {{
  for (s <<- @"devrewards/settlement") {{
    s!("conflicts", *ret) |
    for (@r <- ret) {{ @"{out}"!(r) }}
  }}
}}"#
    )
}

fn fund_treasury_term(address: &str, amount: i64, out: &str) -> String {
    format!(
        r#"new rv(`rho:rchain:revVault`), deployerId(`rho:rchain:deployerId`), ch in {{
  rv!("transfer", *deployerId, "{address}", {amount}, *ch) |
  for (@r <- ch) {{ @"{out}"!(r) }}
}}"#
    )
}

fn owner_key() -> Vec<u8> {
    public_key(OWNER).bytes().to_vec()
}

fn other_key() -> Vec<u8> {
    public_key(OTHER).bytes().to_vec()
}

// --- tests --------------------------------------------------------------------------------------

/// **The registry is self-scoped and anchor-versioned.** A beneficiary change applies at its block
/// and never before it; another deployer's registration cannot touch this key.
#[test]
fn the_beneficiary_registry_is_self_scoped_and_anchor_versioned() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let mut post = installed(&rm).await;
        let t1 = address_of(&public_key(0x21));
        let t2 = address_of(&public_key(0x22));
        let t3 = address_of(&public_key(0x23));
        let default = address_of(&public_key(OWNER));

        post = step(&rm, &post, &register_term(&t1, "reg-ok-1"), 2).await;
        assert!(read(&rm, "reg-ok-1").await.contains("GBool(true)"));
        post = step(
            &rm,
            &post,
            &beneficiary_term(&owner_key(), 2, "beneficiary-at-2"),
            3,
        )
        .await;
        assert_eq!(read_string(&rm, "beneficiary-at-2").await, t1);
        post = step(
            &rm,
            &post,
            &beneficiary_term(&owner_key(), 1, "beneficiary-at-1"),
            4,
        )
        .await;
        assert_eq!(
            read_string(&rm, "beneficiary-at-1").await,
            default,
            "height 1 predates the registration"
        );

        post = step(&rm, &post, &register_term(&t2, "reg-ok-2"), 5).await;
        post = step(
            &rm,
            &post,
            &beneficiary_term(&owner_key(), 2, "beneficiary-at-2b"),
            6,
        )
        .await;
        assert_eq!(
            read_string(&rm, "beneficiary-at-2b").await,
            t1,
            "a change at height 5 is not retroactive"
        );
        post = step(
            &rm,
            &post,
            &beneficiary_term(&owner_key(), 5, "beneficiary-at-5"),
            7,
        )
        .await;
        assert_eq!(read_string(&rm, "beneficiary-at-5").await, t2);

        post = step_by(&rm, &post, &register_term(&t3, "reg-ok-3"), 8, OTHER).await;
        post = step(
            &rm,
            &post,
            &beneficiary_term(&owner_key(), 8, "beneficiary-other"),
            9,
        )
        .await;
        assert_eq!(
            read_string(&rm, "beneficiary-other").await,
            t2,
            "a different deployer's registration must not change the owner's beneficiary"
        );
        let _ = step(
            &rm,
            &post,
            &beneficiary_term(&other_key(), 8, "beneficiary-other-own"),
            10,
        )
        .await;
        assert_eq!(read_string(&rm, "beneficiary-other-own").await, t3);
    });
}

/// **Claim against the root, once.** The treasury holds while inactive; a valid manifest pays the
/// beneficiary and marks the claim; a replayed claim is refused; a second manifest for the same
/// epoch is refused and recorded; the paying block replays at the same cost and state.
#[test]
fn the_settlement_pays_a_claim_once_and_refuses_a_conflicting_manifest() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let post1 = installed(&rm).await;
        let pos_genesis = rchain_casper::genesis::build_pos_genesis(&proof_of_stake(), None);
        let treasury = read_string(&rm, "devrewards/treasury-address").await;
        assert!(!treasury.is_empty(), "the install publishes the address");

        let payee = address_of(&public_key(0x21));
        let amount = 500_000i64;
        let mut post = step(
            &rm,
            &post1,
            &fund_treasury_term(&treasury, 1_000_000, "funded"),
            2,
        )
        .await;
        post = step(&rm, &post, &register_term(&payee, "reg-ok"), 3).await;

        let anchor = blake2b256::hash(b"anchor-hash");
        let root = leaf(&owner_key(), amount);
        let digest = manifest_digest(1, &anchor, &root);
        post = step(
            &rm,
            &post,
            &accept_term(
                1,
                3,
                &anchor,
                &root,
                amount,
                &attestation(&digest),
                "accepted",
            ),
            4,
        )
        .await;
        let accepted = read(&rm, "accepted").await;
        assert!(
            accepted.contains("accepted"),
            "the accept reply: {accepted}"
        );

        post = step(
            &rm,
            &post,
            &claim_term(1, &owner_key(), amount, "[]", "claim-inactive"),
            5,
        )
        .await;
        assert!(
            read(&rm, "claim-inactive")
                .await
                .contains("treasury inactive"),
            "an inactive treasury holds the reserve: {}",
            read(&rm, "claim-inactive").await
        );
        assert_eq!(vault_balance(&rm, &payee).await, 0);

        let act = activation_digest(&treasury);
        post = step(
            &rm,
            &post,
            &activate_term(&attestation(&act), "activated"),
            6,
        )
        .await;
        assert!(read(&rm, "activated").await.contains("active"));

        let claim = claim_term(1, &owner_key(), amount, "[]", "claim-paid");
        let (post7, results7, sys7) = rm
            .compute_state(
                &post,
                &[deploy_by(&claim, OWNER)],
                &[],
                &seed(7),
                block(7),
                &fringe_state(7),
            )
            .await
            .expect("the claim deploy runs");
        assert!(results7[0].eval_result.succeeded());
        assert!(
            read(&rm, "claim-paid").await.contains("paid"),
            "the claim is paid: {}",
            read(&rm, "claim-paid").await
        );
        assert_eq!(vault_balance(&rm, &payee).await, amount);

        let (post8, _, _) = rm
            .compute_state(
                &post7,
                &[deploy_by(
                    &claim_term(1, &owner_key(), amount, "[]", "claim-replay"),
                    OWNER,
                )],
                &[],
                &seed(8),
                block(8),
                &fringe_state(8),
            )
            .await
            .expect("the replayed claim runs");
        assert!(
            read(&rm, "claim-replay").await.contains("already paid"),
            "a replayed claim is refused: {}",
            read(&rm, "claim-replay").await
        );
        assert_eq!(vault_balance(&rm, &payee).await, amount);

        let other_root = leaf(&owner_key(), amount + 1);
        let other_digest = manifest_digest(1, &anchor, &other_root);
        let post9 = step(
            &rm,
            &post8,
            &accept_term(
                1,
                3,
                &anchor,
                &other_root,
                amount + 1,
                &attestation(&other_digest),
                "conflicting",
            ),
            9,
        )
        .await;
        assert!(
            read(&rm, "conflicting")
                .await
                .contains("conflicting manifest"),
            "a second manifest for the epoch is refused: {}",
            read(&rm, "conflicting").await
        );
        let post10 = step(&rm, &post9, &conflicts_term("conflicts"), 10).await;
        let conflicts = read(&rm, "conflicts").await;
        assert!(
            !conflicts.contains("exprs: []"),
            "the conflicting manifest is recorded: {conflicts}"
        );

        // The same manifest again is idempotent, not a conflict.
        let post11 = step(
            &rm,
            &post10,
            &accept_term(
                1,
                3,
                &anchor,
                &root,
                amount,
                &attestation(&digest),
                "reaccepted",
            ),
            11,
        )
        .await;
        assert!(
            read(&rm, "reaccepted").await.contains("already accepted"),
            "the same digest is accepted once: {}",
            read(&rm, "reaccepted").await
        );

        // An over-claim does not fold to the stored root and pays nothing.
        let post12 = step(
            &rm,
            &post11,
            &claim_term(1, &owner_key(), amount + 1, "[]", "overclaim"),
            12,
        )
        .await;
        assert!(
            read(&rm, "overclaim").await.contains("invalid proof"),
            "an amount with no leaf in the root is refused: {}",
            read(&rm, "overclaim").await
        );
        assert_eq!(vault_balance(&rm, &payee).await, amount);

        // A claim for an epoch nobody accepted is refused before any authority is consulted.
        let post13 = step(
            &rm,
            &post12,
            &claim_term(99, &owner_key(), amount, "[]", "unknown-epoch"),
            13,
        )
        .await;
        assert!(
            read(&rm, "unknown-epoch").await.contains("unknown epoch"),
            "an unaccepted epoch is refused: {}",
            read(&rm, "unknown-epoch").await
        );
        let _ = post13;

        let processed: Vec<ProcessedDeploy> = results7.into_iter().map(|r| r.deploy).collect();
        let sys: Vec<ProcessedSystemDeploy> = sys7.into_iter().map(|r| r.deploy).collect();
        let (replayed, _) = rm
            .replay_compute_state(
                &post,
                &processed,
                &sys,
                &seed(7),
                block(7),
                &fringe_state(7),
                &BTreeMap::new(),
                true,
                &pos_genesis,
                &vaults(),
            )
            .await
            .expect("the paying claim replays at the same cost");
        assert_eq!(post7, replayed, "the claim block replays to the same state");
    });
}

/// **A claim that cannot be paid is not marked, so it can be retried.** The treasury is activated
/// with an empty vault; the claim is refused; funding it and claiming again pays exactly once.
#[test]
fn an_underfunded_claim_is_refused_and_can_be_retried() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let post1 = installed(&rm).await;
        let treasury = read_string(&rm, "devrewards/treasury-address").await;
        let payee = address_of(&public_key(0x21));
        let amount = 100_000i64;

        let mut post = step(&rm, &post1, &register_term(&payee, "reg-ok"), 2).await;
        let anchor = blake2b256::hash(b"underfunded-anchor");
        let root = leaf(&owner_key(), amount);
        let digest = manifest_digest(2, &anchor, &root);
        post = step(
            &rm,
            &post,
            &accept_term(
                2,
                2,
                &anchor,
                &root,
                amount,
                &attestation(&digest),
                "accepted",
            ),
            3,
        )
        .await;
        let act = activation_digest(&treasury);
        post = step(
            &rm,
            &post,
            &activate_term(&attestation(&act), "activated"),
            4,
        )
        .await;

        post = step(
            &rm,
            &post,
            &claim_term(2, &owner_key(), amount, "[]", "claim-empty"),
            5,
        )
        .await;
        assert!(
            read(&rm, "claim-empty").await.contains("GBool(false)"),
            "an empty treasury refuses the claim: {}",
            read(&rm, "claim-empty").await
        );
        assert_eq!(vault_balance(&rm, &payee).await, 0);

        post = step(
            &rm,
            &post,
            &fund_treasury_term(&treasury, 1_000_000, "funded"),
            6,
        )
        .await;
        let _ = step(
            &rm,
            &post,
            &claim_term(2, &owner_key(), amount, "[]", "claim-retry"),
            7,
        )
        .await;
        assert!(
            read(&rm, "claim-retry").await.contains("paid"),
            "the retried claim pays: {}",
            read(&rm, "claim-retry").await
        );
        assert_eq!(vault_balance(&rm, &payee).await, amount);
    });
}

/// **A replayed attestation counts once.** The threshold counts distinct signers, not list entries.
#[test]
fn a_replayed_attestation_counts_once() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let second = 0x42u8;
        let pos = proof_of_stake();
        let pos_genesis = rchain_casper::genesis::build_pos_genesis(&pos, None);
        let (_, genesis_post, _) = rm
            .compute_genesis(
                &terms(&pos),
                &seed(7),
                BlockData::empty(),
                &pos_genesis,
                &vaults(),
            )
            .await
            .expect("a plain genesis");

        let install = render_install(&[public_key(ATTESTOR), public_key(second)], 2);
        let mut post = step(&rm, &genesis_post, &install, 1).await;
        let treasury = read_string(&rm, "devrewards/treasury-address").await;
        let digest = activation_digest(&treasury);
        let one = attestation(&digest);

        // The same signature twice is one signer, below the threshold of two.
        let replayed = format!("{one}, {one}");
        post = step(&rm, &post, &activate_term(&replayed, "replayed"), 2).await;
        assert!(
            read(&rm, "replayed")
                .await
                .contains("insufficient attestations"),
            "a replayed attestation must not count twice: {}",
            read(&rm, "replayed").await
        );

        // Two distinct attestors meet it.
        let distinct = format!(
            "{}, {}",
            attestation(&digest),
            attestation_by(second, &digest)
        );
        let _ = step(&rm, &post, &activate_term(&distinct, "distinct"), 3).await;
        assert!(
            read(&rm, "distinct").await.contains("active"),
            "two distinct attestors activate the treasury: {}",
            read(&rm, "distinct").await
        );
    });
}
