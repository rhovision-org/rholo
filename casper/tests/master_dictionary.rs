//! The rooted-name master dictionary, **against the dictionary as deployed in genesis** (issue #99).
//!
//! The issue closes when "the namespace rule is stated (a `decide`-able model) **and the probes pass
//! against the deployed dictionary, the four must-fail cases included**". This is the second half:
//! every probe below resolves the facet a client would, at the constant key genesis publishes it
//! under, and runs as a *deploy signed by a real key* — so the caller's identity is derived exactly
//! the way the contract derives it.
//!
//! The probes are one ordered genesis: the dictionary bootstraps itself, then each probe runs as its
//! own deploy. That ordering is load-bearing, not stylistic — `publish` is append-only, so a probe
//! that pins version 0 only means something after a second version exists.

mod common;

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

use common::build_runtime_manager;

/// Run an async body on a worker thread with the node's 32 MiB stack — the blessed genesis terms
/// recurse deeper than the 2 MiB test default, and so does normalizing them.
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

fn ceremony_identity() -> rchain_casper::validator_identity::ValidatorIdentity {
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
fn deploy_signed_by(term: &str, seed: u8) -> SignedDeployData {
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
fn rev_address_of(seed: u8) -> String {
    let sk = PrivateKey::new(vec![seed; 32]);
    let pk = Secp256k1
        .to_public(&sk)
        .expect("a fixed 32-byte scalar is a valid key");
    rchain_rholang::util::rev_address::RevAddress::from_public_key(&pk)
        .expect("a secp256k1 public key has a REV address")
        .to_base58()
}

fn proof_of_stake() -> ProofOfStake {
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
fn probe(facet: &str, verb: &str, args: &str, with_id: bool, expect: &str) -> String {
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
fn probe_cond(facet: &str, verb: &str, args: &str, with_id: bool, cond: &str) -> String {
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
async fn run(probes: Vec<(String, u8)>) -> Vec<String> {
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

fn resolve_uri() -> String {
    rgov::masterdict_resolve_uri().expect("the resolve facet key")
}
fn publish_uri() -> String {
    rgov::masterdict_publish_uri().expect("the publish facet key")
}
fn root_uri() -> String {
    rgov::masterdict_root_uri().expect("the root facet key")
}

/// The alias tier the genesis bootstrap built — every name points somewhere, and an unknown name
/// answers `Nil` ("absence is not an error").
#[test]
fn the_genesis_alias_tier_resolves() {
    with_big_stack(async {
        let r = resolve_uri();
        let mut probes: Vec<(String, u8)> = [
            "Directory",
            "Echo",
            "Log",
            "Inbox",
            "Issue",
            "Kudos",
            "Roll",
            "Chat",
            "Ballot",
            "Group",
            "GetMe",
            "SendThem",
        ]
        .iter()
        .map(|name| {
            // The exact target is the *operator's* own rooted path, derived at genesis; the claim
            // that matters here is that the name is aliased at all, and `Nil` is what an unaliased
            // name answers. The name-by-name targets are pinned in `genesis_registry.rs`.
            (
                probe_cond(&r, "targetOf", &format!(r#"["{name}"]"#), false, "v != Nil"),
                11,
            )
        })
        .collect();
        probes.push((probe(&r, "targetOf", r#"["NoSuchName"]"#, false, "Nil"), 11));
        let tags = run(probes).await;
        assert_eq!(
            tags,
            vec!["ok"; 13],
            "every genesis name is aliased, and an unknown name is absent"
        );
    });
}

/// Append-only publish, pinned versions, the owner prefix, and absence-is-not-an-error.
#[test]
fn publish_is_append_only_and_versions_are_pinned() {
    with_big_stack(async {
        let (r, p) = (resolve_uri(), publish_uri());
        let alice = rev_address_of(11);
        let own = format!("{alice}/inbox");
        let tags = run(vec![
            (
                probe(
                    &p,
                    "publish",
                    &format!(r#"[*deployerId, "{own}", "v0"]"#),
                    true,
                    &format!(r#"("published", "{own}", 0)"#),
                ),
                11,
            ),
            (
                probe(
                    &p,
                    "publish",
                    &format!(r#"[*deployerId, "{own}", "v1"]"#),
                    true,
                    &format!(r#"("published", "{own}", 1)"#),
                ),
                11,
            ),
            (
                probe(&r, "resolve", &format!(r#"["{own}"]"#), false, r#""v1""#),
                11,
            ),
            (
                probe(
                    &r,
                    "resolveAt",
                    &format!(r#"["{own}", 0]"#),
                    false,
                    r#""v0""#,
                ),
                11,
            ),
            (
                probe(&r, "resolveAt", &format!(r#"["{own}", 9]"#), false, "Nil"),
                11,
            ),
            (
                probe(&r, "resolve", r#"["no-such-path"]"#, false, "Nil"),
                11,
            ),
            (
                probe(
                    &r,
                    "ownerOf",
                    &format!(r#"["{own}"]"#),
                    false,
                    &format!(r#""{alice}""#),
                ),
                11,
            ),
            (
                probe_cond(
                    &r,
                    "versionsOf",
                    &format!(r#"["{own}"]"#),
                    false,
                    r#"v == ["v0", "v1"]"#,
                ),
                11,
            ),
        ])
        .await;
        assert_eq!(tags, vec!["ok"; 8], "the publish/resolve round trip");
    });
}

/// The four must-fail cases, plus a forged identity, an unknown verb and a wrong arity.
#[test]
fn the_four_must_fail_cases() {
    with_big_stack(async {
        let (r, p, a) = (resolve_uri(), publish_uri(), root_uri());
        let alice = rev_address_of(11);
        let tags = run(vec![
            // 1. writing under another's root
            (
                probe(
                    &p,
                    "publish",
                    &format!(r#"[*deployerId, "{alice}/inbox", "bob"]"#),
                    true,
                    r#"("dir-error", "not your namespace")"#,
                ),
                12,
            ),
            // a forged / absent identity
            (
                probe(
                    &p,
                    "publish",
                    r#"["not-an-id", "x/y", "v"]"#,
                    false,
                    r#"("dir-error", "no identity")"#,
                ),
                11,
            ),
            // 4. a non-root identity setting a short name
            (
                probe(
                    &a,
                    "alias",
                    r#"[*deployerId, "Inbox", "rho:id:evil"]"#,
                    true,
                    r#"("dir-error", "not the root authority")"#,
                ),
                12,
            ),
            // an unknown verb, and a known verb at the wrong arity
            (
                probe(
                    &r,
                    "nonsense",
                    r#"[]"#,
                    false,
                    r#"("dir-error", "bad verb or arity")"#,
                ),
                11,
            ),
            (
                probe(
                    &r,
                    "resolve",
                    r#"[]"#,
                    false,
                    r#"("dir-error", "bad verb or arity")"#,
                ),
                11,
            ),
        ])
        .await;
        assert_eq!(tags, vec!["ok"; 5], "every refusal answers");
    });
}

/// `seal` closes a path to further versions — the other two of the four must-fail cases, which need
/// a published path first.
#[test]
fn seal_closes_a_path_and_the_key_still_resolves() {
    with_big_stack(async {
        let (r, p) = (resolve_uri(), publish_uri());
        let p0 = format!("{}/sealed", rev_address_of(11));
        let tags = run(vec![
            (
                probe(
                    &p,
                    "publish",
                    &format!(r#"[*deployerId, "{p0}", "v0"]"#),
                    true,
                    &format!(r#"("published", "{p0}", 0)"#),
                ),
                11,
            ),
            (
                probe(
                    &p,
                    "seal",
                    &format!(r#"[*deployerId, "{p0}"]"#),
                    true,
                    &format!(r#"("sealed", "{p0}")"#),
                ),
                11,
            ),
            // 3. publishing to a sealed path
            (
                probe(
                    &p,
                    "publish",
                    &format!(r#"[*deployerId, "{p0}", "v1"]"#),
                    true,
                    r#"("dir-error", "sealed")"#,
                ),
                11,
            ),
            (
                probe(&r, "resolve", &format!(r#"["{p0}"]"#), false, r#""v0""#),
                11,
            ),
            (
                probe(&r, "sealed", &format!(r#"["{p0}"]"#), false, "true"),
                11,
            ),
        ])
        .await;
        assert_eq!(
            tags,
            vec!["ok"; 5],
            "seal closes the path and it still resolves"
        );
    });
}

/// The second of the four must-fail cases: a granted writekey dies at `revoke`.
///
/// The probe is written the way a consumer writes it — `grant`, take the key, publish through it —
/// and it is the shape that caught a real mistake while this was being written: `for (@wk <- key)`
/// binds the key as a **value**, and a value cannot be called. It must be `for (wk <- key)`.
#[test]
fn a_granted_key_works_until_it_is_revoked() {
    with_big_stack(async {
        let (r, p) = (resolve_uri(), publish_uri());
        let path = format!("{}/keyed", rev_address_of(11));
        let use_key = |then: &str, value: &str, expect: String| {
            format!(
                r#"new deployerId(`rho:rchain:deployerId`), rl(`rho:registry:lookup`),
                     fCh, key, ret, r2
                   in {{
                     rl!(`{p}`, *fCh) |
                     for (f <- fCh) {{
                       f!("grant", [*deployerId, "{path}"], *key) |
                       for (wk <- key) {{
                         f!("{then}", [*deployerId, "{path}"], *ret) |
                         for (@_a <- ret) {{
                           wk!("{value}", *r2) |
                           for (@v <- r2) {{
                             if (v == {expect}) {{ @"out"!("ok") }} else {{ @"out"!("ne") }}
                           }}
                         }}
                       }}
                     }}
                   }}"#
            )
        };
        let tags = run(vec![
            (
                use_key("publish", "a", format!(r#"("published", "{path}", 0)"#)),
                11,
            ),
            // ...and after a revoke the same shape is refused: that is the must-fail case.
            (
                use_key("revoke", "b", r#"("dir-error", "revoked")"#.to_string()),
                11,
            ),
            // The revoked key did not append, so version 1 is still absent.
            (
                probe(&r, "resolveAt", &format!(r#"["{path}", 1]"#), false, "Nil"),
                11,
            ),
        ])
        .await;
        assert_eq!(tags, vec!["ok"; 3], "the key works, then revoke kills it");
    });
}
