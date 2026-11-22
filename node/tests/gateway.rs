//! Node-level gateway test: one `rnode` that is a member of two shards.
//!
//! Exercises the deferred "Layer 2" item end to end through the node's own HTTP surface: both
//! shards reach genesis under their own shard ids, the membership is reported, a deploy for a
//! non-member shard is refused, and a cross-shard transaction is coordinated by the node itself
//! (`POST /api/v1/txn`) rather than by an off-chain client.

mod common;

use std::time::Duration;

use rchain_rholang::util::rev_address::RevAddress;
use serde_json::{json, Value};

use common::{free_ports, gateway_conf, start, temp_dir, VALIDATOR_PRIV_HEX};

/// Poll an endpoint until it returns 200 with a JSON body.
async fn poll_json(client: &reqwest::Client, url: &str, what: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(resp) = client.get(url).send().await {
            if resp.status().is_success() {
                if let Ok(value) = resp.json::<Value>().await {
                    return value;
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} never became available at {url}"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Wait until **every** member shard has a chain.
///
/// The HTTP surface is up before the shards' genesis ceremonies finish (by design — a node serves
/// while it boots), so `/api/v1/shards` answering is not readiness. A shard reports
/// `latestBlockNumber: 0` with an empty DAG and `1` once its genesis block exists, which is the
/// signal a client can actually act on — transacting earlier races the genesis ceremony.
async fn wait_for_shards_ready(client: &reqwest::Client, base: &str) {
    let url = format!("{base}/api/v1/shards");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(value) = resp.json::<Value>().await {
                if let Some(shards) = value["shards"].as_array() {
                    let all_have_genesis = !shards.is_empty()
                        && shards
                            .iter()
                            .all(|s| s["latestBlockNumber"].as_i64().is_some_and(|n| n >= 1));
                    if all_have_genesis {
                        return;
                    }
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "not every member shard reached genesis within the deadline"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Two shards in one node: both reach genesis under their **own** full shard id, and the membership
/// is reported with the primary first.
#[test]
fn gateway_node_runs_two_shards_with_their_own_genesis() {
    common::test_runtime().block_on(async {
        let dir = temp_dir("gateway-two-shards");
        let ports = free_ports(4);
        let http_port = ports[0];
        let conf = gateway_conf(&dir, &ports, VALIDATOR_PRIV_HEX);
        assert_eq!(conf.casper.shards.len(), 2, "two memberships");
        let node = start(&conf, ports[2], http_port).await;
        let base = format!("http://127.0.0.1:{http_port}");
        let client = reqwest::Client::new();

        wait_for_shards_ready(&client, &base).await;
        let shards = poll_json(&client, &format!("{base}/api/v1/shards"), "shards").await;
        assert_eq!(shards["shardCount"], 2);
        assert_eq!(shards["primaryShard"], "/root");
        assert_eq!(shards["shards"][0]["shardId"], "/root");
        assert_eq!(shards["shards"][0]["primary"], true);
        assert_eq!(shards["shards"][1]["shardId"], "/root/child");
        assert_eq!(shards["shards"][1]["primary"], false);
        // Each shard has its own chain: `/api/v1/shards` only reports a height once that shard's
        // genesis block exists, so a genesis stamped with the bare name — matching no membership —
        // would leave a shard stuck at 0.
        for shard in shards["shards"].as_array().unwrap() {
            assert!(
                shard["latestBlockNumber"].as_i64().is_some_and(|n| n >= 1),
                "each shard must have its own genesis: {shard}"
            );
        }
        // `/api/status` still reports the primary shard's id (unchanged for existing tooling).
        let status = poll_json(&client, &format!("{base}/api/status"), "status").await;
        assert_eq!(status["shardId"], "/root");

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// **Every member shard's health reaches the operator surface, on a real two-shard node**
/// (W-N10-03's scope half and W-N10-04's surface half, live).
///
/// The unit arms pin the cells and the router; this is the one the rows actually point at — a node
/// that is genuinely a member of two shards, read from outside through its own admin listener. Before
/// the fix the operator status carried **one** shard's numbers as the node's, and there was no way to
/// see the other member at all.
///
/// **Why it can fail.** `shards` must be total over the memberships the node boots (two here), each
/// entry must carry its own proposer and finality reading, and `countersDurable` must be true — which
/// it only is if the boot read reached the shard's record, so this also exercises W-N10-02's seed on
/// a real store rather than an in-memory fixture.
#[test]
fn the_operator_status_reports_every_member_shards_health() {
    common::test_runtime().block_on(async {
        let dir = temp_dir("gateway-per-shard-health");
        let ports = free_ports(4);
        let http_port = ports[0];
        let admin_port = ports[1];
        let conf = gateway_conf(&dir, &ports, VALIDATOR_PRIV_HEX);
        let node = start(&conf, ports[2], http_port).await;
        let base = format!("http://127.0.0.1:{http_port}");
        let admin = format!("http://127.0.0.1:{admin_port}");
        let client = reqwest::Client::new();

        wait_for_shards_ready(&client, &base).await;
        let op = poll_json(
            &client,
            &format!("{admin}/api/v1/status/operator"),
            "operator status",
        )
        .await;

        // The flat fields are the **primary's** reading, which is what the row is about: on this node
        // they describe `/root` and say nothing about `/root/child`.
        assert_eq!(op["shardId"], "/root");
        // …and the map is where every member is.
        let shards = op["shards"]
            .as_object()
            .expect("the per-shard map is present and is an object");
        assert_eq!(
            shards.len(),
            2,
            "**total** over the node's memberships: {shards:?}"
        );
        let mut ids: Vec<&String> = shards.keys().collect();
        ids.sort();
        assert_eq!(ids, vec!["/root", "/root/child"]);
        for (id, entry) in shards {
            assert!(
                entry["proposer"]["autoproposeTimerHalted"].is_boolean(),
                "{id}: the member's own proposer reading: {entry}"
            );
            assert!(
                entry["finality"]["stallEpisodes"].is_number(),
                "{id}: …and its own finality reading: {entry}"
            );
            assert_eq!(
                entry["finality"]["countersDurable"], true,
                "{id}: the boot seed reached this shard's record, so its counters are its own history \
                 rather than this process's"
            );
            // The proposer entry carries the halt record too, so a halted member is readable as one.
            assert!(
                entry["proposer"]["autoproposeHalt"].is_object(),
                "{id}: the halt on record travels with each member: {entry}"
            );
        }

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// The node coordinates a cross-shard transaction itself: `POST /api/v1/txn` escrows on both of its
/// own shards and commits, and the durable record is readable back by id.
///
/// **Driven through the admin listener, and the public one is asserted to refuse the same request**
/// (AUDIT C121). The coordinator signs every phase deploy with the node's validator key and the escrow
/// comes out of that key's own REV account, so the route is a spend surface for the operator; before
/// this finding it was mounted on the public router, which binds `0.0.0.0` by default. The two
/// assertions together are the falsifier: the move preserved the feature *and* removed it from the
/// surface a remote caller can reach.
#[test]
fn gateway_node_coordinates_a_two_shard_transaction() {
    common::test_runtime().block_on(async {
        let dir = temp_dir("gateway-txn");
        let ports = free_ports(4);
        let http_port = ports[0];
        let conf = gateway_conf(&dir, &ports, VALIDATOR_PRIV_HEX);
        let node = start(&conf, ports[2], http_port).await;
        let base = format!("http://127.0.0.1:{http_port}");
        let admin_base = format!("http://127.0.0.1:{}", node.admin_port);
        let client = reqwest::Client::new();

        // Transacting before both shards have a chain would race their genesis ceremonies. The shard
        // list is a public read and stays on the public server.
        wait_for_shards_ready(&client, &base).await;

        let txn_id = "aabbccdd";
        // A real REV destination: `rho:txn` refuses a destination it cannot decode (C301).
        let destination = RevAddress::from_public_key(
            &rchain_casper::construct_deploy::default_pub2().expect("a fixed public key"),
        )
        .expect("a REV address")
        .to_base58();
        let body = json!({
            "txnId": txn_id,
            "legs": [
                { "shardId": "/root",       "amount": 30, "to": destination.clone() },
                { "shardId": "/root/child", "amount": 40, "to": destination.clone() }
            ]
        });

        // The falsifier, first: this node *is* a gateway with the transaction API enabled, and the
        // public listener must still have no such route. On the tree before C120/C121's fix this
        // answered 200 and moved the node's own REV.
        let public_attempt = client
            .post(format!("{base}/api/v1/txn"))
            .json(&body)
            .send()
            .await
            .expect("POST /api/v1/txn to the public port");
        assert_eq!(
            public_attempt.status(),
            404,
            "the fund-moving transaction route must not be reachable on the public listener, \
             whatever the gateway's configuration"
        );

        let resp = client
            .post(format!("{admin_base}/api/v1/txn"))
            .json(&body)
            .send()
            .await
            .expect("POST /api/v1/txn");
        let status = resp.status();
        let record: Value = resp.json().await.expect("record json");
        assert_eq!(status, 200, "record: {record}");
        assert_eq!(record["state"], "committed", "record: {record}");
        assert_eq!(record["txnId"], txn_id);
        assert_eq!(record["legs"].as_array().unwrap().len(), 2);
        let votes = record["votes"].as_array().unwrap();
        assert_eq!(votes.len(), 2);
        assert!(votes.iter().all(|v| v["vote"] == "ready"), "{votes:?}");
        assert!(
            record["recordHash"].as_str().is_some_and(|h| !h.is_empty()),
            "the record is content-addressed: {record}"
        );

        // The same record comes back by id, and a re-run is idempotent (no second escrow).
        let fetched = poll_json(
            &client,
            &format!("{admin_base}/api/v1/txn/{txn_id}"),
            "txn record",
        )
        .await;
        assert_eq!(fetched, record);
        let replay = client
            .post(format!("{admin_base}/api/v1/txn"))
            .json(&body)
            .send()
            .await
            .expect("re-run");
        let replayed: Value = replay.json().await.unwrap();
        assert_eq!(replayed, record, "a re-run returns the same record");

        // Nothing is left in flight.
        let in_flight = poll_json(
            &client,
            &format!("{admin_base}/api/v1/txn"),
            "in-flight list",
        )
        .await;
        assert_eq!(in_flight["inFlight"].as_array().unwrap().len(), 0);

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// A transaction naming a shard this node does not validate is refused, with the membership.
#[test]
fn gateway_node_refuses_a_non_member_shard() {
    common::test_runtime().block_on(async {
        let dir = temp_dir("gateway-non-member");
        let ports = free_ports(4);
        let http_port = ports[0];
        let conf = gateway_conf(&dir, &ports, VALIDATOR_PRIV_HEX);
        let node = start(&conf, ports[2], http_port).await;
        let base = format!("http://127.0.0.1:{http_port}");
        let admin_base = format!("http://127.0.0.1:{}", node.admin_port);
        let client = reqwest::Client::new();
        wait_for_shards_ready(&client, &base).await;

        let resp = client
            .post(format!("{admin_base}/api/v1/txn"))
            .json(&json!({
                "txnId": "deadbeef",
                "legs": [ { "shardId": "/elsewhere", "amount": 1, "to": "dest" } ]
            }))
            .send()
            .await
            .expect("POST /api/v1/txn");
        assert_eq!(resp.status(), 400);
        let err: Value = resp.json().await.unwrap();
        let err = err.as_str().unwrap_or_default();
        assert!(err.contains("/elsewhere"), "{err}");
        assert!(err.contains("/root, /root/child"), "{err}");

        // A malformed transaction id is a bad request, not a panic.
        let resp = client
            .post(format!("{admin_base}/api/v1/txn"))
            .json(&json!({ "txnId": "zz", "legs": [ { "shardId": "/root", "amount": 1, "to": "d" } ] }))
            .send()
            .await
            .expect("POST /api/v1/txn");
        assert_eq!(resp.status(), 400);

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}
