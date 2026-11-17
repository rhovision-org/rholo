//! **The LFS checkpoint restore gate.**
//!
//! This suite used to carry a local developer-allocation matrix (matching recipient, a different
//! recipient, changed wallet balances, missing descriptors). That matrix tests a *local config
//! claim*, not a network gate, and it lives at unit level —
//! `casper/tests/committed_bounds.rs::restore_guard_checks_the_developer_allocation_before_running`
//! and the config→claim wiring in `casper/tests/determinism.rs`. Testing it through a full two-node
//! checkpoint sync coupled a local guard to the handshake and made CI red for a reason that was not
//! the restore path.
//!
//! What is tested here is the mainnet gate: a cold joiner with only the bootstrap's checkpoint
//! source restores the committed state, validates the first post-anchor block, and — when bonded —
//! proposes a block the bootstrap finalises. The developer allocation is genesis native state (a
//! vesting account, not a vault), so it is asserted only as part of the restored state.
//!
//! These are two real in-process nodes over the protocol transport, not an in-process replay: the
//! joiner enters `NodeSyncing`, pulls the certificate and state roots, populates the DAG, and passes
//! the restore-completion guard before Running.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{
    deploy_conf, free_ports, joiner_conf, start_pair_after, temp_dir, test_runtime,
    wait_for_height, wait_for_sync_cleared, JoinerSpec, DEPLOYER_REV_ADDR,
};
use rchain_comm::peer_node::PeerNode;
use rchain_crypto::public_key::PublicKey;
use rchain_node::configuration::model::NodeConf;
use rchain_node::runtime::node_environment;
use rchain_rholang::native_state::DevVesting;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::refined::Port;

fn wallets(total_drops: &str) -> String {
    format!("{DEPLOYER_REV_ADDR},{total_drops}\n")
}

fn address(byte: u8) -> String {
    RevAddress::from_public_key(&PublicKey::new(vec![byte; 65]))
        .expect("REV address")
        .to_base58()
}

/// A bootstrap that creates a production genesis with a non-empty allocation, and produces blocks
/// when `autopropose` is on. The wallet total is above the 1B-REV crossover, so the percentage arm
/// (not just the floor) is the amount.
fn bootstrap_conf(
    dir: &Path,
    ports: &[u16],
    recipient: &str,
    wallet_total: &str,
    autopropose: bool,
) -> NodeConf {
    assert!(
        ports.len() >= 6,
        "need [http, admin-http, grpc-internal, protocol, grpc-external, discovery]"
    );
    let mut conf = deploy_conf(dir, &ports[0..5]);
    conf.peers_discovery.port = ports[5] as i32;
    // A valid address means the production genesis gate is satisfied.
    conf.dev_mode = false;
    conf.autopropose = autopropose;
    let wallets_path = dir.join("wallets.txt");
    std::fs::write(&wallets_path, wallets(wallet_total)).expect("write bootstrap wallets");
    let gbd = &mut conf.casper.shards.primary_mut().genesis_block_data;
    gbd.wallets_file = wallets_path.to_string_lossy().into_owned();
    gbd.dev_tax_address = recipient.to_string();
    gbd.dev_tax_vesting_blocks = 18_934_560;
    conf
}

/// The normal joining configuration: no local allocation claim (an absent claim is not a claim of
/// no allocation), the bootstrap's bond set as the trust anchor, and optionally the second
/// validator key.
fn joiner_spec(bonded: bool) -> JoinerSpec {
    JoinerSpec {
        recipient: None,
        wallets_rows: None,
        bonds: true,
        bonded,
    }
}

async fn wait_for_genesis(client: &reqwest::Client, base: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(resp) = client.get(format!("{base}/api/blocks")).send().await {
            if let Ok(serde_json::Value::Array(blocks)) = resp.json::<serde_json::Value>().await {
                if !blocks.is_empty() {
                    return;
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the bootstrap never produced a genesis block: {base}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// **The certifiable checkpoint is the readiness gate**, not a finalized height. A chain can have
/// finalized a block and still have no finalized fringe to certify (the bootstrap's
/// `/api/v1/checkpoint` answers 400); starting the joiner there spends its sync attempts on a
/// bootstrap that cannot answer. Poll the route a joiner actually uses.
async fn wait_for_certifiable_checkpoint(
    client: &reqwest::Client,
    base: &str,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let mut last;
    loop {
        match client.get(format!("{base}/api/v1/checkpoint")).send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.json::<serde_json::Value>().await {
                    Ok(body) if status.is_success() => {
                        let anchor = body["checkpoint"]["anchorHeight"].as_i64().unwrap_or(-1);
                        if anchor > 0 {
                            return body;
                        }
                        last = format!("HTTP {status}, no anchor height: {body}");
                    }
                    Ok(body) => last = format!("HTTP {status}: {body}"),
                    Err(error) => last = format!("HTTP {status}, invalid JSON: {error}"),
                }
            }
            Err(error) => last = format!("request failed: {error}"),
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the bootstrap never published a certifiable checkpoint: {base}; last read: {last}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Poll until the node's `/api/blocks` reports at least one block — for a joiner, that is the
/// restore's first visible effect. Before this, `/api/status` answers and `sync` may already read
/// null (the task has not started), so neither is a readiness signal.
async fn wait_for_blocks(client: &reqwest::Client, base: &str, joiner: &common::TestNode) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let last_read = match client.get(format!("{base}/api/blocks")).send().await {
            Ok(resp) => {
                let status = resp.status();
                match resp.json::<serde_json::Value>().await {
                    Ok(serde_json::Value::Array(blocks)) if !blocks.is_empty() => return,
                    Ok(body) => format!("HTTP {status}, body: {body}"),
                    Err(error) => format!("HTTP {status}, invalid JSON: {error}"),
                }
            }
            Err(error) => format!("request failed: {error}"),
        };
        if tokio::time::Instant::now() >= deadline {
            let messages = joiner.log.messages();
            panic!(
                "the joiner never restored a block: {base}; last read: {last_read}; joiner log tail: {:?}",
                messages.into_iter().rev().take(32).collect::<Vec<_>>()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_finalized(client: &reqwest::Client, base: &str, height: i64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        if let Ok(resp) = client
            .get(format!("{base}/api/last-finalized-block"))
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(finalized) = resp.json::<serde_json::Value>().await {
                    if finalized["blockInfo"]["blockNumber"]
                        .as_i64()
                        .is_some_and(|n| n >= height)
                    {
                        return;
                    }
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the bootstrap never finalized height {height}: {base}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Make a later **finalized** block: autopropose's dummy `Nil` deploy has no state transition, so
/// the attestation guard publishes no layer (`recovery_artefacts.rs::wait_for_finality` says why).
/// A state-changing deploy is what gives the finalizer something to carry.
async fn deploy_for_snapshot(dir: &Path, ports: &[u16], client: &reqwest::Client, base: &str) {
    use rchain_casper::protocol::client::{DeployRuntime, GrpcDeployService};
    use rchain_crypto::private_key::PrivateKey;
    use rchain_shared::base16;

    let rho = dir.join("snapshot.rho");
    std::fs::write(&rho, "@\"snapshot\"!(\"state\")\n").expect("write rho source");
    // **The bootstrap's HTTP readiness is not its gRPC readiness.** A loaded CI runner has refused
    // the deploy connection here while the HTTP status already answered; the deploy is idempotent
    // across attempts (it is never pooled twice), so a bounded retry replaces a hard failure that
    // said nothing about the code under test.
    let mut last_error = String::new();
    for _ in 0..20 {
        let deploy = match GrpcDeployService::connect(
            "127.0.0.1",
            ports[4] as i32,
            16 * 1024 * 1024,
        )
        .await
        {
            Ok(deploy) => deploy,
            Err(e) => {
                last_error = e;
                tokio::time::sleep(Duration::from_millis(300)).await;
                continue;
            }
        };
        match DeployRuntime::deploy_file_program(
            &deploy,
            1_000_000,
            1,
            -1,
            &PrivateKey::new(base16::decode(common::VALIDATOR_PRIV_HEX).expect("decode key")),
            rho.to_str().expect("rho path"),
            "/root",
        )
        .await
        {
            Ok(()) => {
                wait_for_finalized(client, base, 1).await;
                return;
            }
            Err(e) => last_error = format!("{e:?}"),
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    panic!("deploy for snapshot refused after 20 attempts: {last_error}");
}

/// Wait for the launch phase's own Running line. `wait_for_sync_cleared` can return before the
/// restore-completion guard has run, so a one-shot log read here races the guard.
async fn wait_for_running(node: &common::TestNode) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let messages = node.log.messages();
        if messages
            .iter()
            .any(|line| line.contains("Making a transition to Running state."))
        {
            return;
        }
        if let Some(error) = messages
            .iter()
            .find(|line| line.contains("NodeLaunch exited with error"))
        {
            panic!("the joiner did not enter Running: {error}");
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the joiner never entered Running; log tail: {:?}",
            messages.into_iter().rev().take(12).collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn latest_height(client: &reqwest::Client, base: &str) -> i64 {
    let status: serde_json::Value = client
        .get(format!("{base}/api/status"))
        .send()
        .await
        .expect("GET /api/status")
        .json()
        .await
        .expect("/api/status json");
    status["latestBlockNumber"].as_i64().unwrap_or(-1)
}

async fn latest_post_state_hash(client: &reqwest::Client, base: &str) -> String {
    // **A metadata-only joiner serves no body above its checkpoint** (sync/verification
    // separation): `/api/blocks` reads bodies, so the tip is absent until the backfill reaches it.
    // The genesis body is the one the certified path always fetches and verifies, and it is where
    // the developer allocation lives — so fall back to height 0 when the tip has no body.
    let blocks: serde_json::Value = client
        .get(format!("{base}/api/blocks"))
        .send()
        .await
        .expect("GET /api/blocks")
        .json()
        .await
        .expect("/api/blocks json");
    let tip = blocks
        .as_array()
        .and_then(|blocks| blocks.first())
        .filter(|block| block["postStateHash"].is_string());
    let block = match tip {
        Some(block) => block.clone(),
        None => {
            let genesis: serde_json::Value = client
                .get(format!("{base}/api/blocks/0/0"))
                .send()
                .await
                .expect("GET /api/blocks/0/0")
                .json()
                .await
                .expect("/api/blocks/0/0 json");
            genesis
                .as_array()
                .and_then(|blocks| blocks.first())
                .cloned()
                .expect("the joiner serves the genesis body")
        }
    };
    block["postStateHash"]
        .as_str()
        .expect("a block with a postStateHash")
        .to_string()
}

/// **The join contract.** `syncPhase` is Running, the node runs from a checkpoint, and its
/// `latestBlockNumber` is strictly above the anchor it restored: the first post-anchor block was
/// validated. A height advance alone is not the claim; the anchor is.
async fn wait_for_join_contract(
    client: &reqwest::Client,
    admin_base: &str,
    joiner: &common::TestNode,
    timeout: Duration,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(resp) = client
            .get(format!("{admin_base}/api/v1/status/operator"))
            .send()
            .await
        {
            if let Ok(status) = resp.json::<serde_json::Value>().await {
                let phase = status["syncPhase"].as_str().unwrap_or("");
                let kind = status["checkpoint"]["kind"].as_str().unwrap_or("");
                let anchor = status["checkpoint"]["anchorHeight"].as_i64().unwrap_or(-1);
                let latest = status["latestBlockNumber"].as_i64().unwrap_or(-1);
                if phase == "running" && kind == "checkpoint" && anchor > 0 && latest > anchor {
                    return status;
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let messages = joiner.log.messages();
            panic!(
                "the joiner never validated the first post-anchor block; log tail: {:?}",
                messages.into_iter().rev().take(32).collect::<Vec<_>>()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn compare_leaves(joiner: Option<DevVesting>, bootstrap: Option<DevVesting>) {
    let joiner = joiner.expect("the restored joiner carries an allocation");
    let bootstrap = bootstrap.expect("the bootstrap genesis carries an allocation");
    assert_eq!(
        joiner.address, bootstrap.address,
        "the joiner must restore the bootstrap's recipient"
    );
    assert_eq!(
        joiner.allocation, bootstrap.allocation,
        "the joiner must restore the bootstrap's amount"
    );
    assert!(
        joiner.unreleased <= joiner.allocation,
        "a restored leaf cannot have released more than it holds"
    );
}

/// A checkpoint-restoring joiner that validates the first post-anchor block.
#[test]
fn a_checkpoint_joiner_restores_the_certified_state() {
    test_runtime().block_on(async {
        let boot_dir = temp_dir("lfs-restore-boot");
        let pair_ports = free_ports(12);
        let boot_ports = pair_ports[..6].to_vec();
        let join_ports = pair_ports[6..].to_vec();
        let recipient = address(2);
        let boot_conf = bootstrap_conf(
            &boot_dir,
            &boot_ports,
            &recipient,
            "1000000000000000000",
            true,
        );

        let boot_id = node_environment::create(&boot_conf).expect("bootstrap identity");
        let boot_peer = PeerNode::from(
            boot_id,
            "127.0.0.1".to_string(),
            Port::new(boot_ports[3]),
            Port::new(boot_ports[5]),
        );

        let join_dir = temp_dir("lfs-restore-join");
        let mut join_conf = joiner_conf(&join_dir, &join_ports, &boot_peer);
        join_conf.casper.checkpoint_source = Some(format!("http://127.0.0.1:{}", boot_ports[0]));
        joiner_spec(false).apply(&mut join_conf, &join_dir);

        let boot_base = format!("http://127.0.0.1:{}", boot_ports[0]);
        let client = reqwest::Client::new();
        let ready_client = client.clone();
        let ready_base = boot_base.clone();
        let ready_dir = boot_dir.clone();
        let ready_ports = boot_ports.clone();
        let (boot_node, join_node) = start_pair_after(
            (&boot_conf, boot_ports[4], boot_ports[0]),
            move || async move {
                wait_for_genesis(&ready_client, &ready_base).await;
                deploy_for_snapshot(&ready_dir, &ready_ports, &ready_client, &ready_base).await;
                wait_for_certifiable_checkpoint(&ready_client, &ready_base).await;
            },
            (&join_conf, join_ports[5], join_ports[0]),
        )
        .await;

        let join_base = format!("http://127.0.0.1:{}", join_ports[0]);
        let join_admin = format!("http://127.0.0.1:{}", join_ports[1]);
        wait_for_blocks(&client, &join_base, &join_node).await;
        wait_for_sync_cleared(&join_admin, Duration::from_secs(120)).await;
        wait_for_running(&join_node).await;
        wait_for_join_contract(&client, &join_admin, &join_node, Duration::from_secs(120)).await;

        let join_root = latest_post_state_hash(&client, &join_base).await;
        let boot_root = latest_post_state_hash(&client, &boot_base).await;
        compare_leaves(
            join_node.dev_vesting_at_hex(&join_root).await,
            boot_node.dev_vesting_at_hex(&boot_root).await,
        );

        join_node.request_stop();
        boot_node.request_stop();
        let _ = tokio::time::timeout(Duration::from_secs(30), join_node.handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(30), boot_node.handle).await;
    });
}

/// **A restored bonded joiner validates the chain and can propose on it.** The accepting test
/// above shows the joiner restores the committed state; this shows the restored state is usable for
/// the node's actual job, not merely readable. Both validators are bonded at genesis, so the
/// joiner's proposal is one the bootstrap can accept.
#[test]
fn a_checkpoint_restored_bonded_joiner_proposes_and_the_bootstrap_finalises_it() {
    test_runtime().block_on(async {
        let boot_dir = temp_dir("lfs-bonded-boot");
        let pair_ports = free_ports(12);
        let boot_ports = pair_ports[..6].to_vec();
        let join_ports = pair_ports[6..].to_vec();
        let recipient = address(2);
        let boot_conf = bootstrap_conf(
            &boot_dir,
            &boot_ports,
            &recipient,
            "1000000000000000000",
            true,
        );
        // The genesis bond set must include the joiner from the start; adding it after genesis
        // would not match the committed set.
        let bonds = boot_dir.join("bonds.txt");
        let mut rows = std::fs::read_to_string(&bonds).expect("read bootstrap bonds");
        rows.push_str(&format!(
            "{} 100\n",
            common::validator_pubkey_hex(common::SECOND_VALIDATOR_PRIV_HEX)
        ));
        std::fs::write(&bonds, rows).expect("write bootstrap bonds");

        let boot_id = node_environment::create(&boot_conf).expect("bootstrap identity");
        let boot_peer = PeerNode::from(
            boot_id,
            "127.0.0.1".to_string(),
            Port::new(boot_ports[3]),
            Port::new(boot_ports[5]),
        );

        let join_dir = temp_dir("lfs-bonded-join");
        let mut join_conf = joiner_conf(&join_dir, &join_ports, &boot_peer);
        join_conf.casper.checkpoint_source = Some(format!("http://127.0.0.1:{}", boot_ports[0]));
        joiner_spec(true).apply(&mut join_conf, &join_dir);

        let boot_base = format!("http://127.0.0.1:{}", boot_ports[0]);
        let client = reqwest::Client::new();
        let ready_client = client.clone();
        let ready_base = boot_base.clone();
        let ready_dir = boot_dir.clone();
        let ready_ports = boot_ports.clone();
        let (boot_node, join_node) = start_pair_after(
            (&boot_conf, boot_ports[4], boot_ports[0]),
            move || async move {
                wait_for_genesis(&ready_client, &ready_base).await;
                deploy_for_snapshot(&ready_dir, &ready_ports, &ready_client, &ready_base).await;
                wait_for_certifiable_checkpoint(&ready_client, &ready_base).await;
            },
            (&join_conf, join_ports[5], join_ports[0]),
        )
        .await;

        let join_base = format!("http://127.0.0.1:{}", join_ports[0]);
        let join_admin = format!("http://127.0.0.1:{}", join_ports[1]);
        wait_for_blocks(&client, &join_base, &join_node).await;
        wait_for_sync_cleared(&join_admin, Duration::from_secs(120)).await;
        wait_for_running(&join_node).await;
        wait_for_join_contract(&client, &join_admin, &join_node, Duration::from_secs(120)).await;

        let before = latest_height(&client, &join_base).await;
        let response = client
            .post(format!("{join_admin}/api/propose"))
            .send()
            .await
            .expect("POST /api/propose");
        assert!(
            response.status().is_success(),
            "the restored bonded joiner's propose failed: {}",
            response.status()
        );
        wait_for_height(&join_base, before + 1, Duration::from_secs(90)).await;
        // The bootstrap validates the joiner's block: its height reaches the same one.
        wait_for_height(&boot_base, before + 1, Duration::from_secs(90)).await;

        join_node.request_stop();
        boot_node.request_stop();
        let _ = tokio::time::timeout(Duration::from_secs(30), join_node.handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(30), boot_node.handle).await;
    });
}
