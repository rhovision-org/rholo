//! **#420's contract half, replayed from the on-chain terms.** No node, no Docker, no merge.
//!
//! The live install's four Root-writing deploys are replayed in sequence through the proposer's own
//! `compute_state` on the deployed revision `71c6a513d` — `root.rho`, `admin_guard.rho`, `kernel.rho`,
//! `bounds.rho`, `governor.rho` — extracted verbatim from blocks 815/820/825/830/835 and committed
//! under `fixtures/n420/` (their SHA-256s are asserted below, so a fixture that drifts from the chain
//! is a test failure rather than a different experiment).
//!
//! The live key sets after those blocks were:
//!
//! ```text
//! 820  admin-guard
//! 825  admin-guard, verifier            (clock and codec did not land)
//! 830  admin-guard, verifier            (bounds did not land)
//! 835  admin-guard, governor, verifier
//! ```
//!
//! Each block here is a **single block with no merge**: one deploy applied to the previous post-state
//! by the sequential scheduler the deployed nodes run (`effect-scheduler = dfs`). So:
//!
//! - **the same sets without a merge** → the split is the contract's own behaviour, not a state-merge
//!   defect, and #420's state-integrity concern is a Root-contract finding;
//! - **all three kernel keys present** → the contract is sound in one block, and the live split needs
//!   the multi-block/merge environment — the replay moves to the fringe path.
//!
//! The one deliberate change to the root term is a capture line: after the registry insert, the local
//! URI is also emitted on `@"root-uri"`, because the local `insertArbitrary` URI is not the testnet's
//! and the kernel term resolves the root by that URI.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rchain_casper::merging::{BlockIndex, MergeScope, RejectionsMap};
use rchain_casper::runtime_manager::RuntimeManager;
use rchain_casper::system_deploy::SystemDeploy;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::hash::blake2b512_random::Blake2b512Random;
use rchain_crypto::public_key::PublicKey;
use rchain_models::ast::Par;
use rchain_models::block::state_hash::StateHash;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_rholang::merging::SidecarRecord;
use rchain_rholang::native_state::NativeSystemState;
use rchain_rholang::scheduler::EffectMode;
use rchain_rholang::system_processes::BlockData;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_rspace::merger::event_log_index::NumberChannelsDiff;
use rchain_shared::refined::NonNegI64;

use common::build_runtime_manager_with_mode;

/// The install's deployer — the `installer` key embedded in the on-chain root term. Public.
const INSTALLER: [u8; 65] = [
    0x04, 0xd0, 0xb8, 0xc0, 0xf3, 0x13, 0xa5, 0xde, 0x9a, 0x67, 0x68, 0xe8, 0x2a, 0x60, 0x73, 0xa4,
    0xed, 0x5f, 0x63, 0xd1, 0x75, 0xad, 0x4a, 0xd0, 0x4f, 0x5d, 0xf4, 0x9f, 0xa1, 0xb9, 0xf6, 0x82,
    0x4d, 0x77, 0x46, 0x21, 0xc1, 0x24, 0x8a, 0x69, 0x84, 0x58, 0xae, 0x8e, 0x75, 0x99, 0xf1, 0xf5,
    0xcd, 0x8f, 0xd6, 0xd5, 0xf7, 0x37, 0x95, 0xdc, 0xab, 0xa8, 0xc2, 0xb8, 0xc2, 0x9a, 0xd5, 0x14,
    0x0f,
];
/// The testnet root URI, as embedded in the on-chain terms. Replaced with the local URI.
const TESTNET_ROOT: &str = "rho:id:7srrj8is8zcz6de4p1oqsi87sj1fod7x4r5dzz8pf1fnnsy176co";

const ROOT_TERM: &str = include_str!("fixtures/n420/root.rho");
const ADMIN_GUARD_TERM: &str = include_str!("fixtures/n420/admin_guard.rho");
const KERNEL_TERM: &str = include_str!("fixtures/n420/kernel.rho");
const BOUNDS_TERM: &str = include_str!("fixtures/n420/bounds.rho");
const GOVERNOR_TERM: &str = include_str!("fixtures/n420/governor.rho");

fn fixed_rand() -> Blake2b512Random {
    Blake2b512Random::from_init(&[0u8; 32])
}

fn deploy(sig: u8, term: &str) -> SignedDeployData {
    SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: term.to_string(),
            timestamp: 0,
            phlo_price: 1,
            phlo_limit: 2_000_000,
            valid_after_block_number: 0,
            shard_id: "root".to_string(),
        },
        deployer: INSTALLER.to_vec(),
        sig: vec![sig; 65],
        sig_algorithm: "secp256k1".to_string(),
    }
}

/// Fund the installer's REV address so `pre_charge` passes.
async fn seed(rm: &RuntimeManager) -> Blake2b256Hash {
    let native = NativeSystemState::new(rm.runtime().native_store());
    let addr = RevAddress::from_public_key(&PublicKey::new(INSTALLER.to_vec()))
        .expect("the installer key is a valid address")
        .to_base58();
    native.set_vault_balance(
        &addr,
        NonNegI64::try_from(1_000_000_000_000).expect("a test balance"),
    );
    rm.runtime()
        .create_checkpoint()
        .await
        .expect("seeding checkpoint")
        .root
}

async fn step(
    rm: &RuntimeManager,
    pre: Blake2b256Hash,
    block_number: i64,
    term: &str,
    sig: u8,
) -> Blake2b256Hash {
    let block_data = BlockData {
        block_number: rchain_shared::refined::BlockHeight::try_from(block_number).expect("height"),
        sender: PublicKey::new(INSTALLER.to_vec()),
        seq_num: rchain_shared::refined::SeqNum::try_from(block_number).expect("seq"),
        timestamp: 0,
    };
    let (post, user, _sys) = rm
        .compute_state(
            &pre,
            &[deploy(sig, term)],
            &[],
            &fixed_rand(),
            block_data,
            &pre,
        )
        .await
        .expect("compute_state");
    assert!(
        user[0].eval_result.succeeded(),
        "deploy {sig:#x} failed: {:?}",
        user[0].eval_result.errors
    );
    post
}

async fn reply(rm: &RuntimeManager, state: Blake2b256Hash, term: &str) -> String {
    let reply = rm
        .play_exploratory_deploy(term, &StateHash::new(*state.as_bytes()))
        .await
        .expect("exploratory deploy");
    format!("{:?}", reply.data)
}

/// Every value on a channel at a state — the enumeration a contract peek cannot do.
async fn values_at(rm: &RuntimeManager, state: Blake2b256Hash, channel: &Par) -> Vec<Par> {
    rm.get_data(&StateHash::new(*state.as_bytes()), channel)
        .await
        .expect("get_data")
}

fn keys_term(uri: &str) -> String {
    format!(
        "new return, rl(`rho:registry:lookup`), ch, r in {{ rl!(`{uri}`, *ch) | \
         for (Root <- ch) {{ Root!(\"keys\", *r) | for (@x <- r) {{ return!(x) }} }} }}"
    )
}

fn keys(debug: &str) -> BTreeSet<String> {
    debug
        .split("GString(\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .filter(|s| s.starts_with("component:") || *s == "rcash:root")
        .map(str::to_string)
        .collect()
}

fn first_uri(debug: &str) -> Option<String> {
    debug
        .split("rho:id:")
        .nth(1)
        .map(|s| {
            s.chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .map(|s| format!("rho:id:{s}"))
}

/// The five fixture terms must be the chain's, byte for byte.
#[test]
fn the_fixtures_are_the_on_chain_terms() {
    fn sha(name: &str) -> String {
        // SHA-256, matching the hashes reported in the #420 evidence.
        use std::process::Command;
        let path = format!("{}/tests/fixtures/n420/{name}", env!("CARGO_MANIFEST_DIR"));
        let out = Command::new("sha256sum")
            .arg(path)
            .output()
            .expect("sha256sum");
        String::from_utf8(out.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    }
    assert_eq!(
        sha("root.rho"),
        "376918a93e1a9d56e95e998be3aa35c992e04197a68c03630f7d1caa1bdfb093"
    );
    assert_eq!(
        sha("admin_guard.rho"),
        "0b2a2c80700b9a137c147179bc17e8b2fa774f4e71fa471053f8c136b4b8cdca"
    );
    assert_eq!(
        sha("kernel.rho"),
        "16d51fdd0d1b513de528dff970253a05235d0762025cdc7137f2dce04b262378"
    );
    assert_eq!(
        sha("bounds.rho"),
        "4a99fb49aa23639044bdfbf603d3366a0978c241b244cd4ff1d4ba57258ddd15"
    );
    assert_eq!(
        sha("governor.rho"),
        "7c139b89f80dd76e73f6feed729ae74ebe507a71b5587d0d5883b74b45ceb91c"
    );
}

/// Run an async body on a worker thread with the node's 32 MiB stack — the blessed terms recurse
/// deeper than the 2 MiB test default while they parse and normalize.
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

/// The replay: root → admin-guard → kernel → bounds → governor, one block each, no merge.
#[ignore = "falsifier: the live split did not reproduce in one sequential block; see the boundary-merge replay"]
#[test]
fn the_live_root_key_sets_replay_without_a_merge() {
    with_big_stack(async {
        let rm = build_runtime_manager_with_mode(EffectMode::Sequential).await;
        let start = seed(&rm).await;

        // The root term, with one added capture line so the local registry URI is readable.
        let root_term = ROOT_TERM.replacen(
            "for (@r <- ack) { deployId!([uri]) }",
            "for (@r <- ack) { deployId!([uri]) | @\"root-uri\"!(uri) }",
            1,
        );
        assert_ne!(
            root_term, ROOT_TERM,
            "the capture line must have been inserted"
        );
        let root_post = step(&rm, start, 815, &root_term, 0x01).await;
        let uri_debug = reply(&rm, root_post, r#"for (@u <<- @"root-uri") { @"out"!(u) }"#).await;
        let uri = first_uri(&uri_debug).expect("the root URI is emitted");
        println!("local root URI: {uri}");

        let render = |text: &str| text.replace(TESTNET_ROOT, &uri);

        let admin_post = step(&rm, root_post, 820, &render(ADMIN_GUARD_TERM), 0x02).await;
        let admin_keys = keys(&reply(&rm, admin_post, &keys_term(&uri)).await);
        println!("820 admin_guard keys: {admin_keys:?}");
        assert!(
            admin_keys.contains("component:admin-guard"),
            "the control deploy landed"
        );

        let kernel_post = step(&rm, admin_post, 825, &render(KERNEL_TERM), 0x03).await;
        let kernel_keys = keys(&reply(&rm, kernel_post, &keys_term(&uri)).await);
        println!("825 kernel keys: {kernel_keys:?}");

        let bounds_post = step(&rm, kernel_post, 830, &render(BOUNDS_TERM), 0x04).await;
        let bounds_keys = keys(&reply(&rm, bounds_post, &keys_term(&uri)).await);
        println!("830 bounds keys: {bounds_keys:?}");

        let governor_post = step(&rm, bounds_post, 835, &render(GOVERNOR_TERM), 0x05).await;
        let governor_keys = keys(&reply(&rm, governor_post, &keys_term(&uri)).await);
        println!("835 governor keys: {governor_keys:?}");

        // The live sets, for the comparison the report is about:
        //   820 {admin-guard, rcash:root}
        //   825 {admin-guard, verifier, rcash:root}
        //   830 same as 825
        //   835 {admin-guard, governor, verifier, rcash:root}
        //
        // **The falsifier**: the live kernel set carries one of its three registers. If this replay —
        // one sequential block, no merge — lands all three, the contract is sound in one block and the
        // live split needs the multi-block/merge environment. The test failing is that finding, not a
        // broken instrument.
        assert!(kernel_keys.iter().any(|k| k == "component:verifier"));
        assert!(
        !kernel_keys.iter().any(|k| k == "component:clock")
            && !kernel_keys.iter().any(|k| k == "component:codec"),
        "the replay landed all three kernel keys in one sequential block: the live split needs the \
         merge, not the contract. kernel keys = {kernel_keys:?}"
    );
    });
}

// ---------------------------------------------------------------------------------------------------
// The live pre-state is a boundary-round merge: 820 is an epoch boundary, so the kernel's pre-state
// 9c6cf603… is the merge of the admin_guard block with three empty CloseBlock siblings. This test
// reproduces that merge with the real terms and runs the kernel on the result.
// ---------------------------------------------------------------------------------------------------

struct Played {
    hash: BlockHash,
    post: Blake2b256Hash,
    index: BlockIndex,
}

async fn play_block(
    rm: &RuntimeManager,
    hash_byte: u8,
    pre: Blake2b256Hash,
    block_number: i64,
    term: Option<&str>,
    system: &[SystemDeploy],
    sig: u8,
) -> Played {
    let block_data = BlockData {
        block_number: rchain_shared::refined::BlockHeight::try_from(block_number).expect("height"),
        sender: PublicKey::new(INSTALLER.to_vec()),
        seq_num: rchain_shared::refined::SeqNum::try_from(block_number).expect("seq"),
        timestamp: 0,
    };
    let terms: Vec<SignedDeployData> = term.map(|t| deploy(sig, t)).into_iter().collect();
    let (post, user, sys_results) = rm
        .compute_state(&pre, &terms, system, &fixed_rand(), block_data, &pre)
        .await
        .expect("compute_state");
    if let Some(u) = user.first() {
        assert!(
            u.eval_result.succeeded(),
            "deploy {sig:#x} failed: {:?}",
            u.eval_result.errors
        );
    }
    let usr: Vec<_> = user.iter().map(|u| u.deploy.clone()).collect();
    let sys: Vec<_> = sys_results.iter().map(|s| s.deploy.clone()).collect();
    let mergeable: Vec<NumberChannelsDiff> = user
        .iter()
        .map(|u| u.mergeable.clone())
        .chain(sys_results.iter().map(|s| s.mergeable.clone()))
        .collect();
    let sidecar = match rm
        .load_native_changes(post.as_bytes(), INSTALLER.as_ref(), block_number)
        .await
        .expect("sidecar read")
        .expect("the block's sidecar")
    {
        SidecarRecord::Attributed(e) => e,
        other => panic!("a played block's sidecar must be attributed; got {other:?}"),
    };
    let executor = RevAddress::from_public_key(&PublicKey::new(INSTALLER.to_vec()))
        .expect("executor")
        .to_base58();
    let index = BlockIndex::apply(
        BlockHash::new([hash_byte; 32]),
        &usr,
        &sys,
        pre,
        post,
        rm.get_history_repo(),
        &mergeable,
        sidecar,
        executor,
    )
    .await
    .expect("block index");
    Played {
        hash: BlockHash::new([hash_byte; 32]),
        post,
        index,
    }
}

#[test]
fn the_live_pre_state_boundary_merge_replays() {
    with_big_stack(async {
        let rm = build_runtime_manager_with_mode(EffectMode::Sequential).await;
        let start = seed(&rm).await;

        let root_term = ROOT_TERM
            .replacen(
                "for (@r <- ack) { deployId!([uri]) }",
                "for (@r <- ack) { deployId!([uri]) | @\"root-uri\"!(uri) }",
                1,
            )
            .replacen(
                "  rs!(bundle+{*Root}, *uriCh) |",
                "  @\"rootState-par\"!(*rootState) |\n  rs!(bundle+{*Root}, *uriCh) |",
                1,
            );
        let root_post = play_block(&rm, 0x41, start, 815, Some(&root_term), &[], 0x01)
            .await
            .post;
        let uri_debug = reply(&rm, root_post, r#"for (@u <<- @"root-uri") { @"out"!(u) }"#).await;
        let uri = first_uri(&uri_debug).expect("the root URI is emitted");
        let par_reply = rm
            .play_exploratory_deploy(
                r#"for (@p <<- @"rootState-par") { @"out"!(p) }"#,
                &StateHash::new(*root_post.as_bytes()),
            )
            .await
            .expect("the rootState name is readable");
        let root_state: Par = par_reply
            .data
            .first()
            .cloned()
            .expect("the rootState name is emitted");
        let render = |text: &str| text.replace(TESTNET_ROOT, &uri);

        // The 820 boundary round: the admin_guard block and three empty CloseBlock siblings.
        let admin_term = render(ADMIN_GUARD_TERM);
        let mut blocks = Vec::new();
        for i in 0..4 {
            let term = (i == 0).then_some(admin_term.as_str());
            let close = SystemDeploy::close_block(
                820,
                root_post,
                BTreeMap::new(),
                fixed_rand().split_byte(9),
            );
            blocks.push(play_block(&rm, 0x50 + i, root_post, 820, term, &[close], 0x10 + i).await);
        }
        let indexes: BTreeMap<BlockHash, Arc<BlockIndex>> = blocks
            .iter()
            .map(|b| (b.hash, Arc::new(b.index.clone())))
            .collect();
        let lookup = move |h: BlockHash| {
            let found = indexes.get(&h).cloned();
            async move { found.ok_or_else(|| format!("no index for {h:?}")) }
        };
        let scope = MergeScope {
            fork_blocks: BTreeSet::new(),
            final_scope: BTreeSet::new(),
            conflict_scope: blocks.iter().map(|b| b.hash).collect(),
            ancestry: BTreeMap::new(),
        };
        let outcome = MergeScope::merge(
            &scope,
            root_post,
            &RejectionsMap::new(),
            rm.get_history_repo(),
            &lookup,
            |_| 0,
        )
        .await
        .expect("the 820 boundary round merges");
        println!(
            "820 merge: equals_parent={:?} kept/rejected={}/{} final_scope_rejected={} \
             dropped_native_slots={}",
            blocks.iter().position(|b| b.post == outcome.state),
            outcome.report.kept_chains,
            outcome.report.rejected_chains,
            outcome.report.final_scope_rejected_chains,
            outcome.report.dropped_native_slots.len(),
        );
        // **The merge accounting, not the block-level `rejectedDeploys` field.** The conflict scope
        // is four blocks; the deploy block contributes a user chain and a system chain.
        let r = &outcome.report;
        assert_eq!(
            r.kept_chains + r.rejected_chains,
            r.conflict_chains,
            "every conflict-scope chain is either kept or rejected"
        );
        assert!(
            r.conflict_chains >= 4,
            "four blocks contribute at least one chain each"
        );
        assert_eq!(
            r.final_scope_rejected_chains, 0,
            "all four blocks are in the conflict scope here; the final-scope path is the control"
        );
        assert!(
            r.rejections_without_a_conflict.is_empty(),
            "I1: {:?}",
            r.rejections_without_a_conflict
        );
        assert!(
            r.unapplied_kept_writes.is_empty(),
            "I2: {:?}",
            r.unapplied_kept_writes
        );

        // Control: merge the deploy-carrying block alone (no empty siblings). Before the fix this
        // duplicated the Root value too; after it, the single-block merge must reproduce the
        // block's own post-state.

        let one_indexes: BTreeMap<BlockHash, Arc<BlockIndex>> =
            BTreeMap::from([(blocks[0].hash, Arc::new(blocks[0].index.clone()))]);
        let one_lookup = move |h: BlockHash| {
            let found = one_indexes.get(&h).cloned();
            async move { found.ok_or_else(|| format!("no index for {h:?}")) }
        };
        let one_scope = MergeScope {
            fork_blocks: BTreeSet::new(),
            final_scope: BTreeSet::new(),
            conflict_scope: BTreeSet::from([blocks[0].hash]),
            ancestry: BTreeMap::new(),
        };
        let one_outcome = MergeScope::merge(
            &one_scope,
            root_post,
            &RejectionsMap::new(),
            rm.get_history_repo(),
            &one_lookup,
            |_| 0,
        )
        .await
        .expect("the single-block merge");
        let one_values = values_at(&rm, one_outcome.state, &root_state).await;
        println!(
            "single-block merge of the deploy block: equals_parent={} rootState values={}",
            one_outcome.state == blocks[0].post,
            one_values.len()
        );
        assert_eq!(
            one_values.len(),
            1,
            "control: the deploy block's own post carries one Root value"
        );

        let pre_keys = keys(&reply(&rm, outcome.state, &keys_term(&uri)).await);
        let pre_values = values_at(&rm, outcome.state, &root_state).await;
        let admin_post_values = values_at(&rm, blocks[0].post, &root_state).await;
        println!(
            "admin block post rootState values={} | merged pre rootState values={} | values_equal={}",
            admin_post_values.len(),
            pre_values.len(),
            pre_values == admin_post_values
        );
        println!("merged pre keys: {pre_keys:?}");
        for (i, v) in pre_values.iter().enumerate() {
            println!("  pre value {i}: {:?}", keys(&format!("{v:?}")));
        }
        assert!(
            pre_keys.contains("component:admin-guard"),
            "control: the admin_guard write must survive the boundary merge"
        );

        // The kernel on the merged pre-state.
        let kernel_post = play_block(
            &rm,
            0x60,
            outcome.state,
            825,
            Some(&render(KERNEL_TERM)),
            &[],
            0x20,
        )
        .await
        .post;
        let kernel_keys = keys(&reply(&rm, kernel_post, &keys_term(&uri)).await);
        let kernel_values = values_at(&rm, kernel_post, &root_state).await;
        println!(
            "kernel on merged pre keys: {kernel_keys:?} rootState values={}",
            kernel_values.len()
        );
        for (i, v) in kernel_values.iter().enumerate() {
            println!("  kernel value {i}: {:?}", keys(&format!("{v:?}")));
        }

        let bounds_post = play_block(
            &rm,
            0x61,
            kernel_post,
            830,
            Some(&render(BOUNDS_TERM)),
            &[],
            0x21,
        )
        .await
        .post;
        let bounds_keys = keys(&reply(&rm, bounds_post, &keys_term(&uri)).await);
        let bounds_values = values_at(&rm, bounds_post, &root_state).await;
        println!(
            "bounds on kernel post keys: {bounds_keys:?} rootState values={}",
            bounds_values.len()
        );

        // **The invariant is the accepted value, not a bag.** Before the fix the 820 boundary merge
        // left two identical Root values; the merged pre-state must now carry exactly the value the
        // deploy-carrying block committed, and the kernel on it must still produce state.
        assert_eq!(
            admin_post_values.len(),
            1,
            "control: the admin_guard block's own post carries one Root value"
        );
        assert_eq!(
            pre_values, admin_post_values,
            "the 820 boundary merge did not preserve the accepted chain's Root value: \
             block post {admin_post_values:?}, merged {pre_values:?}"
        );
        assert_eq!(
            one_values, admin_post_values,
            "the single-block merge did not preserve the accepted Root value"
        );
        assert!(
            !kernel_values.is_empty(),
            "the kernel produced no Root value at all"
        );
    });
}
