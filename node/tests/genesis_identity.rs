//! **Law 84 / #456 on real nodes: the genesis is a function of its inputs.**
//!
//! The unit test in `casper/src/engine/node_launch.rs` proves the ceremony's identity is the first
//! bond and that any other key is refused. What it cannot show is that two *ceremonies* — separate
//! data directories, separate processes, the same bonds, wallets and signing key — produce the same
//! block bytes. This does: each node creates its own genesis at boot, and their block hashes must
//! agree. Before the rule, each node's local key entered the sender and the blessed deploys'
//! signatures, so identical inputs produced different chains (the `rnode-c` fork).

mod common;

use std::time::Duration;

use common::{deploy_conf, free_ports, start, temp_dir, test_runtime};

use reqwest::Client;

async fn wait_for_status(client: &Client, base: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(resp) = client.get(format!("{base}/api/status")).send().await {
            if resp.status().is_success() {
                return;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the node never answered /api/status: {base}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The genesis block's content hash, read from the node's own API.
async fn genesis_hash(client: &Client, base: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(resp) = client.get(format!("{base}/api/blocks")).send().await {
            if let Ok(serde_json::Value::Array(blocks)) = resp.json::<serde_json::Value>().await {
                if let Some(genesis) = blocks
                    .iter()
                    .find(|b| b.get("blockNumber").and_then(|n| n.as_i64()) == Some(0))
                {
                    return genesis
                        .get("blockHash")
                        .and_then(|h| h.as_str())
                        .expect("a genesis block carries its hash")
                        .to_string();
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the node never produced a genesis block: {base}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Two ceremonies, identical inputs, one hash.
///
/// `deploy_conf` is the same fixture for both: the same validator key (which is the first and only
/// bond), the same funded wallet, the same parameters — only the data directory and the ports differ,
/// and neither is a genesis input. The run is sequential so the two nodes do not contend for memory.
#[test]
fn two_ceremonies_with_the_same_inputs_and_key_produce_one_genesis() {
    test_runtime().block_on(async {
        let mut hashes = Vec::new();
        for run in 0..2 {
            let dir = temp_dir(&format!("genesis-identity-{run}"));
            let ports = free_ports(5);
            let conf = deploy_conf(&dir, &ports);
            let node = start(&conf, ports[4], ports[0]).await;
            let base = format!("http://127.0.0.1:{}", ports[0]);
            let client = Client::new();
            wait_for_status(&client, &base).await;
            hashes.push(genesis_hash(&client, &base).await);

            node.request_stop();
            let served = tokio::time::timeout(Duration::from_secs(30), node.handle)
                .await
                .expect("a node asked to stop must return")
                .expect("the serve task must not panic");
            served.expect("a stop on the operator's word is not a server error");
            std::fs::remove_dir_all(&dir).ok();
        }

        assert_eq!(
            hashes[0], hashes[1],
            "the genesis hash must be a function of the inputs and the signing key, not of the data \
             directory or the host — #456's two-ceremony check"
        );
    });
}
