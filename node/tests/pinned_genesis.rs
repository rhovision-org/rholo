//! On-disk genesis identity is rechecked across restarts; matching and mismatching
//! pins must lead to different admission outcomes on the same populated DAG.
//! This harness uses real node storage and launch, not only hash comparison.

mod common;

use std::time::Duration;

use common::{deploy_conf, free_ports, start, temp_dir, test_runtime};

async fn wait_for_log(node: &common::TestNode, needle: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let messages = node.log.messages();
        if messages.iter().any(|line| line.contains(needle)) {
            return;
        }
        if let Some(error) = messages
            .iter()
            .find(|line| line.contains("NodeLaunch exited with error"))
        {
            panic!("unexpected launch error while awaiting {needle}: {error}");
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "node never reported {needle}; log tail: {:?}",
            messages.into_iter().rev().take(12).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_refusal(node: &common::TestNode, expected_pin: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let messages = node.log.messages();
        if let Some(line) = messages.iter().find(|line| {
            line.contains("NodeLaunch exited with error")
                && line.contains("differs from pinned genesis")
        }) {
            assert!(line.contains(expected_pin), "refusal must name pin: {line}");
            assert!(
                !messages
                    .iter()
                    .any(|line| line.contains("Making a transition to Running state.")),
                "wrong-genesis node entered Running before refusing"
            );
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "wrong-genesis restart never refused; log tail: {:?}",
            messages.into_iter().rev().take(12).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn read_genesis_hash(base: &str) -> String {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        if let Ok(resp) = client.get(format!("{base}/api/blocks")).send().await {
            if let Ok(serde_json::Value::Array(blocks)) = resp.json::<serde_json::Value>().await {
                if let Some(block) = blocks
                    .iter()
                    .find(|b| b.get("blockNumber").and_then(|n| n.as_i64()) == Some(0))
                {
                    return block["blockHash"]
                        .as_str()
                        .expect("genesis block hash")
                        .to_string();
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "genesis block unavailable on {base}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[test]
fn populated_dag_restart_accepts_its_pin_and_refuses_a_different_pin() {
    test_runtime().block_on(async {
        let dir = temp_dir("pinned-genesis-restart");
        let initial_ports = free_ports(6);
        let base = format!("http://127.0.0.1:{}", initial_ports[0]);
        let mut conf = deploy_conf(&dir, &initial_ports);
        conf.peers_discovery.port = initial_ports[5] as i32;

        let canonical = {
            let node = start(&conf, initial_ports[4], initial_ports[0]).await;
            wait_for_log(&node, "Making a transition to Running state.").await;
            let hash = read_genesis_hash(&base).await;
            node.request_stop();
            tokio::time::timeout(Duration::from_secs(30), node.handle)
                .await
                .expect("original node must stop")
                .expect("node task must not panic")
                .expect("node must shut down cleanly");
            hash
        };

        // Reuse the *same on-disk DAG* but bind a fresh set of ephemeral ports.
        // Listener shutdown may outlive the serve handle briefly; rebinding its
        // previous protocol port would turn a persistence check into an EADDRINUSE
        // test of asynchronous listener teardown.
        let matching_ports = free_ports(6);
        let mut correct = deploy_conf(&dir, &matching_ports);
        correct.peers_discovery.port = matching_ports[5] as i32;
        correct.casper.expected_genesis_hash = Some(canonical.clone());
        {
            let node = start(&correct, matching_ports[4], matching_ports[0]).await;
            wait_for_log(&node, "Making a transition to Running state.").await;
            let restored_hash =
                read_genesis_hash(&format!("http://127.0.0.1:{}", matching_ports[0])).await;
            assert_eq!(
                restored_hash, canonical,
                "matching-pin restart must retain its genesis"
            );
            node.request_stop();
            tokio::time::timeout(Duration::from_secs(30), node.handle)
                .await
                .expect("matching-pin restart must stop")
                .expect("node task must not panic")
                .expect("node must shut down cleanly");
        }

        let wrong_pin = if canonical == "22".repeat(32) {
            "33".repeat(32)
        } else {
            "22".repeat(32)
        };
        let wrong_ports = free_ports(6);
        let mut wrong = deploy_conf(&dir, &wrong_ports);
        wrong.peers_discovery.port = wrong_ports[5] as i32;
        wrong.casper.expected_genesis_hash = Some(wrong_pin.clone());
        {
            let node = start(&wrong, wrong_ports[4], wrong_ports[0]).await;
            wait_for_refusal(&node, &wrong_pin).await;
            node.request_stop();
            let _ = tokio::time::timeout(Duration::from_secs(30), node.handle).await;
        }
        std::fs::remove_dir_all(&dir).ok();
    });
}
