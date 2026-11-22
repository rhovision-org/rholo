//! Deploy → block integration test.
//!
//! This is the coverage `cargo test --workspace` was missing: it boots a real standalone validator,
//! submits a signed deploy, and produces a block containing it. `node_api.rs` only checks *genesis*
//! over HTTP — it never exercises the block-production path, which is where the
//! `BugError (seqNum 1)` proposal failure lived.

mod common;

use std::time::Duration;

use rchain_casper::protocol::client::{
    build_par, DeployRuntime, DeployService, GrpcDeployService, GrpcProposeService, Name,
    ProposeService,
};
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::signatures::secp256k1::Secp256k1;
use rchain_crypto::signatures::signed::Signed;
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_models::casper::protocol::deploy_service::{BlocksQuery, DataAtNameQuery};
use rchain_shared::base16;

use common::{
    deploy_conf, free_ports, temp_dir, test_runtime, DEPLOYER_REV_ADDR, VALIDATOR_PRIV_HEX,
};

/// Poll the node's HTTP `/api/blocks` until the genesis block appears.
async fn wait_for_genesis(base: &str) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
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

/// **`--autopropose`: the chain grows with nothing sent to it.** The autopropose tap (which enqueues a
/// propose on every validated block) and the interval timer beside it (which covers the case where
/// nothing happens at all — a lone validator has no peers to send it blocks) are what a devnet runs
/// on. **No test set `autopropose`**, so both were unexecuted: `setup_shard_runtime`'s taps in
/// `node/src/runtime/node_runtime.rs` never ran their bodies.
///
/// This boots a standalone validator with it on, sends *nothing*, and waits for a second block. The
/// dev-mode dummy deploy is enabled because that is what the timer's own comment names as the source
/// of a block with no client deploy — without it a propose has nothing to propose.
#[test]
fn autopropose_grows_the_chain_without_a_deploy() {
    test_runtime().block_on(async {
        let dir = temp_dir("autopropose");
        let ports = free_ports(5);
        let mut conf = deploy_conf(&dir, &ports);
        conf.autopropose = true;
        conf.dev_mode = true;
        let node = common::start(&conf, ports[2] as u16, ports[0] as u16).await;
        let base = format!("http://127.0.0.1:{}", ports[0]);

        wait_for_genesis(&base).await;

        // `/api/blocks/:start/:end` is the height range (the depth form answers only the head), so it
        // is the read that can show growth. The timer's interval is 2s; 45s is slack for a loaded
        // machine, and the assertion names the count so a slow run is legible.
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let mut blocks = 0usize;
        loop {
            if let Ok(resp) = client.get(format!("{base}/api/blocks/0/16")).send().await {
                if let Ok(serde_json::Value::Array(a)) = resp.json::<serde_json::Value>().await {
                    blocks = blocks.max(a.len());
                    if blocks >= 2 {
                        break;
                    }
                }
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "autopropose produced no second block in 45s ({blocks} block(s))"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn deploy_is_processed_into_a_block() {
    test_runtime().block_on(async {
        let dir = temp_dir("deploy-block");
        let ports = free_ports(5); // http, admin-http, grpc-internal, protocol, grpc-external
        let conf = deploy_conf(&dir, &ports);
        let node = common::start(&conf, ports[2] as u16, ports[0] as u16).await;
        let base = format!("http://127.0.0.1:{}", ports[0]);

        wait_for_genesis(&base).await;

        // Deploy `@"hello"!("world")` (deployer = the funded wallet's key).
        let rho = dir.join("hello.rho");
        std::fs::write(&rho, "@\"hello\"!(\"world\")\n").expect("write rho source");
        let deploy = GrpcDeployService::connect("127.0.0.1", ports[4] as i32, 16 * 1024 * 1024)
            .await
            .expect("deploy gRPC");
        DeployRuntime::deploy_file_program(
            &deploy,
            1000000,
            1,
            -1,
            &PrivateKey::new(base16::decode(VALIDATOR_PRIV_HEX).expect("decode key")),
            rho.to_str().expect("rho path"),
            "/root",
        )
        .await
        .expect("deploy accepted");

        // Propose: the block-production path the suite was missing.
        let propose = GrpcProposeService::connect("127.0.0.1", ports[2] as i32, 16 * 1024 * 1024)
            .await
            .expect("propose gRPC");
        propose
            .propose(false)
            .await
            .expect("propose produced a block");

        // The proposed block (with the deploy) is in the DAG.
        let blocks = deploy
            .get_blocks(&BlocksQuery { depth: 5 })
            .await
            .expect("blocks");
        assert!(
            blocks.contains("block 1"),
            "proposed block missing from DAG:\n{blocks}"
        );

        // The deploy's send must be observable: query the `"hello"` channel and assert the `"world"`
        // datum was recorded (exercises the deploy_log → is_listening_name_reduced path).
        let query_par =
            build_par(&Name::PubName("\"hello\"".to_string())).expect("build query name");
        let data = deploy
            .listen_for_data_at_name(&DataAtNameQuery {
                depth: 50,
                name: query_par,
            })
            .await
            .expect("listen-data-at-name");
        let world = build_par(&Name::PubName("\"world\"".to_string())).expect("build world datum");
        assert!(
            data.iter().any(|d| d.post_block_data.contains(&world)),
            "expected `\"world\"` at `\"hello\"`, got: {data:?}"
        );

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// **A failed deploy's status carries the reason the reducer gave it** (`block_api_impl.rs`'s
/// `deploy_error_text`).
///
/// The Scala reads the message out of the node's per-node execution tracker and returns
/// `"<deploy error message not available in cache or deploy executed on another node>"` when it has
/// nothing — which is what a node that did not run the deploy always has. This port records the
/// reason in the block itself (`ProcessedDeploy.system_deploy_error`, issue #15), so the answer is
/// available to every node holding the block. Before this test the read path returned the placeholder
/// unconditionally, which is why a deployer could see a failure and never learn why.
///
/// The deploy is failed the cheapest deterministic way: a phlo limit of 1 against a term that costs
/// more, which the reducer reports as `OutOfPhlogistonsError`.
#[test]
fn a_failed_deploy_reports_its_reason_over_http() {
    test_runtime().block_on(async {
        let dir = temp_dir("deploy-error-status");
        let ports = free_ports(5); // http, admin-http, grpc-internal, protocol, grpc-external
        let conf = deploy_conf(&dir, &ports);
        let node = common::start(&conf, ports[2] as u16, ports[0] as u16).await;
        let base = format!("http://127.0.0.1:{}", ports[0]);
        wait_for_genesis(&base).await;

        // Sign the deploy here rather than through `deploy_file_program`, because the status route is
        // keyed by the deploy's *signature* and the helper does not return it.
        let key = PrivateKey::new(base16::decode(VALIDATOR_PRIV_HEX).expect("decode key"));
        let data = DeployData {
            attachments: Vec::new(),
            term: r#"@"never"!("runs")"#.to_string(),
            timestamp: 0,
            phlo_price: 1,
            // Too little phlo to reduce anything: the pre-charge succeeds, the deploy fails.
            phlo_limit: 1,
            valid_after_block_number: 0,
            shard_id: "/root".to_string(),
        };
        let signed = Signed::new(data, &Secp256k1, &key).expect("sign");
        let deploy_id = signed.sig.clone();
        let deploy = GrpcDeployService::connect("127.0.0.1", ports[4] as i32, 16 * 1024 * 1024)
            .await
            .expect("deploy gRPC");
        deploy
            .deploy(&SignedDeployData {
                data: signed.data,
                deployer: signed.pk.bytes().to_vec(),
                sig: signed.sig,
                sig_algorithm: signed.sig_algorithm.name().to_string(),
            })
            .await
            .expect("deploy accepted");

        let propose = GrpcProposeService::connect("127.0.0.1", ports[2] as i32, 16 * 1024 * 1024)
            .await
            .expect("propose gRPC");
        propose
            .propose(false)
            .await
            .expect("propose produced a block");

        // Poll the route until the deploy has been processed, then read the error text.
        let client = reqwest::Client::new();
        let url = format!("{base}/api/v1/deploy-status/{}", base16::encode(&deploy_id));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let error = loop {
            let json: serde_json::Value = client
                .get(&url)
                .send()
                .await
                .expect("GET deploy-status")
                .json()
                .await
                .expect("deploy-status json");
            // The **web** DTO's tags are PascalCase (`node/src/api/dto.rs`'s
            // `DeployExecStatus`, which is what law 43's envelope catalogue pins) while the
            // *casper* DTO of the same name renames them to camelCase — two types, one name, two
            // conventions. The route serves the web one.
            if let Some(error) = json
                .get("ProcessedWithError")
                .and_then(|e| e.get("deployError"))
                .and_then(|e| e.as_str())
            {
                break error.to_string();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the deploy never reached processedWithError: {json}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };

        assert_eq!(
            error, "Computation ran out of phlogistons.",
            "the status must carry the reducer's reason, not the placeholder the read path used to \
             return for every failed deploy"
        );
        assert!(
            !error.starts_with("<deploy error message not available"),
            "the placeholder is for a record with no message, and this block has one"
        );

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

async fn finalized_rev_balance(client: &reqwest::Client, base: &str, address: &str) -> Option<i64> {
    let term = format!(
        r#"new return, vault(`rho:rchain:revVault`), ret in {{ vault!("getBalance", "{address}", *ret) | for (@b <- ret) {{ return!(b) }} }}"#
    );
    let response = client
        .post(format!("{base}/api/v1/explore-deploy"))
        .json(&term)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: serde_json::Value = response.json().await.ok()?;
    body["expr"][0]["ExprInt"]["data"].as_i64()
}

/// Faucet acceptance requires a funded signer, ProcessedWithSuccess, finality past the drip block,
/// and an actual 30,000,000-drop balance delta in finalized state.
#[test]
fn faucet_delivery_is_successful_finalized_and_idempotent() {
    test_runtime().block_on(async {
        const RECIPIENT: &str = "1111pJu4TJaJDNJDTinnftr2fcHvMfnDeTRXRzwgPfwuKmGMa5juj";

        let dir = temp_dir("faucet-delivery");
        let ports = free_ports(5);
        let mut conf = deploy_conf(&dir, &ports);
        conf.dev_mode = true;
        conf.dev.deployer_private_key = Some(VALIDATOR_PRIV_HEX.to_string());
        conf.propose_on_deploy = true;
        conf.autopropose = true;

        let node = common::start(&conf, ports[2], ports[0]).await;
        let base = format!("http://127.0.0.1:{}", ports[0]);
        let client = reqwest::Client::new();
        wait_for_genesis(&base).await;

        let signer_deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let signer_balance = loop {
            if let Some(balance) = finalized_rev_balance(&client, &base, DEPLOYER_REV_ADDR).await {
                break balance;
            }
            assert!(
                tokio::time::Instant::now() < signer_deadline,
                "finalized state never became readable for the faucet signer"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        assert!(
            signer_balance >= 31_000_000,
            "faucet signer precondition failed: finalized balance {signer_balance} cannot cover drip + precharge"
        );

        let before = finalized_rev_balance(&client, &base, RECIPIENT)
            .await
            .expect("recipient balance from finalized state");
        assert!(before < 30_000_000, "recipient must be eligible before the drip");

        // **The caller is a stranger, and that is the point** (W-N4-03). The faucet is mounted on the
        // *public* listener (`web::http::router`), gated on dev mode plus a deployer key and on nothing
        // about who is asking: no credential is presented here and the request names a foreign Origin,
        // and the node still signs and submits a REV transfer from its own funded wallet. The row's
        // precondition is exactly this configuration, and this line is the falsifier: an unauthenticated
        // remote caller is served, with a drip it did not pay for.
        let drip = client
            .post(format!("{base}/api/v1/faucet"))
            .header("Origin", "http://evil.example")
            .header("Host", "evil.example")
            .json(&serde_json::json!({"address": RECIPIENT}))
            .send()
            .await
            .expect("POST faucet");
        assert_eq!(
            drip.status(),
            200,
            "faucet submission must be accepted — from an unauthenticated caller naming a foreign origin, \
             which is the row (W-N4-03)"
        );
        let drip: serde_json::Value = drip.json().await.expect("faucet response json");
        let deploy_id = drip["deployId"]
            .as_str()
            .expect("faucet response carries deployId")
            .to_string();

        let status_deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let drip_block = loop {
            let status: serde_json::Value = client
                .get(format!("{base}/api/v1/deploy-status/{deploy_id}"))
                .send()
                .await
                .expect("GET deploy-status")
                .json()
                .await
                .expect("deploy-status json");
            if let Some(error) = status.get("ProcessedWithError") {
                panic!("faucet deploy replayed with error: {error}");
            }
            if let Some(block_number) = status
                .get("ProcessedWithSuccess")
                .and_then(|s| s.get("block"))
                .and_then(|b| b.get("blockNumber"))
                .and_then(|n| n.as_i64())
            {
                break block_number;
            }
            assert!(
                tokio::time::Instant::now() < status_deadline,
                "faucet deploy never reached ProcessedWithSuccess: {status}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        };

        let finality_deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            let response = client
                .get(format!("{base}/api/last-finalized-block"))
                .send()
                .await
                .expect("GET last-finalized-block");
            if response.status().is_success() {
                let finalized: serde_json::Value = response
                    .json()
                    .await
                    .expect("last-finalized-block json");
                if finalized["blockInfo"]["blockNumber"]
                    .as_i64()
                    .is_some_and(|n| n >= drip_block)
                {
                    break;
                }
            }
            assert!(
                tokio::time::Instant::now() < finality_deadline,
                "faucet block {drip_block} never became finalized"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        let after = finalized_rev_balance(&client, &base, RECIPIENT)
            .await
            .expect("recipient balance after finality");
        assert_eq!(
            after - before,
            30_000_000,
            "faucet acceptance requires an actual finalized balance delta"
        );

        let retry = client
            .post(format!("{base}/api/v1/faucet"))
            .json(&serde_json::json!({"address": RECIPIENT}))
            .send()
            .await
            .expect("POST faucet retry");
        assert_eq!(retry.status(), 400, "a delivered drip is not granted twice");
        let reason: serde_json::Value = retry.json().await.expect("retry refusal json");
        assert!(
            reason
                .as_str()
                .is_some_and(|s| s.contains("already funded in finalized state")),
            "retry refusal must come from finalized chain eligibility: {reason}"
        );

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}
