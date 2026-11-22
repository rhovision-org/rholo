//! **A lone dev-mode validator: what it produces on a deploy, and what it finalises** (issue #356).
//!
//! The playground is one dev-mode validator that proposes **on a deploy** and never on a timer
//! (`--propose-on-deploy`, no autopropose). A deploy arrives, the API answers `Success!`, and nothing
//! moves — while on the pre-upgrade binary the same deploy produced a block and finality advanced.
//!
//! Measured on the current build (2026-10-15); this file is the deterministic part of it.
//!
//! * **Production works**: a deploy anchored where the chain is produces a block, with no extra flags
//!   (`a_fresh_single_validator_chain_produces_on_a_deploy_without_extra_flags`). So n=1 is not itself
//!   the defect.
//! * **Finality does not follow it**, in that configuration, and the reason is a *trigger* rather than
//!   a rule: a round closes when a propose carries the attestation, and with autopropose off the only
//!   triggers are the deploy path and the not-due retry — which arms only on `NotEnoughNewBlocks`
//!   (`node/src/instances/proposer_instance.rs::not_due`), never after a propose that produced a
//!   block. `--attest-on-new-blocks` is a third, separate opt-in and is also off. So the fringe stays
//!   one short of the block: the shape the tree's own C209 note records from a live net ("the fringe
//!   stopped one layer short of the deploy").
//! * **With autopropose**, the same node produces *and* finalises
//!   (`a_lone_validator_with_autopropose_produces_and_finalises`). That is the configuration
//!   `tools/devnet.sh` ships with, and the one a lone chain wants.
//!
//! **A third behaviour was measured on a chain mined past `DEPLOY_LIFESPAN`** — and the node now
//! **refuses that deploy at admission** instead (the past half of the future-anchor screen, registered
//! as a deliberate Scala deviation in `spec/audit/passes.md` §6). The trace is kept because it is what
//! the refusal is for: on a settled lone node, a deploy the node cannot include did not merely go
//! unproposed, it produced no block at all and said nothing.
//!
//! ```text
//! settled at h=55, pool=0
//! stale deploy on a settled chain: h 55 -> 55, pool=1, status={"NotProcessed":{"status":"Pooled"}}
//! attestation at tip 54: withheld — nothing_to_finalize=true new_state_transition=false
//!                                     quorum_reachable=true cadence_due=false paced=false
//! propose refused: Failure: Proposal failed: NoNewDeploys (seqNum 55)
//! ```
//!
//! A deploy anchored at **zero** on a chain that tall is born expired (`valid_after <= height - 50`,
//! `casper/src/validate.rs::deploy_expired_at_block`). `select_deploys` skipped it as `expired` —
//! silently — and because a *settled* lone node withholds its attestation (`nothing_to_finalize =
//! true`, the #70 rule that a chain advances on content and not on wall-clock), not even the empty
//! attestation block was produced. The **first** clause is now refused at the door
//! (`a_deploy_anchored_behind_the_lifespan_is_refused_by_name`), and the **second** — a future anchor,
//! which is still admitted because the chain will reach it — is reported by the proposer's own warning
//! line (`report_nothing_to_include`), which is what turns this class of silence into something an
//! operator can read. So the height does not move **at all**: not a stall after proposing,
//! and not a refusal any caller can see, since `BlockApiImpl::deploy` discards the trigger's result
//! and the trigger's refusal is never logged. The tree names this trap in its own words
//! (`casper/src/protocol/client.rs::resolve_valid_after_block_number`: "born expired on any chain
//! taller than the lifespan … silently never proposed").

mod common;

use std::time::Duration;

use rchain_casper::protocol::client::{DeployService, GrpcDeployService};
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signed::Signed;
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_shared::base16;

use common::{deploy_conf, free_ports, temp_dir, test_runtime, VALIDATOR_PRIV_HEX};

async fn wait_for_genesis(base: &str) {
    let client = reqwest::Client::new();
    // Sixty seconds rather than the thirty the other node tests use: this file boots three nodes in
    // one binary (the harness serialises them), and on a loaded box a genesis has taken past thirty —
    // which shows up as a failure in a *wait*, not in an assertion. The wait is a condition, not a
    // speed claim.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(resp) = client.get(format!("{base}/api/blocks")).send().await {
            if resp.status().is_success() {
                if let Ok(serde_json::Value::Array(a)) = resp.json::<serde_json::Value>().await {
                    if !a.is_empty() {
                        return;
                    }
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "genesis never appeared: {base}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The chain's height, off the public status.
async fn height(base: &str) -> i64 {
    let status: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/api/status"))
        .send()
        .await
        .expect("GET /api/status")
        .json()
        .await
        .expect("status json");
    status["latestBlockNumber"].as_i64().unwrap_or(0)
}

/// The height the fringe has reached, or `-1` when the node has no fringe yet — the route answers the
/// sentence "Finalized fringe is not available." in that case, which is not a number to read as 0.
async fn finalized(base: &str) -> i64 {
    let v: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/api/last-finalized-block"))
        .send()
        .await
        .expect("GET /api/last-finalized-block")
        .json()
        .await
        .expect("last-finalized json");
    v["blockInfo"]["blockNumber"].as_i64().unwrap_or(-1)
}

/// Wait for the chain to reach `want`, or give up saying where it stopped.
async fn wait_for_height(base: &str, want: i64, secs: u64) -> i64 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let h = height(base).await;
        if h >= want {
            return h;
        }
        if tokio::time::Instant::now() >= deadline {
            return h;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Submit a deploy anchored at `valid_after`, and return its id (the signature, per the wire).
async fn submit(
    deploy: &GrpcDeployService,
    key: &PrivateKey,
    valid_after: i64,
    term: &str,
) -> Vec<u8> {
    let data = DeployData {
        attachments: Vec::new(),
        term: term.to_string(),
        timestamp: 0,
        phlo_price: 1,
        phlo_limit: 1_000_000,
        valid_after_block_number: valid_after,
        shard_id: "/root".to_string(),
    };
    let signed = Signed::new(data, &Secp256k1, key).expect("sign");
    let id = signed.sig.clone();
    deploy
        .deploy(&SignedDeployData {
            data: signed.data,
            deployer: signed.pk.bytes().to_vec(),
            sig: signed.sig,
            sig_algorithm: signed.sig_algorithm.name().to_string(),
        })
        .await
        .expect("the deploy API accepts it");
    id
}

/// **A deploy on a fresh single-validator chain produces a block, with no extra flags** (issue #356).
///
/// The first half of the close condition: `propose_on_deploy` is the only trigger — no autopropose, no
/// timer, and no explicit `propose` from the caller. The deploy is anchored at the chain's own height,
/// the way `rnode deploy` and the faucet anchor theirs.
///
/// The second half — *and finality advances past it* — is **not** met in this configuration, and the
/// test asserts that reading rather than hiding it: nothing carries the attestation (see the module
/// comment), so the fringe stays short of the block. The test beside it shows the same node closing the
/// round once `--autopropose` supplies a trigger.
#[test]
fn a_fresh_single_validator_chain_produces_on_a_deploy_without_extra_flags() {
    test_runtime().block_on(async {
        let dir = temp_dir("deploy-lifespan-fresh");
        let ports = free_ports(5);
        let mut conf = deploy_conf(&dir, &ports);
        conf.dev_mode = true;
        conf.dev.deployer_private_key = Some(VALIDATOR_PRIV_HEX.to_string());
        conf.propose_on_deploy = true;
        conf.autopropose = false;

        let node = common::start(&conf, ports[2], ports[0]).await;
        let base = format!("http://127.0.0.1:{}", ports[0]);
        wait_for_genesis(&base).await;

        let key = PrivateKey::new(base16::decode(VALIDATOR_PRIV_HEX).expect("decode the key"));
        let deploy = GrpcDeployService::connect("127.0.0.1", ports[4] as i32, 16 * 1024 * 1024)
            .await
            .expect("deploy gRPC");

        let h = height(&base).await;
        submit(&deploy, &key, h, "@\"playground\"!(\"fresh\")").await;

        let after = wait_for_height(&base, h + 1, 60).await;
        assert_eq!(
            after,
            h + 1,
            "**a deploy on a fresh single-validator chain produces a block with no extra flags** \
             (issue #356): the trigger is `propose_on_deploy`, and nothing else"
        );

        // The reading that is the finding: production works and the round does not close.
        let stalled = finalized(&base).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(
            finalized(&base).await,
            stalled,
            "finality has not advanced past the block in 10 s — recorded rather than hidden, because \
             with `--propose-on-deploy` and no autopropose nothing carries the attestation"
        );
        assert!(
            stalled <= h,
            "and the fringe is *short* of the deploy's block ({stalled} <= {h}): the two halves of \
             issue #356's title, on one chain"
        );

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// **With `--autopropose`, the same lone validator produces *and* finalises** — the control for the
/// test above, differing from it in exactly one flag.
///
/// The autopropose tap proposes on every validated block, so the attestation that closes the round has
/// a carrier. This is the configuration `tools/devnet.sh` ships with, and the one a lone chain wants.
#[test]
fn a_lone_validator_with_autopropose_produces_and_finalises() {
    test_runtime().block_on(async {
        let dir = temp_dir("deploy-lifespan-autopropose");
        let ports = free_ports(5);
        let mut conf = deploy_conf(&dir, &ports);
        conf.dev_mode = true;
        conf.dev.deployer_private_key = Some(VALIDATOR_PRIV_HEX.to_string());
        conf.propose_on_deploy = true;
        conf.autopropose = true;

        let node = common::start(&conf, ports[2], ports[0]).await;
        let base = format!("http://127.0.0.1:{}", ports[0]);
        wait_for_genesis(&base).await;

        let key = PrivateKey::new(base16::decode(VALIDATOR_PRIV_HEX).expect("decode the key"));
        let deploy = GrpcDeployService::connect("127.0.0.1", ports[4] as i32, 16 * 1024 * 1024)
            .await
            .expect("deploy gRPC");

        let h = height(&base).await;
        submit(&deploy, &key, h, "@\"playground\"!(\"with-autopropose\")").await;

        let mut reached = finalized(&base).await;
        for _ in 0..90 {
            if reached > h {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            reached = finalized(&base).await;
        }
        assert!(
            reached > h,
            "**a lone validator with autopropose closes the round past the deploy's block**: the \
             fringe reached {reached}, the deploy's block is at {}",
            h + 1
        );

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// **A deploy the node can never include is refused, by name** (issue #356).
///
/// The field case is an anchor at or below `tip - DEPLOY_LIFESPAN`: born expired on any chain taller
/// than the lifespan, and permanent — the height only grows, so it can never become includable.
/// Reaching that on a fifty-block chain would make this test a ladder, and the ladder races the round
/// gate; the same condition arrives the other way, with an anchor far *behind* a fresh chain. It is the
/// screen's own arithmetic (`valid_after <= next_block - DEPLOY_LIFESPAN`) and the screen's own
/// refusal that is asserted, so which end of the window the anchor sits at does not matter.
///
/// Falsified by removing the past half of the screen: the deploy is then accepted, held, and — as the
/// module doc records — never proposed.
#[test]
fn a_deploy_anchored_behind_the_lifespan_is_refused_by_name() {
    test_runtime().block_on(async {
        let dir = temp_dir("deploy-lifespan-refused");
        let ports = free_ports(5);
        let mut conf = deploy_conf(&dir, &ports);
        conf.dev_mode = true;
        conf.dev.deployer_private_key = Some(VALIDATOR_PRIV_HEX.to_string());

        let node = common::start(&conf, ports[2], ports[0]).await;
        let base = format!("http://127.0.0.1:{}", ports[0]);
        wait_for_genesis(&base).await;

        let key = PrivateKey::new(base16::decode(VALIDATOR_PRIV_HEX).expect("decode the key"));
        let deploy = GrpcDeployService::connect("127.0.0.1", ports[4] as i32, 16 * 1024 * 1024)
            .await
            .expect("deploy gRPC");

        let signed = Signed::new(
            DeployData {
                attachments: Vec::new(),
                term: "@\"never\"!(\"runs\")".to_string(),
                timestamp: 0,
                phlo_price: 1,
                phlo_limit: 1_000_000,
                valid_after_block_number: -1_000,
                shard_id: "/root".to_string(),
            },
            &Secp256k1,
            &key,
        )
        .expect("sign");

        let refused = deploy
            .deploy(&SignedDeployData {
                data: signed.data,
                deployer: signed.pk.bytes().to_vec(),
                sig: signed.sig,
                sig_algorithm: signed.sig_algorithm.name().to_string(),
            })
            .await
            .expect_err("**a deploy that can never be included is refused** (issue #356)");
        let text = refused.join(" ");
        assert!(
            text.contains("already expired"),
            "and the sentence says which rule it broke: {text}"
        );
        assert!(
            text.contains("-1000"),
            "naming the anchor that was refused: {text}"
        );
        assert!(
            text.contains("50"),
            "and the lifespan it is measured against: {text}"
        );

        // Nothing was pooled, so nothing can wedge later.
        let held: serde_json::Value = reqwest::Client::new()
            .get(format!("{base}/api/deploys"))
            .send()
            .await
            .expect("GET /api/deploys")
            .json()
            .await
            .expect("pool json");
        assert!(
            held["deploys"].as_array().is_some_and(|d| d.is_empty()),
            "a refused deploy is not held: {held}"
        );

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}
