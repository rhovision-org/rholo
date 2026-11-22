//! Node-level gate cases for the cross-shard transaction routes (Laws 26–29).
//!
//! The routes exist on every node but only answer on a *gateway* with the API switched on; this file
//! pins both halves of that gate from a real config, and the per-shard chain progress that the
//! membership endpoint reports.

mod common;

use std::time::Duration;

use serde_json::Value;

use common::{
    free_ports, gateway_conf_with_txn_api, standalone_conf, start, temp_dir, VALIDATOR_PRIV_HEX,
};

/// Poll until the listener accepts a request. The admin listener is a second binding and can come up
/// after the public one; a test that reads it must wait for it rather than panic on a refused
/// connection.
async fn wait_for_listener(client: &reqwest::Client, base: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if client.get(base.to_string()).send().await.is_ok() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the listener {base} never accepted a request"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Poll until the node's HTTP surface answers.
async fn wait_for_http(client: &reqwest::Client, base: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(resp) = client.get(format!("{base}/version")).send().await {
            if resp.status().is_success() {
                return;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the HTTP surface never came up"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A gateway node with `api-server.enable-txn-api = false` serves the shard list but **not** the
/// transaction routes — the operator's switch, asserted through a real config on both listeners.
#[test]
fn the_txn_routes_404_when_the_api_is_disabled() {
    common::test_runtime().block_on(async {
        let dir = temp_dir("gateway-txn-disabled");
        let ports = free_ports(4);
        let http_port = ports[0];
        let conf = gateway_conf_with_txn_api(&dir, &ports, VALIDATOR_PRIV_HEX, false);
        let node = start(&conf, ports[2], http_port).await;
        let base = format!("http://127.0.0.1:{http_port}");
        let admin_base = format!("http://127.0.0.1:{}", node.admin_port);
        let client = reqwest::Client::new();
        wait_for_http(&client, &base).await;
        wait_for_listener(&client, &admin_base).await;

        // Still a gateway: the membership is reported.
        let shards: Value = client
            .get(format!("{base}/api/v1/shards"))
            .send()
            .await
            .expect("shards")
            .json()
            .await
            .expect("json");
        assert_eq!(shards["shardCount"], 2);

        // But the transaction routes are off — on both listeners, because the switch is a second gate
        // in front of the bind, not a substitute for it (AUDIT C121).
        for host in [&base, &admin_base] {
            assert_eq!(
                client
                    .get(format!("{host}/api/v1/txn"))
                    .send()
                    .await
                    .expect("list")
                    .status(),
                404,
                "GET /api/v1/txn on {host}"
            );
            assert_eq!(
                client
                    .get(format!("{host}/api/v1/txn/aabb"))
                    .send()
                    .await
                    .expect("status")
                    .status(),
                404,
                "GET /api/v1/txn/aabb on {host}"
            );
            assert_eq!(
                client
                    .post(format!("{host}/api/v1/txn"))
                    .json(&serde_json::json!({
                        "txnId": "aabb",
                        "legs": [{ "shardId": "/root", "amount": 1, "to": "d" }]
                    }))
                    .send()
                    .await
                    .expect("run")
                    .status(),
                404,
                "POST /api/v1/txn on {host}"
            );
        }

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// **The falsifier for AUDIT C121**, and the reason the previous test is not enough on its own: with
/// the transaction API *enabled* — the documented gateway configuration — the fund-moving route must
/// still be absent from the public listener and present only on the admin one.
///
/// On the tree before the fix the public `POST /api/v1/txn` answered `200` and, on a funded gateway,
/// escrowed and committed a transfer out of the node's own validator REV account to an address the
/// caller chose. So this asserts the status code *and* names what it would have meant.
#[test]
fn an_enabled_gateway_serves_the_txn_routes_only_on_the_admin_listener() {
    common::test_runtime().block_on(async {
        let dir = temp_dir("gateway-txn-public");
        let ports = free_ports(4);
        let http_port = ports[0];
        let conf = gateway_conf_with_txn_api(&dir, &ports, VALIDATOR_PRIV_HEX, true);
        let node = start(&conf, ports[2], http_port).await;
        let base = format!("http://127.0.0.1:{http_port}");
        let admin_base = format!("http://127.0.0.1:{}", node.admin_port);
        let client = reqwest::Client::new();
        wait_for_http(&client, &base).await;
        wait_for_listener(&client, &admin_base).await;

        // Public: 404, and this node's transaction API is on — the 404 is the route's absence, not a
        // feature flag, which is exactly what the fix changed.
        assert_eq!(
            client
                .get(format!("{base}/api/v1/txn"))
                .send()
                .await
                .expect("list")
                .status(),
            404,
            "the transaction list must not be reachable on the public listener"
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
                .expect("run")
                .status(),
            404,
            "a caller must not be able to open a transaction, and thereby spend the node's REV, \
             across the public listener"
        );

        // Admin: the same request is the node's own operator talking to the privileged listener, and
        // the list answers rather than 404ing — so the move did not disable the feature.
        let list = client
            .get(format!("{admin_base}/api/v1/txn"))
            .send()
            .await
            .expect("admin list");
        assert_eq!(list.status(), 200, "the admin listener serves the route");
        let value: Value = list.json().await.expect("json");
        assert!(value["inFlight"].is_array(), "in-flight list: {value}");

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// A single-shard node has no gateway, so the routes 404 — the same assertion `node_api.rs` makes,
/// repeated here beside the enabled-but-single-shard case so the two gates are read together.
#[test]
fn a_single_shard_node_serves_no_txn_routes() {
    common::test_runtime().block_on(async {
        let dir = temp_dir("gateway-single-shard");
        let ports = free_ports(4);
        let http_port = ports[0];
        // `standalone_conf` is the ordinary one-shard node; the flag cannot be set through this
        // helper, which is the point: a single-shard node is not a gateway regardless.
        let conf = standalone_conf(&dir, &ports, Some(VALIDATOR_PRIV_HEX));
        let node = start(&conf, ports[2], http_port).await;
        let base = format!("http://127.0.0.1:{http_port}");
        let admin_base = format!("http://127.0.0.1:{}", node.admin_port);
        let client = reqwest::Client::new();
        wait_for_http(&client, &base).await;
        wait_for_listener(&client, &admin_base).await;

        let shards: Value = client
            .get(format!("{base}/api/v1/shards"))
            .send()
            .await
            .expect("shards")
            .json()
            .await
            .expect("json");
        assert_eq!(shards["shardCount"], 1);
        assert_eq!(shards["primaryShard"], "/root");

        // No gateway on either listener: the routes answer 404 because there is no coordinator to
        // drive, which is the *other* gate from the operator's switch (AUDIT C121).
        for host in [&base, &admin_base] {
            assert_eq!(
                client
                    .get(format!("{host}/api/v1/txn"))
                    .send()
                    .await
                    .expect("list")
                    .status(),
                404,
                "GET /api/v1/txn on {host}"
            );
        }

        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// Each member shard's height is reported independently: a gateway node's chains do not move
/// together, which is what makes the membership endpoint worth reading.
#[test]
fn the_shard_list_reports_each_chain_height() {
    common::test_runtime().block_on(async {
        let dir = temp_dir("gateway-heights");
        let ports = free_ports(4);
        let http_port = ports[0];
        let conf = gateway_conf_with_txn_api(&dir, &ports, VALIDATOR_PRIV_HEX, true);
        let node = start(&conf, ports[2], http_port).await;
        let base = format!("http://127.0.0.1:{http_port}");
        let client = reqwest::Client::new();

        // Both shards reach their own genesis, each at height 1.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(resp) = client.get(format!("{base}/api/v1/shards")).send().await {
                if let Ok(value) = resp.json::<Value>().await {
                    if let Some(shards) = value["shards"].as_array() {
                        if shards.len() == 2
                            && shards
                                .iter()
                                .all(|s| s["latestBlockNumber"].as_i64().is_some_and(|n| n >= 1))
                        {
                            for shard in shards {
                                assert!(
                                    shard["shardId"].as_str().is_some(),
                                    "every member reports its own id: {shard}"
                                );
                            }
                            node.shutdown().await;
                            let _ = std::fs::remove_dir_all(&dir);
                            return;
                        }
                    }
                }
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "not every shard reached genesis"
            );
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    });
}
