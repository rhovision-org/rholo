//! Genesis registry content.
//!
//! A fresh chain's `rho:registry:lookup` must resolve the shorthands consumers actually reach for,
//! and the value it hands back must be **callable** — not merely non-`Nil`. The distinction is the
//! whole point: an unmatched `for` is not an error in rholang, so a lookup that resolves to nothing
//! usable produces a silent no-op that reads exactly like a client bug (this is how the rgov family
//! came to return `[]`). `spec/GENESIS.md` is the manifest these tests pin, with the consumer
//! evidence for every entry.
//!
//! Each probe below is written the way the *consumer* writes it, with the consumer named — a shape
//! this port accepts but a client does not would be worthless.

mod common;

use std::collections::BTreeMap;

use rchain_casper::genesis::contracts::{ProofOfStake, Registry};
use rchain_casper::genesis::default_blessed_terms;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_models::ast::Par;
use rchain_models::casper::protocol::casper_message::{
    DeployData, ProcessedDeploy, SignedDeployData,
};
use rchain_models::rholang::RhoType::{RhoList, RhoString};
use rchain_rholang::native_state::PosGenesis;
use rchain_rholang::system_processes::BlockData;
use rchain_shared::refined::NonNegI64;

use common::{build_runtime_manager, fringe_state};

fn fixed_rand() -> Blake2b512Random {
    Blake2b512Random::from_init(&[7u8; 32])
}

fn deploy(term: &str) -> SignedDeployData {
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
        deployer: vec![0u8; 32],
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
    }
}

/// Run an async body on a worker thread with the node's 32 MiB stack. The blessed genesis terms
/// recurse deeper than the default 2 MiB test stack allows, and so does normalizing them — this
/// mirrors `node/src/main.rs` and `node/tests/common/mod.rs`.
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

/// The genesis ceremony's identity, fixed so a test genesis is deterministic.
fn ceremony_identity() -> rchain_casper::validator_identity::ValidatorIdentity {
    use rchain_crypto::private_key::PrivateKey;
    use rchain_crypto::signatures::secp256k1::Secp256k1;
    use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
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

/// A deploy signed by a **real** key pair, so `rho:rchain:deployerId` is a genuine public key. Terms
/// that derive a REV address from it need that: `RevAddress!("fromPublicKey", …)` matches nothing
/// for a placeholder byte-array, and the call then stalls silently — which is what a governance
/// handshake does before it answers.
fn deploy_signed_by(term: &str, seed: u8) -> SignedDeployData {
    use rchain_crypto::private_key::PrivateKey;
    use rchain_crypto::signatures::secp256k1::Secp256k1;
    use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
    let sk = PrivateKey::new(vec![seed; 32]);
    let pk = Secp256k1
        .to_public(&sk)
        .expect("a fixed 32-byte scalar is a valid secp256k1 key");
    let mut d = deploy(term);
    d.deployer = pk.bytes().to_vec();
    d
}

/// The REV address that `deploy_signed_by(term, seed)`'s deployer id derives to — what the
/// dictionary's `publish` verb computes for that caller, so a probe can name a path under its own
/// root without guessing.
fn rev_address_of(seed: u8) -> String {
    use rchain_crypto::private_key::PrivateKey;
    use rchain_crypto::signatures::secp256k1::Secp256k1;
    use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
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

/// One probe per seeded shorthand: look it up, destructure the `(nonce, value)` pair the way the
/// consumer does, **call** the value, and only then report the tag. The call's reply is what proves
/// reachability — a lookup that yields a name nothing answers on never reaches the tag.
///
/// The two destructuring conventions are both real consumer code: `@(_, X)` + `@X!(…)` is the
/// wallet (`r-wallet/src/utils/rho.ts:31`), `@(_, *X)` + `X!(…)` is rgov
/// (`rgov/rholang/core/Issue.rho`). Each probe uses the form of the consumer it is named for.
fn probes() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "rho:rchain:revVault",
            r#"new rl(`rho:registry:lookup`), ch, ret in {
                 rl!(`rho:rchain:revVault`, *ch) |
                 for (@(_, RevVault) <- ch) {
                   @RevVault!("getBalance", "111111111111111111111111111111111111111111111111111111", *ret) |
                   for (@_ <- ret) { @"out"!("rho:rchain:revVault") }
                 }
               }"#,
        ),
        (
            "rho:rchain:pos",
            r#"new rl(`rho:registry:lookup`), ch, ret in {
                 rl!(`rho:rchain:pos`, *ch) |
                 for (@(_, PoS) <- ch) {
                   @PoS!("getBonds", *ret) |
                   for (@_ <- ret) { @"out"!("rho:rchain:pos") }
                 }
               }"#,
        ),
        (
            "rho:lang:listOps",
            r#"new rl(`rho:registry:lookup`), ch, ret in {
                 rl!(`rho:lang:listOps`, *ch) |
                 for (@(_, *ListOps) <- ch) {
                   ListOps!("range", 0, 2, *ret) |
                   for (@_ <- ret) { @"out"!("rho:lang:listOps") }
                 }
               }"#,
        ),
        (
            "rho:lang:nonNegativeNumber",
            r#"new rl(`rho:registry:lookup`), ch, ret in {
                 rl!(`rho:lang:nonNegativeNumber`, *ch) |
                 for (@(_, *NonNegativeNumber) <- ch) {
                   NonNegativeNumber!(3, *ret) |
                   for (@_ <- ret) { @"out"!("rho:lang:nonNegativeNumber") }
                 }
               }"#,
        ),
        (
            // The multi-signature vault, installed rather than refused (AUDIT C114's alternative).
            // `makeSealerUnsealer` is the cheapest method that proves the *contract* is answering
            // rather than a stub: it mints a pair and replies with both.
            "rho:rchain:multiSigRevVault",
            r#"new rl(`rho:registry:lookup`), ch, ret in {
                 rl!(`rho:rchain:multiSigRevVault`, *ch) |
                 for (@(_, *MultiSigRevVault) <- ch) {
                   MultiSigRevVault!("makeSealerUnsealer", *ret) |
                   for (@_ <- ret) { @"out"!("rho:rchain:multiSigRevVault") }
                 }
               }"#,
        ),
        (
            "rho:rchain:makeMint",
            r#"new rl(`rho:registry:lookup`), ch, ret in {
                 rl!(`rho:rchain:makeMint`, *ch) |
                 for (@(nonce, *MakeMint) <- ch) {
                   MakeMint!(*ret) |
                   for (@_ <- ret) { @"out"!("rho:rchain:makeMint") }
                 }
               }"#,
        ),
        (
            // The ERTP object API (issue #249). `makeIssuerKit` is the cheapest call that proves the
            // *contract* is answering over the native ledger rather than a stub: it draws a brand and
            // an authority from the ledger and replies both, so a `lookup!` that resolved to nothing
            // usable never reaches the tag.
            "rho:rchain:ertp",
            r#"new rl(`rho:registry:lookup`), ch, ret in {
                 rl!(`rho:rchain:ertp`, *ch) |
                 for (@(_, ERTP) <- ch) {
                   @ERTP!("makeIssuerKit", *ret) |
                   for (@_ <- ret) { @"out"!("rho:rchain:ertp") }
                 }
               }"#,
        ),
        (
            // Not a shorthand but the *contract's own* multi-step path: `unorderedParMap` is the one
            // `ListOps` operation that runs through `collect` (`ListOps.rho:197-215`), whose
            // `if (sc == cc + 1)` follows a send. While any non-first `if` reduced to nothing (AUDIT
            // C21), `collect` neither advanced its counters nor answered, so this call could never
            // return — a hang, but the silent kind. `range` (the probe above) takes a path with no
            // `if` and would have passed throughout, which is exactly why this one is here.
            "rho:lang:listOps:unorderedParMap",
            r#"new rl(`rho:registry:lookup`), ch, ret, double in {
                 contract double(@x, r) = { r!(x * 2) } |
                 rl!(`rho:lang:listOps`, *ch) |
                 for (@(_, *ListOps) <- ch) {
                   ListOps!("unorderedParMap", [1, 2, 3], *double, *ret) |
                   for (@_ <- ret) { @"out"!("rho:lang:listOps:unorderedParMap") }
                 }
               }"#,
        ),
    ]
}

/// On a fresh chain every seeded shorthand resolves **and its value answers a call**.
#[test]
fn a_fresh_chain_resolves_and_can_call_every_seeded_shorthand() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();

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
        assert!(
            !terms.is_empty(),
            "genesis must install something — an empty list is the defect this test exists for"
        );
        let expected: Vec<&str> = probes().iter().map(|(name, _)| *name).collect();
        terms.extend(probes().iter().map(|(_, term)| deploy(term)));

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

        // Each probe reported its tag only after its call was answered.
        let produced = rm
            .runtime()
            .get_data_par(&rchain_models::sorted::SortedProc::new(
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString(
                    "out".to_string(),
                )),
            ))
            .await
            .expect("read the probes' output channel");
        let mut tags: Vec<String> = produced
            .iter()
            .filter_map(|p| {
                rchain_models::rholang::RhoType::RhoString::unapply(p).map(str::to_string)
            })
            .collect();
        tags.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            tags, expected,
            "every seeded shorthand must resolve AND answer a call"
        );
    });
}

/// **The genesis replays to itself** — the falsifier for #293.
///
/// A validator joining a chain holds no mergeable-channel sidecar for block #0, so `merging.rs`
/// regenerates one by replaying the genesis (`regenerate_sidecars`). That replay runs `replay_deploys`
/// with `with_cost_accounting = false`, and re-seeds the registry aliases **after every blessed
/// deploy** — writes that are genesis state, exactly as `compute_genesis` has them, which holds one
/// `NativeWriter::Genesis` window open across its whole loop (`runtime_manager.rs:789-828`). The
/// replay used to close its genesis window *before* that loop, so every post-deploy re-seed landed
/// with **no** writer. `BlockNativeEffects::from_drain` refuses a block with an unattributed write,
/// so the regeneration returned
///
///     failed to regenerate mergeable channels … N native write(s) outside any deploy's window
///
/// and every propose on a chain whose genesis installs anything stalled (`dev`, 2026-10-09, #293).
///
/// The genesis below is the *blessed* one — the case that seeds an alias after a deploy, and so the
/// case that goes red. Red on the pre-fix tree by the refusal above; green once the replay holds its
/// window across the deploy loop.
#[test]
fn the_genesis_replays_to_itself() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();
        let terms = default_blessed_terms(
            &proof_of_stake(),
            &Registry {
                system_contract_pub_key: String::new(),
            },
            &[],
            "root",
            &ceremony_identity(),
        )
        .expect("the blessed term list builds");
        assert!(
            !terms.is_empty(),
            "the genesis must install something — an empty list would never re-seed an alias after \
             a deploy, which is the write this test exists for"
        );

        let (empty, post, results) = rm
            .compute_genesis(
                &terms,
                &rand,
                BlockData::empty(),
                &PosGenesis::default(),
                &[],
            )
            .await
            .expect("compute_genesis");
        let processed: Vec<ProcessedDeploy> = results.iter().map(|r| r.deploy.clone()).collect();

        // The genesis replay as `merging.rs` performs it: no cost accounting, and the vaults the
        // genesis was created with (`&[]` here).
        let (replayed, _) = rm
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
            .expect(
                "the genesis must replay to itself — a replay that leaves a genesis write with no \
                 deploy to attribute it to is refused (#293)",
            );
        assert_eq!(
            post, replayed,
            "the replayed genesis must compute the same post-state as the played one"
        );
    });
}

/// Two genesis ceremonies over fresh stores must agree on the registered values — the property that
/// makes the aliases chain-independent (a consumer hardcodes them; `spec/GENESIS.md`).
/// A deploy signed by a **real 65-byte key**. `deploy` above uses a 32-byte placeholder, which
/// `RevAddress::from_deployer_id` refuses; anything that spends needs a deployerId that resolves.
fn deploy_from(
    term: &str,
    deployer: Vec<u8>,
) -> rchain_models::casper::protocol::casper_message::SignedDeployData {
    let mut d = deploy(term);
    d.deployer = deployer;
    d
}

/// A genesis vault for `public_key`, so the funding transfer has something to move.
fn vault_for(public_key: Vec<u8>, amount: i64) -> rchain_casper::genesis::contracts::Vault {
    rchain_casper::genesis::contracts::Vault {
        rev_address: rchain_rholang::util::rev_address::RevAddress::from_public_key(
            &rchain_crypto::public_key::PublicKey::new(public_key),
        )
        .expect("a valid rev address"),
        initial_balance: rchain_shared::refined::NonNegI64::try_from(amount).expect("non-negative"),
    }
}

/// **The multi-signature vault actually moving funds** — AUDIT C114's alternative, exercised rather
/// than merely installed.
///
/// The authorisation is the *human* path the oracle's wallet uses: `deployerAuthKey` makes an
/// `AuthKey` token whose shape is `(*_multiSigRevVault, pubKey)`, and the vault's `transfer` checks it
/// through the installed `AuthKey` contract. **The shape contains the contract's own private name**,
/// so only the contract can build it — which is why this path needs no native code at all, and why
/// the earlier attempt through the sealer/unsealer machinery was looking in the wrong place.
///
/// What the flow proves, beyond "the contract answers": the vault's REV is spent by *the contract*,
/// through the vault handle's `transfer` and an `unforgeableAuthKey` — i.e. the delegated spend that
/// `spec/RUST-FIRST.md`'s B2 said this port did not have. The assertion is the destination balance,
/// because a reply of "done" with no movement is the defect C114 was about, one layer in.
fn multi_sig_single_key_term(target: &str) -> String {
    format!(
        r#"new rl(`rho:registry:lookup`), ch, deployerId(`rho:rchain:deployerId`),
               DeployerIdOps(`rho:rchain:deployerId:ops`),
               mainVault(`rho:rchain:revVault`),
               pkCh, createCh, authCh, fundCh, transferCh, out(`rho:io:stdout`) in {{
             rl!(`rho:rchain:multiSigRevVault`, *ch) |
             for (@(_, *MultiSigRevVault) <- ch) {{
               DeployerIdOps!("pubKeyBytes", *deployerId, *pkCh) |
               for (@pk <- pkCh) {{
                 // Quorum 1 with this deployer's own key: the simple custody case, and the one whose
                 // authorisation needs nothing but the deployer's identity.
                 MultiSigRevVault!("create", [pk], [], 1, *createCh) |
                 for (@created <- createCh) {{
                   match created {{
                     (true, (*multiSig, revAddr, *revVault)) => {{
                       // Fund the vault the contract just made: its address comes from a fresh name,
                       // so it starts empty and a transfer out of it would fail for insufficient
                       // funds — correctly. (The oracle's wallet funds the vault too.)
                       mainVault!("transfer", *deployerId, revAddr, 40000000, *fundCh) |
                       for (_ <- fundCh) {{
                         MultiSigRevVault!("deployerAuthKey", *deployerId, *authCh) |
                         for (auth <- authCh) {{
                           multiSig!("transfer", "{target}", 30000000, *auth, *transferCh) |
                           for (@result <- transferCh) {{ @"out"!(result) }}
                         }}
                       }}
                     }}
                     other => {{ @"out"!(other) }}
                   }}
                 }}
               }}
             }}
           }}"#
    )
}

#[test]
fn a_multisig_vault_spends_through_the_contract_that_holds_it() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();
        let target = rchain_rholang::util::rev_address::RevAddress::from_public_key(
            &rchain_crypto::public_key::PublicKey::new(vec![2u8; 65]),
        )
        .expect("target address")
        .to_base58();

        let terms = default_blessed_terms(
            &proof_of_stake(),
            &Registry {
                system_contract_pub_key: String::new(),
            },
            &[],
            "root",
            &ceremony_identity(),
        )
        .expect("the blessed term list builds");

        let (_, post, _) = rm
            .compute_genesis(
                &terms,
                &rand,
                BlockData::empty(),
                &PosGenesis::default(),
                &[vault_for(vec![0u8; 65], 1_000_000_000)],
            )
            .await
            .expect("compute_genesis");

        let (post, results, _) = rm
            .compute_state(
                &post,
                &[deploy_from(
                    &multi_sig_single_key_term(&target),
                    vec![0u8; 65],
                )],
                &[],
                &rand,
                BlockData::empty(),
                &fringe_state(1),
            )
            .await
            .expect("the multi-signature deploy runs");
        let last = results.last().expect("the deploy ran");
        assert!(
            last.eval_result.succeeded(),
            "the flow must run: {:?}",
            last.eval_result.errors
        );

        {
            let ch = rchain_models::sorted::SortedProc::new(rchain_models::par_ops::from_expr(
                rchain_models::ast::Expr::GString("out".to_string()),
            ));
            let datums = rm.runtime().get_data_par(&ch).await.expect("read out");
            assert!(
                !datums.is_empty(),
                "the contract must answer the transfer (post-state {post:?})"
            );
            // The contract's success shape for a single-signature vault is `(true, (false, "done"))`
            // — the inner `false` means "no further confirmation is needed", which is why the check
            // is on the *word* rather than on the presence of a boolean. (The first version of this
            // test asserted no `GBool(false)` at all, and would have failed the success case.)
            let text = format!("{:?}", datums);
            assert!(
                text.contains("done") && !text.contains("invalid auth"),
                "the deployer's own auth key must authorise the transfer: {text}"
            );
        }

        let native =
            rchain_rholang::native_state::NativeSystemState::new(rm.runtime().native_store());
        let balance = native
            .vault_balance(&target)
            .await
            .expect("read the target balance")
            .map(i64::from)
            .unwrap_or(0);
        assert_eq!(
            balance, 30_000_000,
            "the contract's own transfer must move the funds: a reply without a movement is not the \
             delegated spend working (post-state {post:?})"
        );
    });
}

#[test]
fn the_seeded_registry_is_identical_across_fresh_genesis_ceremonies() {
    with_big_stack(async {
        let mut digests = Vec::new();
        for _ in 0..2 {
            let rm = build_runtime_manager().await;
            let rand = fixed_rand();
            let terms = default_blessed_terms(
                &proof_of_stake(),
                &Registry {
                    system_contract_pub_key: String::new(),
                },
                &[],
                "root",
                &ceremony_identity(),
            )
            .expect("blessed terms");
            rm.compute_genesis(
                &terms,
                &rand,
                BlockData::empty(),
                &PosGenesis::default(),
                &[],
            )
            .await
            .expect("compute_genesis");
            let native =
                rchain_rholang::native_state::NativeSystemState::new(rm.runtime().native_store());
            let mut entries = Vec::new();
            for alias in rchain_casper::genesis::standard_deploys::GENESIS_ALIASES {
                let value = native
                    .registry_lookup(alias.shorthand)
                    .await
                    .expect("lookup")
                    .unwrap_or_else(|| panic!("{} must be seeded", alias.shorthand));
                entries.push(format!("{} = {value:?}", alias.shorthand));
            }
            digests.push(entries.join("\n"));
        }
        assert_eq!(
            digests[0], digests[1],
            "genesis registry content must not depend on the ceremony"
        );
        assert!(
            digests[0].lines().count() >= 5,
            "all seeded aliases are covered"
        );
    });
}

/// An inbox survives a read. `Inbox.read` — the zero-argument form, which reads and *removes* every
/// message — consumed the store datum and never restored it, so the second thing anyone did with an
/// inbox was the last: every later `write`, `peek` and `read` waited on an empty channel, silently, for
/// the life of the chain. The vendored text is repaired at render time (`rgov.rs::source("inbox")`,
/// recorded in the NOTICE); this is the consumer-level pin, using the class's own call shapes.
#[test]
fn a_read_does_not_destroy_the_inbox() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();
        let inbox_uri = rchain_casper::genesis::rgov::contract_uri_for("inbox").expect("the URI");
        let term = format!(
            r#"new rl(`rho:registry:lookup`), ch, caps, ret1, ret2, ret3 in {{
                 rl!(`{inbox_uri}`, *ch) |
                 // The class registers with `insertArbitrary`, so the lookup answers the bare value.
                 for (Inbox <- ch) {{
                   Inbox!(*caps) |
                   for (read, write, peek <- caps) {{
                     write!(["email", "from", {{"a": 1}}], *ret1) |
                     for (@_w1 <- ret1) {{
                       read!(*ret2) |
                       for (@_all <- ret2) {{
                         write!(["email", "from", {{"a": 2}}], *ret3) |
                         for (@_w3 <- ret3) {{ @"out"!("inbox-survived-read") }}
                       }}
                     }}
                   }}
                 }}
               }}"#
        );
        let mut terms = default_blessed_terms(
            &proof_of_stake(),
            &Registry {
                system_contract_pub_key: String::new(),
            },
            &[],
            "root",
            &ceremony_identity(),
        )
        .expect("blessed terms");
        terms.push(deploy_signed_by(&term, 11));

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
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString(
                    "out".to_string(),
                )),
            ))
            .await
            .expect("read the probe's output channel");
        let tags: Vec<String> = produced
            .iter()
            .filter_map(|p| RhoString::unapply(p).map(str::to_string))
            .collect();
        assert!(
            tags.contains(&"inbox-survived-read".to_string()),
            "a write after a read must still be answered — the read consumed the store and left it \
             consumed: {tags:?}"
        );
    });
}

/// **One** `Group!("new", …)`, the control for the two-call test below: the same contract, the same
/// store, no contention. If this answers and two calls do not, the defect is the shared store's
/// discipline (law 41) rather than the body's statements — which is what separates C25's three
/// candidates for good.
#[test]
fn a_fresh_chain_answers_one_group_creation() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();
        let group_uri = rchain_casper::genesis::rgov::contract_uri_for("group").expect("the URI");
        let term = format!(
            r#"new rl(`rho:registry:lookup`), deployerId(`rho:rchain:deployerId`), ch, ret in {{
                 for (@_d <<- @[*deployerId, "dictionary"]) {{ @"out"!("dictionary-present") }} |
                 rl!(`{group_uri}`, *ch) |
                 for (Group <- ch) {{
                   Group!("new", "t1", Nil, *ret) |
                   for (@_a <- ret) {{ @"out"!("one-new-answered") }}
                 }}
               }}"#
        );
        let mut terms = default_blessed_terms(
            &proof_of_stake(),
            &Registry {
                system_contract_pub_key: String::new(),
            },
            &[],
            "root",
            &ceremony_identity(),
        )
        .expect("blessed terms");
        terms.push(deploy_signed_by(&term, 7));

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
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString(
                    "out".to_string(),
                )),
            ))
            .await
            .expect("read the probe's output channel");
        let tags: Vec<String> = produced
            .iter()
            .filter_map(|p| RhoString::unapply(p).map(str::to_string))
            .collect();
        assert!(
            tags.contains(&"dictionary-present".to_string()),
            "the deployer's dictionary locker must be readable: {tags:?}"
        );
        assert!(
            tags.contains(&"one-new-answered".to_string()),
            "a single `Group!(\"new\", …)` must answer — if it does and two do not, the store's \
             discipline is the defect and the body's statements are not: {tags:?}"
        );
    });
}

/// Two `Group` creations in one deploy, which is the shape the wallet's `newGroup` and the vendored
/// contract's own self-test both use — and the shape that stalled (AUDIT C25).
///
/// `Group.rho`'s `new` contract takes the group map with a **linear** receive (`for (@groups <-
/// groupMapCh)`, `:49`) and puts a datum back only on the *error* branch (`:52`). The success branch's
/// write-back is at `:65`, inside two nested receives (`:59`'s `for (Directory <- ret)` and `:63`'s
/// `for (@{…} <- dirCh)`) — so the store is empty for the whole creation sequence. A second `new`
/// waits on a channel that will not speak until the first finishes, and if the first's chain never
/// resolves the store is gone for the life of the chain, silently. That is law 41's violation: a
/// replicable reader must restore what it consumes.
///
/// Three statements in that body were the candidates (`:49`'s consume, `:50`'s
/// `if (groups.get(name) != Nil)`, `:55`'s dictionary peek), and this test separates them by
/// observation rather than by argument: the dictionary witness below proves the deployer's locker is
/// present, so if `Group!("new", …)` still answers nothing, the peek at `:55` is ruled out and the
/// store is the culprit — and "which call answered" says whether the first one completed at all.
#[test]
fn a_fresh_chain_answers_two_group_creations_in_one_deploy() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();
        let group_uri = rchain_casper::genesis::rgov::contract_uri_for("group").expect("the URI");
        let term = format!(
            r#"new rl(`rho:registry:lookup`), deployerId(`rho:rchain:deployerId`),
                 ch, ret1, ret2
               in {{
                 // The precondition `Group.rho:55` waits on, witnessed: if this tag is absent the
                 // deployer's dictionary locker was never written and the peek cannot fire.
                 for (@_d <<- @[*deployerId, "dictionary"]) {{ @"out"!("dictionary-present") }} |
                 rl!(`{group_uri}`, *ch) |
                 for (Group <- ch) {{
                   Group!("new", "t1", Nil, *ret1) |
                   Group!("new", "t2", Nil, *ret2) |
                   for (@_a <- ret1) {{ @"out"!("first-new-answered") }} |
                   for (@_b <- ret2) {{ @"out"!("second-new-answered") }}
                 }}
               }}"#
        );
        // The ceremony key deploys it: the feature's epilogue writes `@[*deployerId, "dictionary"]`
        // for the key that deployed it, and that key is the one `Group.rho:55` peeks.
        let mut terms = default_blessed_terms(
            &proof_of_stake(),
            &Registry {
                system_contract_pub_key: String::new(),
            },
            &[],
            "root",
            &ceremony_identity(),
        )
        .expect("blessed terms");
        terms.push(deploy_signed_by(&term, 7));

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
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString(
                    "out".to_string(),
                )),
            ))
            .await
            .expect("read the probe's output channel");
        let tags: Vec<String> = produced
            .iter()
            .filter_map(|p| RhoString::unapply(p).map(str::to_string))
            .collect();

        assert!(
            tags.contains(&"dictionary-present".to_string()),
            "the deployer's dictionary locker must be readable, so that a `Group!(\"new\", …)` that \
             still stalls is not the `:55` peek — the precondition this test has to establish before \
             its verdict means anything: {tags:?}"
        );
        assert!(
            tags.contains(&"first-new-answered".to_string()),
            "the first `Group!(\"new\", …)` must answer — its store is taken at `:49` and the \
             read-modify-write chain from `:50` to `:66` must complete: {tags:?}"
        );
        assert!(
            tags.contains(&"second-new-answered".to_string()),
            "the second `Group!(\"new\", …)` must answer too. `:49`'s linear consume puts a datum \
             back only on the error branch (`:52`), so while the first creation is in flight the \
             group map is empty and this call waits on a channel nobody will fill — law 41: a \
             replicable reader restores what it consumes: {tags:?}"
        );
    });
}

/// The three class names the wallet's editor asks for resolve to a **value**, not `Nil`.
///
/// These were directory *slots* written once by `extraSlots`, our own term — which called the
/// directory's write capability with two arguments where `Directory.rho:56` takes three, so no
/// receive matched, nothing was written, and every read of those names answered `Nil` (AUDIT C21).
/// Under the rooted dictionary (issue #99) they are alias-tier names instead: the genesis bootstrap
/// points `Chat`/`Ballot`/`Group` at the class URIs, and `resolve` answers what the alias points at.
///
/// The trap is unchanged and that is why this asserts the *value* and not merely that a receive
/// fired: a name the dictionary does not know answers `Nil`, and a `Nil` is indistinguishable from
/// broken for a consumer.
#[test]
fn the_class_names_resolve_through_the_alias_tier() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();
        let resolve =
            rchain_casper::genesis::rgov::masterdict_resolve_uri().expect("the resolve facet key");
        let term = r#"new rl(`rho:registry:lookup`), ch, chatCh, ballotCh, groupCh in {
                 rl!(`RESOLVE`, *ch) |
                 for (MCAread <- ch) {
                   MCAread!("resolve", ["Chat"], *chatCh) |
                   MCAread!("resolve", ["Ballot"], *ballotCh) |
                   MCAread!("resolve", ["Group"], *groupCh) |
                   for (@c <- chatCh) {
                     if (c == Nil) { @"out"!("slot:Chat-nil") } else { @"out"!("slot:Chat-value") }
                   } |
                   for (@b <- ballotCh) {
                     if (b == Nil) { @"out"!("slot:Ballot-nil") } else { @"out"!("slot:Ballot-value") }
                   } |
                   for (@g <- groupCh) {
                     if (g == Nil) { @"out"!("slot:Group-nil") } else { @"out"!("slot:Group-value") }
                   }
                 }
               }"#
            .replace("RESOLVE", &resolve);
        let mut terms = default_blessed_terms(
            &proof_of_stake(),
            &Registry {
                system_contract_pub_key: String::new(),
            },
            &[],
            "root",
            &ceremony_identity(),
        )
        .expect("blessed terms");
        terms.push(deploy_signed_by(&term, 11));

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
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString(
                    "out".to_string(),
                )),
            ))
            .await
            .expect("read the probe's output channel");
        let tags: Vec<String> = produced
            .iter()
            .filter_map(|p| RhoString::unapply(p).map(str::to_string))
            .collect();
        for name in ["Chat", "Ballot", "Group"] {
            assert!(
                tags.contains(&format!("slot:{name}-value")),
                "the directory must hold a usable {name} class (an unfilled slot answers `Nil`, \
                 which a consumer cannot tell from broken): {tags:?}"
            );
        }
    });
}

/// A probe for a contract reached by its **constant URI**.
///
/// `destructure` is the pattern the *consumer* uses, and it differs by contract family: the node's
/// own blessed contracts register signed, so they destructure `@(_, X)` (a `(nonce, value)` pair),
/// while the vendored rgov classes register exactly as upstream does (`insertArbitrary`) and
/// destructure the **bare** value `X`. Using the wrong one is exactly the failure this file exists to
/// catch: it matches nothing and the probe simply never reports.
fn lookup_probe(uri: &str, tag: &str, destructure: &str, call: &str, reply_names: &str) -> String {
    format!(
        r#"new rl(`rho:registry:lookup`), ch, ret, log in {{
             rl!(`{uri}`, *ch) |
             for ({destructure} <- ch) {{
               @"out"!("{tag}:resolved") |
               {call} |
               for ({reply_names} <- ret) {{ @"out"!("{tag}:called") }}
             }}
           }}"#
    )
}

/// On a fresh chain every vendored rgov class contract is installed under its constant URI and
/// answers a call. This is what a governance client gets instead of running
/// `scripts/bootstrap-rgov.ts` and recording whatever URI the deployment happened to produce.
#[test]
fn a_fresh_chain_installs_the_rgov_contracts_and_they_answer() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();
        let uris = rchain_casper::genesis::rgov::contract_uris().expect("the vendored URIs");
        let uri = |name: &str| {
            uris.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, u)| u.clone())
                .unwrap_or_else(|| panic!("{name} must be vendored"))
        };

        // Each call is upstream's own shape (its contract signature, or its self-test's call), and
        // `reply_names` is that contract's reply *arity*: `Kudos` and the member directory answer
        // with one value, `Directory` with its capability map, `Inbox` with three capabilities and
        // `Issue` with `(admin, tally)`. A pattern of the wrong arity matches nothing, which is the
        // same class of silence this file exists to turn into a failure.
        let probes: Vec<(&str, String)> = vec![
            (
                "kudos",
                lookup_probe(&uri("kudos"), "kudos", "@X", r#"@X!("peek", *ret)"#, "@_"),
            ),
            (
                "inbox",
                lookup_probe(&uri("inbox"), "inbox", "@X", r#"@X!(*ret)"#, "@_, @_, @_"),
            ),
            (
                "directory",
                lookup_probe(
                    &uri("directory"),
                    "directory",
                    "@X",
                    r#"@X!(Nil, *ret)"#,
                    "@_",
                ),
            ),
            (
                "roll",
                lookup_probe(&uri("roll"), "roll", "@X", r#"@X!("make", {}, *ret)"#, "@_"),
            ),
            (
                "issue",
                lookup_probe(
                    &uri("issue"),
                    "issue",
                    "@X",
                    r#"@X!(["proposal"], *ret, *log)"#,
                    "@_, @_",
                ),
            ),
            // The three classes upstream's template does not list, but the wallet's editor asks the
            // directory for by name — installed and callable like the rest.
            (
                "chat",
                lookup_probe(&uri("chat"), "chat", "@X", r#"@X!(*ret)"#, "@_, @_, @_"),
            ),
            (
                "ballot",
                lookup_probe(
                    &uri("ballot"),
                    "ballot",
                    "@X",
                    r#"@X!(["p1"], *ret, *log)"#,
                    "@_, @_",
                ),
            ),
            (
                "group",
                lookup_probe(&uri("group"), "group", "@X", r#"@X!("lookup", *ret)"#, "@_"),
            ),
        ];

        let mut terms = default_blessed_terms(
            &proof_of_stake(),
            &Registry {
                system_contract_pub_key: String::new(),
            },
            &[],
            "root",
            &ceremony_identity(),
        )
        .expect("blessed terms");
        let blessed = terms.len();
        assert!(
            blessed >= 8,
            "the blessed set includes the libraries and the five vendored contracts (got {blessed})"
        );
        terms.extend(probes.iter().map(|(_, term)| deploy(term)));

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

        // Installed, separately from callable: a direct native read attributes a failure to the
        // registration rather than to the probe.
        let native =
            rchain_rholang::native_state::NativeSystemState::new(rm.runtime().native_store());
        for (name, uri) in &uris {
            assert!(
                native.registry_lookup(uri).await.expect("lookup").is_some(),
                "{name} must be registered under {uri}"
            );
        }
        let produced = rm
            .runtime()
            .get_data_par(&rchain_models::sorted::SortedProc::new(
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString(
                    "out".to_string(),
                )),
            ))
            .await
            .expect("read the probes' output channel");
        let mut tags: Vec<String> = produced
            .iter()
            .filter_map(|p| {
                rchain_models::rholang::RhoType::RhoString::unapply(p).map(str::to_string)
            })
            .collect();
        tags.sort();
        // Two stages per probe, so a failure says *where*: the lookup/destructure, or the call.
        let mut expected: Vec<String> = probes
            .iter()
            .flat_map(|(name, _)| [format!("{name}:resolved"), format!("{name}:called")])
            .collect();
        expected.sort();
        assert_eq!(
            tags, expected,
            "every vendored contract must be installed, resolvable by its constant URI, and callable"
        );
    });
}

/// The wallet's first governance step, on a fresh chain and with no bootstrap: resolve the master
/// read cap by its constant key, get the directory to answer `GetMe`, call it, and **receive the
/// deployer's stuff** — the whole handshake, in-process.
///
/// This is what `scripts/bootstrap-rgov.ts` used to arrange at runtime (with a recorded URI that
/// goes stale per chain). Genesis arranges it instead, for the fixed testnet key, so a client
/// hardcodes `masterdict_resolve_uri()` and needs no bootstrap.
///
/// Two shapes here are load-bearing, and both were learned the hard way:
///
/// - **The feature's log channel is forwarded, not drained.** `getMe`'s control flow has exactly one
///   trace — its own log lines — and a drain throws them away, so a stall and a broken client look
///   identical. Forwarding them to `@"out"` is what makes the assertions below able to name *where*
///   the flow stopped instead of only that it did.
/// - **A fired receive is not an answer.** `MCAread!("GetMe", …)` goes through `Directory.rho:43`'s
///   `read(@key, return)`, which replies `*map.get(key)` — i.e. `Nil` when the key is absent — and a
///   bare `for (GetMe <- …)` pattern matches `Nil`, so "the directory answered" is true either way.
///   The assertions therefore read the *value* (`getme-entry`) and the *reply* (`getme-answered`).
///
/// The ceremony-key leg is the other half: the feature's own deploy-time epilogue (`line 167`) calls
/// `getMe` for the key that deployed it, so if that key's lockers are written, the same flow a client
/// takes has already completed once on this chain.
#[test]
fn a_fresh_chain_serves_the_wallets_new_inbox_handshake() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();
        let resolve =
            rchain_casper::genesis::rgov::masterdict_resolve_uri().expect("the resolve facet key");

        // 1. The wallet's own path, as a client key distinct from the chain's ceremony key, so the
        //    deployer is a *new* member and the `createMe` branch is the one exercised.
        let handshake = r#"new rl(`rho:registry:lookup`), deployerId(`rho:rchain:deployerId`),
                 capCh, getMeCh, stuffCh
               in {
                 rl!(`RESOLVE`, *capCh) |
                 // Bind bare, call bare — the convention the rgov contracts themselves use
                 // (`memberIdGovRev`'s imports, the master-directory template). Mixing it with the
                 // wallet's `for (@X <- ch)` + `@X!` is a parse error, not a silent miss.
                 for (MCAread <- capCh) {
                   MCAread!("resolve", ["GetMe"], *getMeCh) |
                   for (GetMe <- getMeCh) {
                     @"out"!(["getme-entry", *GetMe]) |
                     new logCh in {
                       for (@line <= logCh) { @"out"!(["getme-log", line]) } |
                       GetMe!(*deployerId, *stuffCh, *logCh) |
                       for (@reply <- stuffCh) { @"out"!("getme-answered") }
                     }
                   }
                 }
               }"#
        .replace("RESOLVE", &resolve);

        // 2. The ceremony key's leg — the key the feature's epilogue bootstraps at genesis, and the
        //    key `tools/devnet.sh`'s client actually signs with.
        let ceremony_leg = r#"new rl(`rho:registry:lookup`), deployerId(`rho:rchain:deployerId`),
                 capCh, getMeCh, stuffCh
               in {
                 for (@i <<- @[*deployerId, "inbox"]) { @"out"!("bootstrap-inbox") } |
                 for (@d <<- @[*deployerId, "dictionary"]) { @"out"!("bootstrap-dictionary") } |
                 rl!(`RESOLVE`, *capCh) |
                 for (MCAread <- capCh) {
                   MCAread!("resolve", ["GetMe"], *getMeCh) |
                   for (GetMe <- getMeCh) {
                     new logCh in {
                       for (@line <= logCh) { Nil } |
                       GetMe!(*deployerId, *stuffCh, *logCh) |
                       for (@reply <- stuffCh) { @"out"!("bootstrap-answered") }
                     }
                   }
                 }
               }"#
        .replace("RESOLVE", &resolve);

        let mut terms = default_blessed_terms(
            &proof_of_stake(),
            &Registry {
                system_contract_pub_key: String::new(),
            },
            &[],
            "root",
            &ceremony_identity(),
        )
        .expect("blessed terms");
        terms.push(deploy_signed_by(&handshake, 11));
        terms.push(deploy_signed_by(&ceremony_leg, 7));

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
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString(
                    "out".to_string(),
                )),
            ))
            .await
            .expect("read the handshake's output channel");

        // The bare tags: the flow reached its end, on both legs.
        let tags: Vec<String> = produced
            .iter()
            .filter_map(|p| {
                rchain_models::rholang::RhoType::RhoString::unapply(p).map(str::to_string)
            })
            .collect();
        assert!(
            tags.contains(&"getme-answered".to_string()),
            "`getMe` must answer the client — the wallet's `newInbox` waits on exactly this reply \
             and gets silence otherwise: {tags:?}"
        );
        assert!(
            tags.contains(&"bootstrap-answered".to_string()),
            "`getMe` must answer the ceremony key too: {tags:?}"
        );
        assert!(
            tags.contains(&"bootstrap-inbox".to_string())
                && tags.contains(&"bootstrap-dictionary".to_string()),
            "the feature's deploy-time epilogue writes `@[*deployerId, \"inbox\"]` and \
             `@[*deployerId, \"dictionary\"]` for the key that deployed it (lines 173-174); those two \
             lockers are what eighteen of the wallet's snippets read: {tags:?}"
        );

        // The trace, tag by tag: every step of the feature's own control flow, named. A stall is
        // then a *missing* name rather than an absence of output.
        let mut logged_steps: Vec<String> = Vec::new();
        let mut getme_entry: Option<Par> = None;
        for p in &produced {
            let Some(items) = RhoList::unapply(p) else {
                continue;
            };
            match items.first().and_then(RhoString::unapply) {
                Some("getme-log") => {
                    if let Some(step) = items
                        .get(1)
                        .and_then(RhoList::unapply)
                        .and_then(|line| line.first())
                        .and_then(RhoString::unapply)
                    {
                        logged_steps.push(step.to_string());
                    }
                }
                Some("getme-entry") => getme_entry = items.get(1).cloned(),
                _ => {}
            }
        }
        for line in [
            "you don't exist",
            "creating your stuff",
            "creating you",
            "your inserted stuff",
        ] {
            assert!(
                logged_steps.iter().any(|s| s == line),
                "the feature must log {line:?} on its way through `getMe` -> `createMe`; a missing \
                 line names the step it stopped at. Logged: {logged_steps:?}"
            );
        }

        // And the value the directory holds: a real channel, not the `Nil` an absent key answers.
        let value = getme_entry.expect("the directory must answer `GetMe` with something");
        assert_ne!(
            value,
            Par::default(),
            "`GetMe` must resolve to the feature's channel, not `Nil` — a `Nil` is what a \
             registration that never happened looks like, and the wallet cannot tell it from broken"
        );
    });
}

/// The one order that matters, demonstrated: `MakeMint` resolves `rho:lang:nonNegativeNumber`
/// **during its own deploy** (`MakeMint.rho:27`), and `rho:registry:lookup` answers `Nil` when the
/// counter is not installed yet. Its install gate waits on a reply pattern a `Nil` cannot match, so
/// with the wrong order the deploy *succeeds* and `MakeMint` never registers — the silence this
/// whole task has been about. What ends that silence is the genesis ceremony's completeness check,
/// and this test pins both halves: the silence, and the check that turns it into a failed genesis.
///
/// (`roll` needs no such ordering: it resolves its `directory`/`inbox` imports per *call*, so its
/// position in the list is free — a negative test for it is what established that.)
#[test]
fn installing_make_mint_before_its_dependency_is_caught_by_the_genesis_check() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();
        use rchain_casper::genesis::standard_deploys::StandardDeploys;
        // `make_mint` before `non_negative_number`: the only difference from the blessed order.
        let terms = vec![
            StandardDeploys::list_ops("root").expect("list_ops"),
            StandardDeploys::make_mint("root").expect("make_mint"),
            StandardDeploys::non_negative_number("root").expect("non_negative_number"),
        ];

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
                "no deploy may fail — the misordering is silent, which is the point \
                 (deploy #{i}: {:?})",
                r.eval_result.errors
            );
        }

        // The class never registered, so its shorthand could not be seeded…
        let native =
            rchain_rholang::native_state::NativeSystemState::new(rm.runtime().native_store());
        let missing = rchain_casper::genesis::missing_genesis_aliases(&native)
            .await
            .expect("the completeness check runs");
        assert!(
            missing.contains(&"rho:rchain:makeMint"),
            "the misordered make_mint must be reported missing, got {missing:?}"
        );
        // …which is exactly what the genesis ceremony refuses to start with.
        assert!(
            !missing.is_empty(),
            "a chain that started here would answer `Nil` to that lookup forever"
        );
    });
}

/// **The publish facet is what replaced the grant capability, and a publish reads back.**
///
/// #71's stopgap published `grant` — a minter returning a writer bound to *one* key. Its job was to
/// answer "who may claim a name" without freezing an admission policy, and the rooted dictionary
/// (issue #99) answers that question by **derivation** instead: a publish is scoped to the caller's
/// own root, so there is nothing to claim and nothing to grant, and the `grantcap` key is gone.
///
/// The probe is written the way a consumer writes it: resolve `masterdict_publish_uri()`, publish
/// under your own root, and read the value back through `masterdict_resolve_uri()`. It reads the
/// *value* rather than asserting that a receive fired, because a call the dictionary cannot match is
/// not an error and law 38 makes silence indistinguishable from success — the trap that made #71
/// expensive to diagnose from outside.
#[test]
fn the_publish_facet_is_self_scoped_and_reads_back() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();
        let resolve =
            rchain_casper::genesis::rgov::masterdict_resolve_uri().expect("the resolve facet key");
        let publish =
            rchain_casper::genesis::rgov::masterdict_publish_uri().expect("the publish facet key");
        // The caller's own root: the address its deployer id derives to.
        let own = format!("{}/probeKey", rev_address_of(11));
        let term = r#"new rl(`rho:registry:lookup`), deployerId(`rho:rchain:deployerId`),
                 pcCh, rcCh, ack, readCh
               in {
                 rl!(`PUBLISH`, *pcCh) |
                 rl!(`RESOLVE`, *rcCh) |
                 for (pub <- pcCh; reader <- rcCh) {
                   pub!("publish", [*deployerId, "OWN", "probeValue"], *ack) |
                   for (_ <- ack) {
                     reader!("resolve", ["OWN"], *readCh) |
                     for (@v <- readCh) {
                       if (v == Nil) { @"out"!("publish:read-nil") }
                       else { @"out"!("publish:read-value") }
                     }
                   }
                 }
               }"#
        .replace("PUBLISH", &publish)
        .replace("RESOLVE", &resolve)
        .replace("OWN", &own);
        let mut terms = default_blessed_terms(
            &proof_of_stake(),
            &Registry {
                system_contract_pub_key: String::new(),
            },
            &[],
            "root",
            &ceremony_identity(),
        )
        .expect("blessed terms");
        terms.push(deploy_signed_by(&term, 11));

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
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString(
                    "out".to_string(),
                )),
            ))
            .await
            .expect("read the probe's output channel");
        let tags: Vec<String> = produced
            .iter()
            .filter_map(|p| RhoString::unapply(p).map(str::to_string))
            .collect();

        assert!(
            !tags.contains(&"publish:read-nil".to_string()),
            "a publish under the caller's own root must land where the resolve facet reads, or an \
             application cannot register itself ever: {tags:?}"
        );
        assert!(
            tags.contains(&"publish:read-value".to_string()),
            "and it must read back as a value. The control is in the same probe — a `Nil` here is \
             the derived path not matching the caller's address, which is the silent failure this \
             test exists to catch: {tags:?}"
        );
    });
}

/// **The dictionary's admin handle is written by the key that installs it, and only that key can
/// reach it.** This is the surviving half of what #71's thread settled: "recover the capability and
/// publish it under a name an app can reach" and "make the operator's write path usable" are
/// different changes, and the second is the smaller one.
///
/// #71's parked datum held `{"read", "write", "grant"}` — a capability nothing could reach after
/// block 1. The rooted dictionary (issue #99) replaces it with a handle it writes **itself**, for
/// its own deployer: `{"read": resolveFacet, "write": publishFacet, "root": rootFacet}`. So the
/// datum is no longer a capability parked out of reach; it is the handshake the genesis terms that
/// follow the dictionary use — and `grant` is absent on purpose, because a rooted name is *derived*,
/// so there is nothing to grant.
///
/// Both halves are measured with the same probe text signed by two keys:
///
/// - **the ceremony key reaches it** — the datum sits on `@[*deployerId, "MasterContractAdmin"]`,
///   keyed by that key, and it is read with `<<-` (a *peek*), so nothing consumes it;
/// - **no other key can** — a deploy signed by a stranger keys the channel to *its own* id and
///   matches nothing. So the handle is **operator-held and application-immutable**, which is the
///   state issue #71 reports.
///
/// The stranger's probe announces that it *started* before it tries, and the absence assertions sit
/// beside those start tags: without them, "nothing happened" and "the deploy never ran" are the same
/// observation, which is law 38 and the exact trap this whole family keeps setting.
#[test]
fn only_the_ceremony_key_can_reach_the_admin_handle() {
    with_big_stack(async {
        let rm = build_runtime_manager().await;
        let rand = fixed_rand();

        let probe = |tag: &str| {
            r#"new deployerId(`rho:rchain:deployerId`) in {
                     @"out"!("probe:TAG:start") |
                     for (@{"read": *R, "write": *W, "root": *A, ..._} <<- @[*deployerId, "MasterContractAdmin"]) {
                       @"out"!("probe:TAG:handle")
                     }
                   }"#
            .replace("TAG", tag)
        };

        let mut terms = default_blessed_terms(
            &proof_of_stake(),
            &Registry {
                system_contract_pub_key: String::new(),
            },
            &[],
            "root",
            &ceremony_identity(),
        )
        .expect("blessed terms");
        // The ceremony key is `[7u8; 32]` (`ceremony_identity`); `deploy_signed_by` derives the
        // deployer from the seed, so these two differ in exactly the signer.
        terms.push(deploy_signed_by(&probe("ceremony"), 7));
        terms.push(deploy_signed_by(&probe("stranger"), 11));

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
                rchain_models::par_ops::from_expr(rchain_models::ast::Expr::GString(
                    "out".to_string(),
                )),
            ))
            .await
            .expect("read the probes' output channel");
        let tags: Vec<String> = produced
            .iter()
            .filter_map(|p| RhoString::unapply(p).map(str::to_string))
            .collect();

        // The control that makes the absence below mean something: both probes ran.
        for tag in ["probe:ceremony:start", "probe:stranger:start"] {
            assert!(
                tags.contains(&tag.to_string()),
                "both probes must have run, or an absent handle is indistinguishable from a deploy \
                 that never happened: {tags:?}"
            );
        }
        assert!(
            tags.contains(&"probe:ceremony:handle".to_string()),
            "the dictionary must park its admin handle for the key that installs it. The genesis \
             terms that follow read it with a `<<-` peek, so nothing consumes it: {tags:?}"
        );
        assert!(
            !tags.contains(&"probe:stranger:handle".to_string()),
            "no other key may reach it — the channel is keyed to the *caller's* deployerId, so a \
             stranger matches nothing. That is the application-immutability of #71, measured beside \
             the operator-held half above: {tags:?}"
        );
    });
}
