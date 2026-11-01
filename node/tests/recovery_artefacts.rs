//! The two artefacts the reconciliation tool verifies (AUDIT C261), end to end over HTTP on a real
//! standalone validator.
//!
//! `tools/reconcile-network.sh` is the operator's tool for a broken network, and until this unit it
//! could only read what a node *said*: `/api/last-finalized-block` is a node-local claim with nothing
//! tying it to a validator, and `/api/block/{hash}` is a summary the client cannot recompute the hash
//! of — so every field a recovery decision rested on was the node's word.
//!
//! Two routes close that, and this test drives them the way the tool does:
//!
//! - `GET /api/v1/block/{hash}/raw` answers with the preimage the block hash is taken over, so the
//!   client hashes it and reads the fields off bytes it verified;
//! - `GET /api/v1/finality-attestation?nonce=…` answers with the node's finalised block signed by its
//!   validator key — a claim that is *attributable*, which is what lets it carry weight.
//!
//! The verifier is `rnode verify-block` / `rnode verify-attestation`'s own code
//! (`rchain_node::runtime::recovery_verify`), called here in-process rather than through the binary,
//! so a failure points at the artefact rather than at argument parsing.

mod common;

use std::time::Duration;

use rchain_node::runtime::recovery_verify;
use serde_json::Value;

use common::{deploy_conf, free_ports, temp_dir, VALIDATOR_PRIV_HEX};

/// Poll `/api/last-finalized-block` until the node has finalised something at or above `at`,
/// returning the response body.
///
/// **Why the node needs a state-changing deploy first.** A block whose only payload is the dev-mode
/// dummy `Nil` deploy has no state transition, and the attestation guard reports exactly that —
/// `nothing_to_finalize=true`, `new_state_transition=false` — so no layer is published and nothing
/// finalises. That is the guard working (`F-U2-01`), and it is why this test submits a real deploy and
/// proposes rather than switching autopropose on: the artefact under test is the *finalised* block's.
async fn wait_for_finality(client: &reqwest::Client, base: &str, at: i64) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(resp) = client
            .get(format!("{base}/api/last-finalized-block"))
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(json) = resp.json::<Value>().await {
                    if json["blockInfo"]["blockNumber"]
                        .as_i64()
                        .is_some_and(|n| n >= at)
                    {
                        return json;
                    }
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the node never finalised a block at or above {at}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Poll `/api/blocks` until the genesis block appears — the node is not reachable for anything else
/// before it (`deploy_block.rs::wait_for_genesis` is the same wait).
async fn wait_for_genesis(client: &reqwest::Client, base: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(resp) = client.get(format!("{base}/api/blocks")).send().await {
            if resp.status().is_success() {
                if let Ok(Value::Array(a)) = resp.json::<Value>().await {
                    if !a.is_empty() {
                        return;
                    }
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the genesis block never appeared"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A hex nonce with the entropy the route requires (the handler refuses fewer than 16 bytes).
fn nonce() -> String {
    "5a".repeat(32)
}

#[test]
fn the_node_serves_a_block_a_client_can_verify_and_attests_its_finality() {
    common::test_runtime().block_on(async {
        use rchain_casper::protocol::client::{DeployRuntime, GrpcDeployService};
        use rchain_crypto::private_key::PrivateKey;
        use rchain_shared::base16;

        let dir = temp_dir("recovery-artefacts");
        let ports = free_ports(5); // http, admin-http, grpc-internal, protocol, grpc-external
        let mut conf = deploy_conf(&dir, &ports);
        // The chain has to *keep* growing for the finaliser to have a later message to publish a layer
        // from, and the deploy below is what gives that layer a state transition to carry. Together
        // they are the combination `deploy_block.rs::faucet_delivery_…` runs on.
        conf.autopropose = true;
        conf.dev_mode = true;
        let conf = conf;
        let _node = common::start(&conf, ports[2] as u16, ports[0] as u16).await;
        let base = format!("http://127.0.0.1:{}", ports[0]);
        let client = reqwest::Client::new();
        wait_for_genesis(&client, &base).await;

        // A **state-changing** deploy, then a propose: same path as
        // `deploy_block.rs::deploy_is_processed_into_a_block`. Autopropose's dummy deploy alone would
        // not do — it is a `Nil`, so the guard sees `new_state_transition=false` (see
        // `wait_for_finality`).
        let rho = dir.join("verify-me.rho");
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
        // No explicit propose: autopropose is on, and asking at the same time is refused with
        // "another propose is in progress" (`ProposerInstance` serialises them). The deploy is picked
        // up by the next round.

        let finalized = wait_for_finality(&client, &base, 1).await;
        // --- the attestation, bound to this run's nonce -----------------------------------------
        let n = nonce();
        let attestation: Value = client
            .get(format!("{base}/api/v1/finality-attestation?nonce={n}"))
            .send()
            .await
            .expect("GET attestation")
            .json()
            .await
            .expect("attestation json");
        let proved_attestation = recovery_verify::verify_attestation(&attestation.to_string(), &n)
            .expect("the node's own attestation must verify against the nonce it was asked for");
        let att: Value = serde_json::from_str(&proved_attestation).expect("the verdict is JSON");

        assert_eq!(att["verified"], Value::Bool(true));
        // Bind both artefacts to the attestation's fixed block. Autopropose may advance the
        // last-finalized representative between requests, while this identity remains stable.
        let hash = att["blockHash"]
            .as_str()
            .expect("attested hash")
            .to_string();
        let is_finalized: Value = client
            .get(format!("{base}/api/is-finalized/{hash}"))
            .send()
            .await
            .expect("GET attested block finality")
            .json()
            .await
            .expect("finality JSON");
        assert_eq!(
            is_finalized,
            Value::Bool(true),
            "the attested block is finalized"
        );

        // --- the raw preimage, verified with the node's own hash and signature rules -------------
        let raw: Value = client
            .get(format!("{base}/api/v1/block/{hash}/raw"))
            .send()
            .await
            .expect("GET raw block")
            .json()
            .await
            .expect("raw block json");
        let verdict = recovery_verify::verify_block(&raw.to_string())
            .expect("the node's own raw response must verify");
        let proved: Value = serde_json::from_str(&verdict).expect("the verdict is JSON");

        assert_eq!(proved["verified"], Value::Bool(true));
        assert!(
            proved["blockNumber"].as_i64().unwrap()
                >= finalized["blockInfo"]["blockNumber"].as_i64().unwrap(),
            "the attested finalized block does not predate the observed finality"
        );
        assert_eq!(
            proved["blockHash"].as_str().unwrap(),
            hash,
            "the verified preimage is the block that was asked for"
        );

        // The fields the meet's arithmetic uses come out of the verified bytes, and they must agree
        // with what the summary route says — that agreement is the point: the summary is now checked
        // rather than believed.
        let summary: Value = client
            .get(format!("{base}/api/block/{hash}"))
            .send()
            .await
            .expect("GET block summary")
            .json()
            .await
            .expect("block summary json");
        assert_eq!(
            proved["blockNumber"], summary["blockInfo"]["blockNumber"],
            "the verified height is the height the summary reports"
        );
        assert_eq!(
            proved["seqNum"], summary["blockInfo"]["seqNum"],
            "the verified seq_num is the seq_num the summary reports"
        );
        assert_eq!(
            proved["sender"], summary["blockInfo"]["sender"],
            "the verified sender is the sender the summary reports"
        );
        assert!(
            proved["bonds"].as_array().is_some_and(|b| !b.is_empty()),
            "a finalised block carries the bond map the meet weighs stake with: {proved}"
        );

        // The signature is by a validator this shard bonds — the test's validator key, which is also
        // the sender of every block a one-validator shard produces.
        let validator_pub = {
            use rchain_crypto::private_key::PrivateKey;
            use rchain_crypto::signatures::secp256k1::Secp256k1;
            use rchain_crypto::signatures::signatures_alg::SignaturesAlg;
            use rchain_shared::base16;
            let key = PrivateKey::new(base16::decode(VALIDATOR_PRIV_HEX).expect("decode key"));
            base16::encode(Secp256k1.to_public(&key).expect("pubkey").bytes())
        };
        assert_eq!(att["sender"].as_str().unwrap(), validator_pub);
        assert_eq!(
            proved["sender"].as_str().unwrap(),
            validator_pub,
            "the shard's only validator proposed the block the same key attests"
        );

        // --- the two refusals, on the same live artefacts ---------------------------------------
        //
        // A stale attestation is not an answer to a later question: re-verifying it against another
        // run's nonce must refuse rather than pass. This is the freshness rule, checked against a
        // real response rather than a fixture.
        let other = "7b".repeat(32);
        assert!(
            recovery_verify::verify_attestation(&attestation.to_string(), &other).is_err(),
            "an attestation made for one nonce is not an answer to another"
        );

        // And a raw response whose preimage was changed cannot verify, which is exactly the class of
        // lie the summary route could not detect.
        let mut tampered = raw.clone();
        let encoded = tampered["encoded"].as_str().expect("encoded").to_string();
        let mut bytes = rchain_shared::base16::decode(&encoded).expect("base16");
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0x01;
        tampered["encoded"] = Value::String(rchain_shared::base16::encode(&bytes));
        assert!(
            recovery_verify::verify_block(&tampered.to_string()).is_err(),
            "a preimage that no longer hashes to the claimed hash must refuse"
        );
    });
}
