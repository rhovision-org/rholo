//! Node-level integration tests: a real in-process standalone node driven over HTTP.
//!
//! Mirrors the Scala `integration-tests` suite's single-node scenarios (`test_genesis_ceremony`
//! slice, `test_web_api`).

mod common;

use std::time::Duration;

use serde_json::Value;

use common::{temp_dir, VALIDATOR_PRIV_HEX};

async fn poll_blocks(client: &reqwest::Client, url: &str) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
                Ok(Value::Array(a)) if !a.is_empty() => return a,
                Ok(_) => {}
                Err(_) => {}
            },
            _ => {}
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "blocks never appeared: {url}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Scala `test_web_api` (read-only surface): the HTTP server comes up and serves the
/// genesis-independent endpoints (`/version`, `/status`, `/api/status`) without a validator or a
/// genesis block.
#[tokio::test]
async fn http_surface_without_genesis() {
    let dir = temp_dir("http-surface");
    // Ports are allocated under the heavyweight lock, so this test cannot be handed a port an
    // earlier node bound, and the wait reports the node's own error if it stopped.
    let (mut node, ports) = common::start_standalone(&dir, None).await;
    let http_port = ports[0];
    let base = format!("http://127.0.0.1:{http_port}");
    let client = reqwest::Client::new();

    let version = common::wait_for_http(&mut node, &base).await;
    assert!(version.contains("rholo node"), "version = {version}");
    // A served node names the binary it is, not just its version number: `commit # unknown` is what
    // this route answered before the build script existed, and this string is the one a room member
    // reads to know which reducer produced an attestation (issue #32).
    assert!(
        !version.contains("commit # unknown"),
        "the served version must carry the build commit, got {version}"
    );

    // `/api/status` returns a JSON status document.
    let status = client
        .get(format!("{base}/api/status"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(status.get("networkId").is_some(), "api/status = {status}");

    node.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Scala `test_genesis_ceremony` (single-node slice) + `test_web_api`: a standalone validator node
/// boots, creates the genesis block, and exposes it over the HTTP API.
#[test]
fn genesis_boot_exposes_block_over_http() {
    common::test_runtime().block_on(async {
        let dir = temp_dir("genesis-boot");
        // `exploratory_deploy` is admitted on a read-only node or in dev mode
        // (`block_api_impl.rs:685`); a bonded validator is neither, and `start_standalone` builds a
        // dev-mode standalone node for the reason the devnet runs one.
        let (mut node, ports) = common::start_standalone(&dir, Some(VALIDATOR_PRIV_HEX)).await;
        let http_port = ports[0];
        let base = format!("http://127.0.0.1:{http_port}");
        let client = reqwest::Client::new();

        // The HTTP server comes up before genesis is created; poll until `/version` responds.
        let version = common::wait_for_http(&mut node, &base).await;
        assert!(version.contains("rholo node"), "version = {version}");
        assert!(
            !version.contains("commit # unknown"),
            "the served version must carry the build commit, got {version}"
        );

        // `GET /api/blocks` returns the genesis block once the genesis ceremony completes.
        let blocks = poll_blocks(&client, &format!("{base}/api/blocks")).await;

        let genesis = &blocks[0];
        // The genesis block carries the *full* shard id, not the bare shard name: the proposer, the
        // block receiver and the deploy API all use the full id, so a genesis block stamped with
        // "root" would carry an id no later block or deploy shares.
        assert_eq!(genesis["shardId"], "/root");

        // `GET /api/v1/shards` reports the node's memberships. A single-shard node has exactly one,
        // and it is the primary — the same shard `/api/status` and the genesis block report. (The
        // membership list is deliberately not folded into `/api/status`.)
        let shards_resp = client
            .get(format!("{base}/api/v1/shards"))
            .send()
            .await
            .expect("GET /api/v1/shards");
        assert_eq!(shards_resp.status(), 200);
        let shards: Value = shards_resp.json().await.expect("shards json");
        assert_eq!(shards["primaryShard"], "/root");
        assert_eq!(shards["shardCount"], 1);
        assert_eq!(shards["shards"][0]["shardId"], "/root");
        assert_eq!(shards["shards"][0]["primary"], true);

        // An exploratory deploy runs a term and returns what it produced (AUDIT C38/C39). Three things
        // are asserted at once, and each was wrong before:
        //   * the reply is read from `@"out"` — the channel every corpus, example and client writes
        //     to — and not only from the term's first `new`-bound name;
        //   * the response *says* which channel answered (`replySource`), so an empty `expr` is a
        //     fact rather than a guess;
        //   * the value is the reference document's shape — `{"ExprInt":{"data":42}}` — rather than
        //     the `{"ExprInt":42}` the port used to write, which no client could read.
        //
        // **Through the by-hash route, naming the genesis block explicitly.** AUDIT C129 made
        // `POST /api/v1/explore-deploy` read the last *finalized* block; #554 makes it fall back to
        // genesis when nothing is finalized, so the no-hash default answers on this node now too.
        // Naming the block here keeps this test pinned to the genesis state whatever arm of that
        // decision a later change takes; the no-hash default itself is exercised in
        // `api_surface.rs::the_read_routes_answer_and_their_refusals_are_defined`.
        // (Registered as C163: C129's commit changed the default and verified `--test api_surface`,
        // never this file, so this assertion had been failing unseen.)
        let explore = client
            .post(format!("{base}/api/v1/explore-deploy-by-block-hash"))
            .json(&serde_json::json!({
                "term": "@\"out\"!(42)",
                "blockHash": genesis["blockHash"],
                "usePreStateHash": false
            }))
            .send()
            .await
            .expect("POST /api/v1/explore-deploy-by-block-hash");
        assert_eq!(explore.status(), 200);
        let body: Value = explore.json().await.expect("explore json");
        assert_eq!(
            body["replySource"], "out",
            "the response must name the channel it read: {body}"
        );
        assert_eq!(
            body["expr"][0]["ExprInt"]["data"], 42,
            "and the value must be the reference's shape: {body}"
        );

        // The first `new`-bound name keeps working — a client following the reference node's own
        // convention sees no change.
        let named = client
            .post(format!("{base}/api/v1/explore-deploy-by-block-hash"))
            .json(&serde_json::json!({
                "term": "new result in { result!(7) }",
                "blockHash": genesis["blockHash"],
                "usePreStateHash": false
            }))
            .send()
            .await
            .expect("POST /api/v1/explore-deploy-by-block-hash (first private name)");
        assert_eq!(named.status(), 200);
        let body: Value = named
            .json()
            .await
            .expect("explore json (first private name)");
        assert_eq!(body["replySource"], "firstPrivateName", "{body}");

        // This node is not a gateway (one shard), so there is no cross-shard coordinator to drive.
        // Since AUDIT C121 the route is also **absent from the public router altogether** — a gateway's
        // transaction surface lives on the admin listener, which binds loopback by default — so this
        // 404 is doubly determined here, and what the assertion now pins is that the public surface did
        // not gain the route while it moved. A single-shard node's surface is otherwise unchanged.
        assert_eq!(
            client
                .get(format!("{base}/api/v1/txn"))
                .send()
                .await
                .expect("GET /api/v1/txn")
                .status(),
            404
        );
        assert_eq!(
            client
                .post(format!("{base}/api/v1/txn"))
                .json(&serde_json::json!({
                    "txnId": "aabb",
                    "legs": [{ "shardId": "/root", "amount": 1, "to": "d" }]
                }))
                .send()
                .await
                .expect("POST /api/v1/txn")
                .status(),
            404
        );

        // The genesis block has no justifications.
        assert_eq!(genesis["justifications"].as_array().unwrap().len(), 0);
        // ... and carries the single bonded validator with stake 100.
        let bonds = genesis["bonds"].as_array().unwrap();
        assert_eq!(bonds.len(), 1);
        assert_eq!(bonds[0]["stake"], 100);

        node.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    });
}
