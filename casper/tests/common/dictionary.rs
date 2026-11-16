//! The rooted-name dictionary's test harness: a real-key deploy, the genesis facets, and the ordered
//! probe runner. Shared by `master_dictionary.rs` (the rule's own cases) and `lean_namespace_corpus.rs`
//! (Law 56's conformance layer), so a corpus row and a hand-written probe drive the same contract the
//! same way.

use rchain_casper::genesis::contracts::{ProofOfStake, Registry};
use rchain_casper::genesis::default_blessed_terms;
use rchain_casper::genesis::rgov;
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_models::rholang::RhoType::RhoString;
use rchain_rholang::native_state::PosGenesis;
use rchain_rholang::system_processes::BlockData;
use rchain_shared::refined::NonNegI64;

use super::build_runtime_manager;

/// Run an async body on a worker thread with the node's 32 MiB stack — the blessed genesis terms
/// recurse deeper than the 2 MiB test default, and so does normalizing them.
pub fn with_big_stack<F>(body: F)
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

pub fn ceremony_identity() -> rchain_casper::validator_identity::ValidatorIdentity {
    let sk = PrivateKey::new(vec![7u8; 32]);
    let public_key = Secp256k1
        .to_public(&sk)
        .expect("a fixed 32-byte scalar is a valid key");
    rchain_casper::validator_identity::ValidatorIdentity {
        public_key,
        private_key: sk,
        sig_algorithm: "secp256k1".to_string(),
    }
}

/// A deploy by a **real** key pair: every verb derives the caller's address from the deployer id, so
/// a placeholder would make `RevAddress!("fromDeployerId", …)` answer `Nil` and the probe would read
/// a refusal it did not intend.
pub fn deploy_signed_by(term: &str, seed: u8) -> SignedDeployData {
    let sk = PrivateKey::new(vec![seed; 32]);
    let pk = Secp256k1
        .to_public(&sk)
        .expect("a fixed 32-byte scalar is a valid secp256k1 key");
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
        deployer: pk.bytes().to_vec(),
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
    }
}

/// The REV address `deploy_signed_by(term, seed)`'s deployer id derives to — the owner prefix the
/// dictionary computes for that caller.
pub fn rev_address_of(seed: u8) -> String {
    let sk = PrivateKey::new(vec![seed; 32]);
    let pk = Secp256k1
        .to_public(&sk)
        .expect("a fixed 32-byte scalar is a valid key");
    rchain_rholang::util::rev_address::RevAddress::from_public_key(&pk)
        .expect("a secp256k1 public key has a REV address")
        .to_base58()
}

pub fn proof_of_stake() -> ProofOfStake {
    ProofOfStake {
        minimum_bond: rchain_shared::refined::NonNegI64::try_from(1).unwrap(),
        maximum_bond: rchain_shared::refined::NonNegI64::try_from(100).unwrap(),
        validators: Vec::new(),
        epoch_length: 1,
        quarantine_length: 1,
        number_of_active_validators: 1,
        executor_share: rchain_shared::refined::NonNegI64::try_from(0).unwrap(),
        absence_slack: rchain_shared::refined::NonNegI64::try_from(0).unwrap(),
        participation_grace: rchain_shared::refined::NonNegI64::try_from(0).unwrap(),
        min_phlo_price: rchain_shared::refined::NonNegI64::zero(),
        max_number_of_parents: 0,
        pos_multi_sig_public_keys: Vec::new(),
        pos_multi_sig_quorum: 1,
        pos_vault_pub_key: String::new(),
        dev_tax_vesting_blocks: NonNegI64::zero(),

        liveness_absence_threshold: 0,
        liveness_jail_after_boundaries: 0,
        liveness_jail_epochs: 0,
        liveness_vote_window: 0,
        liveness_slash_severity: 0,
    }
}

/// A probe: resolve the facet a client resolves, call `verb(args)`, and report `ok` iff the answer
/// equals `expect`. `with_id` binds `rho:rchain:deployerId` and passes it as `args[0]`, which is how
/// every write names its caller.
pub fn probe(facet: &str, verb: &str, args: &str, with_id: bool, expect: &str) -> String {
    let binder = if with_id {
        "deployerId(`rho:rchain:deployerId`), "
    } else {
        ""
    };
    format!(
        r#"new {binder}rl(`rho:registry:lookup`), fCh, ret in {{
             rl!(`{facet}`, *fCh) |
             for (f <- fCh) {{
               f!("{verb}", {args}, *ret) |
               for (@v <- ret) {{
                 if (v == {expect}) {{ @"out"!("ok") }} else {{ @"out"!("ne") }}
               }}
             }}
           }}"#
    )
}

/// [`probe`] with the whole condition spelled out, for claims that are not an equality.
pub fn probe_cond(facet: &str, verb: &str, args: &str, with_id: bool, cond: &str) -> String {
    let binder = if with_id {
        "deployerId(`rho:rchain:deployerId`), "
    } else {
        ""
    };
    format!(
        r#"new {binder}rl(`rho:registry:lookup`), fCh, ret in {{
             rl!(`{facet}`, *fCh) |
             for (f <- fCh) {{
               f!("{verb}", {args}, *ret) |
               for (@v <- ret) {{
                 if ({cond}) {{ @"out"!("ok") }} else {{ @"out"!("ne") }}
               }}
             }}
           }}"#
    )
}

/// Install genesis and run `probes` as ordered deploys, returning the `@"out"` tags.
pub async fn run(probes: Vec<(String, u8)>) -> Vec<String> {
    let rm = build_runtime_manager().await;
    let mut terms = default_blessed_terms(
        &proof_of_stake(),
        &Registry {
            system_contract_pub_key: String::new(),
        },
        &[],
        "root",
        &ceremony_identity(),
    )
    .expect("the blessed term list builds");
    for (term, seed) in probes {
        terms.push(deploy_signed_by(&term, seed));
    }
    let rand = rchain_crypto::hash::blake2b512_random::Blake2b512Random::from_init(&[7u8; 32]);
    let (_, _, results) = rm
        .compute_genesis(
            &terms,
            &rand,
            BlockData::empty(),
            &PosGenesis::default(),
            &[],
        )
        .await
        .expect("compute_genesis");
    for (i, r) in results.iter().enumerate() {
        assert!(
            r.eval_result.succeeded(),
            "genesis deploy #{i} failed: {:?}",
            r.eval_result.errors
        );
    }
    let produced = rm
        .runtime()
        .get_data_par(&rchain_models::sorted::SortedProc::new(
            rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString("out".to_string())),
        ))
        .await
        .expect("read the probes' output channel");
    produced
        .iter()
        .filter_map(|p| RhoString::unapply(p).map(str::to_string))
        .collect()
}

pub fn resolve_uri() -> String {
    rgov::masterdict_resolve_uri().expect("the resolve facet key")
}
pub fn publish_uri() -> String {
    rgov::masterdict_publish_uri().expect("the publish facet key")
}
pub fn root_uri() -> String {
    rgov::masterdict_root_uri().expect("the root facet key")
}
