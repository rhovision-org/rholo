//! **The developer allocation over default LFS restore.**
//!
//! Aria's matrix: matching inputs, a different recipient, changed wallet balances, and missing
//! descriptors — for a genesis-only bootstrap and a later finalized snapshot. The rule this pins is
//! *enforce when present, trust when absent*: a joiner that derived its own allocation must match
//! the imported genesis; a joiner with no local claim takes the bootstrap's allocation. A local
//! `None` is "no claim", not "a claim of no allocation" — the normal public-testnet bootstrap.
//!
//! These are two real in-process nodes over the protocol transport, not an in-process replay: the
//! joiner enters `NodeSyncing`, pulls the fringe and state roots, `populate_dag`/`insert_genesis`,
//! and then passes the restore-completion guard before Running.

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

#[derive(Clone, Copy)]
enum Phase {
    GenesisOnly,
    Snapshot,
}

#[derive(Clone, Copy)]
enum Expected {
    Restored,
    Refused,
}

fn wallets(total_drops: &str) -> String {
    format!("{DEPLOYER_REV_ADDR},{total_drops}\n")
}

fn address(byte: u8) -> String {
    RevAddress::from_public_key(&PublicKey::new(vec![byte; 65]))
        .expect("REV address")
        .to_base58()
}

/// A bootstrap that creates a production genesis with a non-empty allocation. The wallet total is
/// above the 1B-REV crossover, so the percentage arm (not just the floor) is the amount.
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
    let deploy = GrpcDeployService::connect("127.0.0.1", ports[4] as i32, 16 * 1024 * 1024)
        .await
        .expect("deploy gRPC");
    DeployRuntime::deploy_file_program(
        &deploy,
        1_000_000,
        1,
        -1,
        &PrivateKey::new(base16::decode(common::VALIDATOR_PRIV_HEX).expect("decode key")),
        rho.to_str().expect("rho path"),
        "/root",
    )
    .await
    .expect("deploy accepted");
    wait_for_finalized(client, base, 1).await;
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
    let blocks: serde_json::Value = client
        .get(format!("{base}/api/blocks"))
        .send()
        .await
        .expect("GET /api/blocks")
        .json()
        .await
        .expect("/api/blocks json");
    blocks
        .as_array()
        .and_then(|blocks| blocks.first())
        .and_then(|block| block["postStateHash"].as_str())
        .expect("a block with a postStateHash")
        .to_string()
}

fn compare_leaves(joiner: Option<DevVesting>, bootstrap: Option<DevVesting>) {
    let joiner = joiner.expect("the restored joiner carries an allocation");
    let bootstrap = bootstrap.expect("the bootstrap genesis carries an allocation");
    assert_eq!(
        joiner.address, bootstrap.address,
        "the joiner must install the bootstrap's recipient"
    );
    assert_eq!(
        joiner.allocation, bootstrap.allocation,
        "the joiner must install the bootstrap's amount"
    );
    assert!(
        joiner.unreleased <= joiner.allocation,
        "a restored leaf cannot have released more than it holds"
    );
}

async fn run_case(phase: Phase, spec: JoinerSpec, expected: Expected) {
    let boot_dir = temp_dir("lfs-boot");
    // Acquire both nodes' ports in one allocation. A pair needs twelve
    // disjoint endpoints; independent ephemeral allocations may reuse a
    // released port before either node binds it (especially under a debugger).
    let pair_ports = free_ports(12);
    let boot_ports = pair_ports[..6].to_vec();
    let join_ports = pair_ports[6..].to_vec();
    let recipient = address(2);
    let snapshot = matches!(phase, Phase::Snapshot);
    let boot_conf = bootstrap_conf(
        &boot_dir,
        &boot_ports,
        &recipient,
        "1000000000000000000",
        snapshot,
    );

    // The bootstrap's identity is persisted by `node_environment::create`; `build` re-reads it, so
    // the joiner can be pointed at it before either node is started.
    let boot_id = node_environment::create(&boot_conf).expect("bootstrap identity");
    let boot_peer = PeerNode::from(
        boot_id,
        "127.0.0.1".to_string(),
        Port::new(boot_ports[3]),
        Port::new(boot_ports[5]),
    );

    let join_dir = temp_dir("lfs-join");
    let mut join_conf = joiner_conf(&join_dir, &join_ports, &boot_peer);
    spec.apply(&mut join_conf, &join_dir);

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
            if snapshot {
                deploy_for_snapshot(&ready_dir, &ready_ports, &ready_client, &ready_base).await;
            }
        },
        (&join_conf, join_ports[5], join_ports[0]),
    )
    .await;

    match expected {
        Expected::Restored => {
            let join_base = format!("http://127.0.0.1:{}", join_ports[0]);
            let join_admin = format!("http://127.0.0.1:{}", join_ports[1]);
            // A snapshot joiner restores at least the finalized fringe the bootstrap published before
            // it dialled; a genesis-only joiner restores to height 0.
            let target = if snapshot { 1 } else { 0 };
            wait_for_blocks(&client, &join_base, &join_node).await;
            wait_for_height(&join_base, target, Duration::from_secs(120)).await;
            wait_for_sync_cleared(&join_admin, Duration::from_secs(120)).await;
            // The API serves before the launch phase finishes; "restored" and "the guard passed"
            // are not the same claim. The Running line is the latter.
            wait_for_running(&join_node).await;

            let join_root = latest_post_state_hash(&client, &join_base).await;
            let boot_root = latest_post_state_hash(&client, &boot_base).await;
            compare_leaves(
                join_node.dev_vesting_at_hex(&join_root).await,
                boot_node.dev_vesting_at_hex(&boot_root).await,
            );
            assert_eq!(
                join_node
                    .params_at_hex(&join_root)
                    .await
                    .dev_tax_vesting_blocks,
                boot_node
                    .params_at_hex(&boot_root)
                    .await
                    .dev_tax_vesting_blocks,
                "the joiner must carry the bootstrap's vesting term"
            );

            if snapshot {
                // Autopropose continues on the bootstrap; the restored joiner validates the next
                // finalized block.
                wait_for_height(&join_base, target + 1, Duration::from_secs(60)).await;
            }

            join_node.request_stop();
            boot_node.request_stop();
            let _ = tokio::time::timeout(Duration::from_secs(30), join_node.handle).await;
            let _ = tokio::time::timeout(Duration::from_secs(30), boot_node.handle).await;
        }
        Expected::Refused => {
            // The launch task's error is logged rather than returned through `serve`; the node's
            // own recording log is the per-case surface.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            loop {
                if let Some(line) = join_node
                    .log
                    .messages()
                    .into_iter()
                    .find(|line| line.contains("refusing to enter Running"))
                {
                    assert!(
                        line.contains("local developer allocation")
                            || line.contains("restored genesis allocation")
                            || line.contains("restored allocation"),
                        "the refusal must be the restore guard's: {line}"
                    );
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the joiner never reported a refusal; log tail: {:?}",
                    join_node
                        .log
                        .messages()
                        .into_iter()
                        .rev()
                        .take(12)
                        .collect::<Vec<_>>()
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            join_node.shutdown();
            boot_node.request_stop();
            let _ = tokio::time::timeout(Duration::from_secs(30), boot_node.handle).await;
        }
    }
}

fn matching_spec() -> JoinerSpec {
    JoinerSpec {
        recipient: Some(address(2)),
        wallets_rows: Some(wallets("1000000000000000000")),
        bonds: true,
        bonded: false,
    }
}

#[test]
fn genesis_only_restore_with_matching_inputs_preserves_the_allocation() {
    test_runtime().block_on(run_case(
        Phase::GenesisOnly,
        matching_spec(),
        Expected::Restored,
    ));
}

#[test]
fn genesis_only_restore_refuses_a_different_recipient() {
    test_runtime().block_on(run_case(
        Phase::GenesisOnly,
        JoinerSpec {
            recipient: Some(address(3)),
            ..matching_spec()
        },
        Expected::Refused,
    ));
}

#[test]
fn genesis_only_restore_refuses_changed_wallet_balances() {
    test_runtime().block_on(run_case(
        Phase::GenesisOnly,
        JoinerSpec {
            // The same recipient, but 2e18 drops is twice the bootstrap's total and therefore a
            // different percentage arm (400M REV against 200M).
            wallets_rows: Some(wallets("2000000000000000000")),
            ..matching_spec()
        },
        Expected::Refused,
    ));
}

#[test]
fn genesis_only_restore_with_missing_descriptors_trusts_the_bootstrap() {
    test_runtime().block_on(run_case(
        Phase::GenesisOnly,
        JoinerSpec {
            recipient: None,
            wallets_rows: None,
            bonds: false,
            bonded: false,
        },
        Expected::Restored,
    ));
}

#[test]
fn snapshot_restore_with_matching_inputs_preserves_the_allocation() {
    test_runtime().block_on(run_case(
        Phase::Snapshot,
        matching_spec(),
        Expected::Restored,
    ));
}

#[test]
fn snapshot_restore_refuses_a_different_recipient() {
    test_runtime().block_on(run_case(
        Phase::Snapshot,
        JoinerSpec {
            recipient: Some(address(3)),
            ..matching_spec()
        },
        Expected::Refused,
    ));
}

#[test]
fn snapshot_restore_refuses_changed_wallet_balances() {
    test_runtime().block_on(run_case(
        Phase::Snapshot,
        JoinerSpec {
            wallets_rows: Some(wallets("2000000000000000000")),
            ..matching_spec()
        },
        Expected::Refused,
    ));
}

#[test]
fn snapshot_restore_with_missing_descriptors_trusts_the_bootstrap() {
    test_runtime().block_on(run_case(
        Phase::Snapshot,
        JoinerSpec {
            recipient: None,
            wallets_rows: None,
            bonds: false,
            bonded: false,
        },
        Expected::Restored,
    ));
}

/// **A restored bonded joiner validates the chain and can propose on it.** The accepting matrix
/// above shows the joiner adopts the allocation; this shows the restored state is usable for the
/// node's actual job, not merely readable. Both validators are bonded at genesis, so the joiner's
/// proposal is one the bootstrap can accept.
#[test]
fn a_restored_bonded_joiner_validates_and_proposes() {
    test_runtime().block_on(async {
        let boot_dir = temp_dir("lfs-bonded-boot");
        // Keep bootstrap/joiner endpoint sets disjoint even if the kernel
        // would otherwise recycle ports between separate allocations.
        let pair_ports = free_ports(12);
        let boot_ports = pair_ports[..6].to_vec();
        let join_ports = pair_ports[6..].to_vec();
        let recipient = address(2);
        let boot_conf = bootstrap_conf(
            &boot_dir,
            &boot_ports,
            &recipient,
            "1000000000000000000",
            false,
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
        JoinerSpec {
            recipient: Some(recipient.clone()),
            wallets_rows: Some(wallets("1000000000000000000")),
            bonds: true,
            bonded: true,
        }
        .apply(&mut join_conf, &join_dir);

        let boot_base = format!("http://127.0.0.1:{}", boot_ports[0]);
        let client = reqwest::Client::new();
        let ready_client = client.clone();
        let ready_base = boot_base.clone();
        let (boot_node, join_node) = start_pair_after(
            (&boot_conf, boot_ports[4], boot_ports[0]),
            move || async move { wait_for_genesis(&ready_client, &ready_base).await },
            (&join_conf, join_ports[5], join_ports[0]),
        )
        .await;

        let join_base = format!("http://127.0.0.1:{}", join_ports[0]);
        let join_admin = format!("http://127.0.0.1:{}", join_ports[1]);
        wait_for_blocks(&client, &join_base, &join_node).await;
        wait_for_sync_cleared(&join_admin, Duration::from_secs(120)).await;
        wait_for_running(&join_node).await;

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
