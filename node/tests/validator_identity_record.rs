//! **A changed validator key on the second boot** (W-N5-06).
//!
//! The row: `--validator-private-key-path` may point at a different file on the next boot, and the
//! resolver holds no persistence — so the node comes up under a new signing identity over a data
//! directory whose chain was built by the old one, and *nothing says so*. The falsifier it asks for is
//! "a test that a second boot with a changed key file either refuses or emits a loud identity-change
//! record"; this is that test, and what it reads is the absence of both.
//!
//! Why it matters: the identity a node signs with is the one thing about it that consensus cares about,
//! and swapping it silently is indistinguishable from an operator pointing a node at the wrong data
//! directory — the class the `shard-id` and `store-format` markers already refuse for two *other*
//! facts about a data directory (`node/src/runtime/node_runtime.rs`). No such marker exists for the
//! validator identity.

mod common;

use std::time::Duration;

use common::{deploy_conf, free_ports, temp_dir, test_runtime};

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

/// Every file **directly** in `dir`, as `(name, bytes)` — the place a boot marker lives
/// (`shard-id` and `store-format` are both written there), and not the LMDB stores beneath it.
fn top_level_files(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("the data directory is readable") {
        let entry = entry.expect("a directory entry");
        let path = entry.path();
        if path.is_file() {
            out.push((
                entry.file_name().to_string_lossy().to_string(),
                std::fs::read(&path).unwrap_or_default(),
            ));
        }
    }
    out
}

/// **A configured validator public key that the private key does not derive is never compared**
/// (W-N5-05).
///
/// The row: the signing identity is derived from `casper.validator-private-key` alone, and
/// `casper.validator-public-key` — a key the operator configures, and the one every *other* surface
/// reports — is never checked against it. So a node can sign under one identity while its
/// configuration names another, and nothing refuses: the class is authority-widening, because the
/// identity that acts is not the identity the operator believes they configured.
///
/// The arm boots a node whose two configured keys genuinely differ and asserts it **starts**. The
/// reading is the boot, not a log line: this node is read-only until genesis is announced, so an
/// answering `/api/blocks` is the node fully up and computing with the private key's identity.
///
/// Falsified by the startup check the row asks for — a boot that derives the public key from the
/// private one and refuses when the two disagree — after which `common::start` never reaches a
/// serving node.
#[test]
fn a_configured_validator_public_key_the_private_key_does_not_derive_does_not_refuse_boot() {
    use rchain_casper::validator_identity::ValidatorIdentity;
    use rchain_shared::base16;

    // A second secp256k1 scalar, distinct from the harness's bonded one.
    const OTHER: &str = "67e56582298859ddae725f972992a07c6c4fb9f62a8fff58ce3ca926a1063530";

    test_runtime().block_on(async {
        let dir = temp_dir("validator-public-key-mismatch");
        let ports = free_ports(5);
        let mut conf = deploy_conf(&dir, &ports);

        let signing = ValidatorIdentity::from_hex(common::VALIDATOR_PRIV_HEX)
            .expect("the harness's bonded identity");
        let other = ValidatorIdentity::from_hex(OTHER).expect("a different identity");
        let signing_pub = base16::encode(signing.public_key.bytes());
        let other_pub = base16::encode(other.public_key.bytes());
        assert_ne!(signing_pub, other_pub, "the two keys have to differ");

        // **The operator's stated identity is the one the private key does not derive.**
        conf.casper.validator_public_key = Some(other_pub.clone());

        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_genesis(&format!("http://127.0.0.1:{}", ports[0])).await;

        eprintln!(
            "W-N5-05 fired: configured validator-public-key {other_pub} against the identity the \
             private key derives {signing_pub}; the node started and is serving"
        );

        // The reading, asserted rather than narrated: the node is up. Everything below it signs with
        // the private key's identity, and the configured public key is inert — reading it anywhere in
        // the runtime is what would make this test unnecessary.
        node.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    });
}

/// **A changed key file on the second boot is neither refused nor recorded** (W-N5-06).
///
/// Two boots over one data directory, with the validator key given *by path* both times and a
/// different file the second time. Three readings:
///
/// 1. the second boot **succeeds** — no refusal, which is half of what the falsifier asks for;
/// 2. **no file in the data directory names either key**, so nothing records the change, which is the
///    other half;
/// 3. and the markers that *do* exist are the two facts this directory already keeps (`shard-id` and
///    `store-format`), so the absence above is an absence and not a this-arm-read-nothing.
#[test]
fn a_second_boot_with_a_changed_key_file_is_neither_refused_nor_recorded() {
    // Two distinct, valid secp256k1 scalars. The first is the bonded one the harness builds genesis
    // with; the second is a validator the chain has never seen — which is exactly the change the row
    // is about, and the reason "it booted" is the finding rather than an accident of the fixture.
    const FIRST: &str = common::VALIDATOR_PRIV_HEX;
    const SECOND: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    test_runtime().block_on(async {
        let dir = temp_dir("validator-identity-change");
        // **The key files live outside the data directory**, and that is not tidiness: the reading
        // below scans the data directory for anything naming a key, and a fixture that wrote its own
        // keys there would find them and call that a finding. (Measured: it did.)
        let keys = temp_dir("validator-identity-keys");
        let first_key = keys.join("validator-first.hex");
        let second_key = keys.join("validator-second.hex");
        rchain_crypto::util::key_util::write_private_key(&first_key, format!("{FIRST}\n"))
            .expect("write the first key");
        rchain_crypto::util::key_util::write_private_key(&second_key, format!("{SECOND}\n"))
            .expect("write the second key");
        // **Read back through the node's own loader**, so the 0600 rule and the bare-base16 parsing
        // are the ones a boot would apply. (The path → hex step is `resolve_validator_private_key`,
        // private to the crate; what this test is about is the identity across boots, not that
        // plumbing, which `configuration.rs`'s own test already pins.)
        let read_key = |path: &std::path::Path| -> String {
            let key = rchain_crypto::util::key_util::load_validator_private_key(path)
                .expect("the key file loads");
            rchain_shared::base16::encode(key.bytes())
        };
        let first_hex = read_key(&first_key);
        let second_hex = read_key(&second_key);
        assert_ne!(
            first_hex, second_hex,
            "two different keys, or nothing changes"
        );

        // --- boot 1: the bonded identity over a fresh data directory ---------------------------
        let ports = free_ports(5);
        let mut conf = deploy_conf(&dir, &ports);
        conf.casper.validator_private_key = Some(first_hex.clone());
        conf.casper.validator_private_key_path = Some(first_key.clone());
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_genesis(&format!("http://127.0.0.1:{}", ports[0])).await;
        node.shutdown().await;
        // **Two seconds, because the next boot opens the same data directory.** LMDB takes an
        // exclusive lock on it, so a second boot that arrives before the first has closed does not
        // merely race — it fails to open the store, and this arm would then be measuring shutdown
        // latency on whatever machine it runs on. (CI's runner is slower than this box by enough to
        // matter; the number is a margin, not a measurement.)
        tokio::time::sleep(Duration::from_secs(2)).await;

        let after_first = top_level_files(&dir);
        assert!(
            after_first.iter().any(|(name, _)| name == "shard-id"),
            "the first boot wrote its markers: {:?}",
            after_first.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );

        // --- boot 2: **a different key**, the same data directory ------------------------------
        let ports = free_ports(5);
        let mut conf = deploy_conf(&dir, &ports);
        conf.casper.validator_private_key = Some(second_hex.clone());
        conf.casper.validator_private_key_path = Some(second_key.clone());
        let node = common::start(&conf, ports[2], ports[0]).await;

        // (1) It comes up. A refusal would be the other half of the falsifier, and this is where it
        //     would have to show: the node is read-only until genesis is announced, so an answering
        //     `/api/blocks` is the node fully started.
        wait_for_genesis(&format!("http://127.0.0.1:{}", ports[0])).await;
        node.shutdown().await;
        // **Two seconds, because the next boot opens the same data directory.** LMDB takes an
        // exclusive lock on it, so a second boot that arrives before the first has closed does not
        // merely race — it fails to open the store, and this arm would then be measuring shutdown
        // latency on whatever machine it runs on. (CI's runner is slower than this box by enough to
        // matter; the number is a margin, not a measurement.)
        tokio::time::sleep(Duration::from_secs(2)).await;

        // (2) Nothing records either key — not the one that built the chain, not the one it is now
        //     signing with. A "loud identity-change record" would be a file here naming one of them.
        let files = top_level_files(&dir);
        for (name, bytes) in &files {
            let text = String::from_utf8_lossy(bytes);
            assert!(
                !text.contains(FIRST) && !text.contains(SECOND),
                "**{name} names a validator key** (W-N5-06): the second boot recorded the change \
                 after all, and this arm's reading is the opposite"
            );
        }

        // (3) The control: the markers a data directory does keep are here, so the loop above had
        //     something to read and the absence is an absence.
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"shard-id") && names.contains(&"store-format"),
            "the directory keeps its own two facts — so the keys' absence is a fact about a \
             directory that does write markers: {names:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&keys);
    });
}
