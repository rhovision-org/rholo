//! The on-node 2PC gateway (Laws 26–29): `GatewayTxn` driving a cross-shard transaction across two
//! shards of the *same* node, with no off-chain client and no `DeployService` test double standing
//! in for the transport.
//!
//! The shards here are served by a test `BlockApi` that executes a pooled deploy on that shard's
//! runtime (the same evaluation the `InProcDeployService` double used), so these tests pin the
//! **coordination** semantics: uniformity, the durable record, restart recovery, and idempotency
//! under a repeated `txn_id`. The block-pipeline side of the legs — a real pooled deploy included in
//! a block, and play/replay agreement — is covered by `cross_shard_txn.rs` and the node-level
//! gateway test.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rchain_casper::construct_deploy;
use rchain_casper::gateway::ledger::{CoordState, TxnLedger, Vote};
use rchain_casper::gateway::{GatewayLeg, GatewayTxn, LocalShard, LocalShardDeployService};
use rchain_models::casper::protocol::casper_message::{DeployData, SignedDeployData};
use rchain_rholang::native_state::NativeSystemState;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::refined::NonNegI64;
use rchain_shared::store_manager::InMemoryStoreManager;

use common::{build_runtime_manager, fund, shard, txn_legs, TestShardApi};

/// The two-shard fixture: shard A and B, each with a runtime and its own `BlockApi`.
struct Fixture {
    gateway: GatewayTxn,
    coordinator_addr: String,
    destination: String,
    shard_a: Arc<TestShardApi>,
    ledger: Arc<TxnLedger>,
    pub_key: rchain_crypto::public_key::PublicKey,
    /// The shards' runtimes, for asserting the escrow where it actually lives.
    rm_a: Arc<rchain_casper::runtime_manager::RuntimeManager>,
    rm_b: Arc<rchain_casper::runtime_manager::RuntimeManager>,
}

/// Build the fixture over `manager` so a test can rebuild it (a "restart") against the same ledger.
async fn fixture(manager: Arc<InMemoryStoreManager>) -> Fixture {
    let (key, pub_key) = construct_deploy::default_key_pair().unwrap();
    let coordinator_addr = RevAddress::from_public_key(&pub_key).unwrap().to_base58();
    // A real REV address: `rho:txn` refuses a destination it cannot decode (C301), so a literal
    // placeholder is no longer a legal fixture.
    let destination = RevAddress::from_public_key(&construct_deploy::default_pub2().unwrap())
        .unwrap()
        .to_base58();

    let rm_a = Arc::new(build_runtime_manager().await);
    let rm_b = Arc::new(build_runtime_manager().await);
    fund(rm_a.runtime(), &coordinator_addr, 100);
    fund(rm_b.runtime(), &coordinator_addr, 100);

    let api_a = Arc::new(TestShardApi::new("/root", Arc::clone(&rm_a)));
    let api_b = Arc::new(TestShardApi::new("/root/child", Arc::clone(&rm_b)));

    let ledger = Arc::new(TxnLedger::open(manager.as_ref()).await.expect("ledger"));
    let mut shards = BTreeMap::new();
    shards.insert(
        shard("/root"),
        LocalShard {
            shard_id: shard("/root"),
            block_api: api_a.clone(),
            max_listen_depth: 50,
        },
    );
    shards.insert(
        shard("/root/child"),
        LocalShard {
            shard_id: shard("/root/child"),
            block_api: api_b.clone(),
            max_listen_depth: 50,
        },
    );
    let local = Arc::new(LocalShardDeployService::new(shards));
    let gateway = GatewayTxn::new(
        local,
        ledger.clone(),
        key.clone(),
        pub_key.clone(),
        Duration::from_secs(5),
    );
    Fixture {
        gateway,
        coordinator_addr,
        destination,
        shard_a: api_a,
        ledger,
        pub_key,
        rm_a,
        rm_b,
    }
}

/// One address's vault balance on a shard; an absent vault is zero, as the participant reads it.
async fn vault(rm: &rchain_casper::runtime_manager::RuntimeManager, address: &str) -> i64 {
    i64::from(
        NativeSystemState::new(rm.runtime().native_store())
            .vault_balance(address)
            .await
            .expect("vault read")
            .unwrap_or_else(NonNegI64::zero),
    )
}

#[tokio::test]
async fn gateway_commits_a_two_shard_transaction_and_records_the_decision() {
    let fx = fixture(Arc::new(InMemoryStoreManager::default())).await;

    let record = fx
        .gateway
        .run(b"gw-commit", &txn_legs(&fx.destination))
        .await
        .expect("run");

    assert_eq!(record.state, CoordState::Committed);
    assert_eq!(record.legs.len(), 2);
    assert_eq!(record.votes.len(), 2);
    assert!(record.votes.iter().all(|(_, v)| *v == Vote::Ready));
    assert!(record.decision_is_deterministic());
    // The record is durable and reported by the ledger.
    let stored = fx.ledger.get(b"gw-commit").await.unwrap().expect("stored");
    assert_eq!(stored, record);
    assert!(fx.gateway.list().await.unwrap().is_empty());
}

/// The escrow actually moved on both shards, in the shards' own state.
#[tokio::test]
async fn gateway_moves_the_escrow_on_every_leg() {
    let fx = fixture(Arc::new(InMemoryStoreManager::default())).await;

    fx.gateway
        .run(b"gw-balances", &txn_legs(&fx.destination))
        .await
        .expect("run");

    let native_a = NativeSystemState::new(fx.rm_a.runtime().native_store());
    let native_b = NativeSystemState::new(fx.rm_b.runtime().native_store());
    // Each shard escrowed its own leg out of the coordinator's vault and credited the destination.
    assert_eq!(
        i64::from(
            native_a
                .vault_balance(&fx.coordinator_addr)
                .await
                .unwrap()
                .unwrap()
        ),
        70
    );
    assert_eq!(
        i64::from(
            native_b
                .vault_balance(&fx.coordinator_addr)
                .await
                .unwrap()
                .unwrap()
        ),
        60
    );
    assert_eq!(
        i64::from(
            native_a
                .vault_balance(&fx.destination)
                .await
                .unwrap()
                .unwrap()
        ),
        30
    );
    assert_eq!(
        i64::from(
            native_b
                .vault_balance(&fx.destination)
                .await
                .unwrap()
                .unwrap()
        ),
        40
    );
    // Law 98: the debit and the credit are the same amount on each shard, so the leg's total is
    // conserved — nothing is minted or burned by the split.
    assert_eq!(
        vault(&fx.rm_a, &fx.coordinator_addr).await + vault(&fx.rm_a, &fx.destination).await,
        100
    );
    assert_eq!(
        vault(&fx.rm_b, &fx.coordinator_addr).await + vault(&fx.rm_b, &fx.destination).await,
        100
    );
}

/// One leg cannot prepare (its escrow is short) ⇒ the whole transaction aborts and the prepared leg
/// is compensated: Law 27's uniformity.
#[tokio::test]
async fn gateway_aborts_every_leg_when_one_cannot_prepare() {
    let manager = Arc::new(InMemoryStoreManager::default());
    let (key, pub_key) = construct_deploy::default_key_pair().unwrap();
    let coordinator_addr = RevAddress::from_public_key(&pub_key).unwrap().to_base58();
    // A real REV address: `rho:txn` refuses a destination it cannot decode (C301), so a literal
    // placeholder is no longer a legal fixture.
    let destination = RevAddress::from_public_key(&construct_deploy::default_pub2().unwrap())
        .unwrap()
        .to_base58();

    let rm_a = Arc::new(build_runtime_manager().await);
    let rm_b = Arc::new(build_runtime_manager().await);
    fund(rm_a.runtime(), &coordinator_addr, 100);
    // Shard B holds only 10 REV, so its 40 REV prepare fails.
    fund(rm_b.runtime(), &coordinator_addr, 10);

    let api_a = Arc::new(TestShardApi::new("/root", Arc::clone(&rm_a)));
    let api_b = Arc::new(TestShardApi::new("/root/child", Arc::clone(&rm_b)));
    let mut shards = BTreeMap::new();
    shards.insert(
        shard("/root"),
        LocalShard {
            shard_id: shard("/root"),
            block_api: api_a.clone(),
            max_listen_depth: 50,
        },
    );
    shards.insert(
        shard("/root/child"),
        LocalShard {
            shard_id: shard("/root/child"),
            block_api: api_b.clone(),
            max_listen_depth: 50,
        },
    );
    let ledger = Arc::new(TxnLedger::open(manager.as_ref()).await.expect("ledger"));
    let gateway = GatewayTxn::new(
        Arc::new(LocalShardDeployService::new(shards)),
        ledger.clone(),
        key,
        pub_key,
        Duration::from_secs(5),
    );

    let record = gateway
        .run(b"gw-abort", &txn_legs(&destination))
        .await
        .expect("run");
    assert_eq!(record.state, CoordState::Aborted);
    assert!(
        record.reason.is_some(),
        "an abort must record why: {:?}",
        record
    );
    assert!(record.decision_is_deterministic());

    // The prepared leg was compensated: shard A is back to its opening balance, and nothing reached
    // the destination anywhere.
    let native_a = NativeSystemState::new(rm_a.runtime().native_store());
    let native_b = NativeSystemState::new(rm_b.runtime().native_store());
    assert_eq!(
        i64::from(
            native_a
                .vault_balance(&coordinator_addr)
                .await
                .unwrap()
                .unwrap()
        ),
        100
    );
    assert_eq!(
        i64::from(
            native_b
                .vault_balance(&coordinator_addr)
                .await
                .unwrap()
                .unwrap()
        ),
        10
    );
    assert_eq!(native_a.vault_balance(&destination).await.unwrap(), None);
    assert_eq!(native_b.vault_balance(&destination).await.unwrap(), None);
    // Law 98: each leg's total is conserved on its own shard — A's debit was refunded, B never
    // moved a unit.
    assert_eq!(
        vault(&rm_a, &coordinator_addr).await + vault(&rm_a, &destination).await,
        100
    );
    assert_eq!(
        vault(&rm_b, &coordinator_addr).await + vault(&rm_b, &destination).await,
        10
    );
}

/// A re-run of a completed transaction returns the same record and touches no participant, so a
/// retried request cannot move funds twice (Law 28 at the coordinator boundary).
#[tokio::test]
async fn gateway_is_idempotent_under_txn_id() {
    let manager = Arc::new(InMemoryStoreManager::default());
    let fx = fixture(manager).await;

    let first = fx
        .gateway
        .run(b"gw-idempotent", &txn_legs(&fx.destination))
        .await
        .expect("first run");
    let second = fx
        .gateway
        .run(b"gw-idempotent", &txn_legs(&fx.destination))
        .await
        .expect("second run");

    assert_eq!(first, second);
    assert_eq!(second.state, CoordState::Committed);
    // A terminal record is returned without contacting a shard, so no further block was produced.
    let height_after_first = fx.shard_a.height();
    fx.gateway
        .run(b"gw-idempotent", &txn_legs(&fx.destination))
        .await
        .expect("third run");
    assert_eq!(
        fx.shard_a.height(),
        height_after_first,
        "a retry must not submit another deploy"
    );
}

/// **The regression test for AUDIT C125.** `run` reads the ledger, decides from that read whether to
/// open a transaction, and writes the record it built from *its own* legs — a check-then-act with no
/// per-`txn_id` serialization. Two concurrent callers for one id therefore both open it: each drives its
/// own phase one with its own legs, and the durable record that `recover_in_flight` trusts afterwards
/// describes whichever leg list won the last write rather than necessarily the effects applied under
/// that id.
///
/// Two things about how it is written. The concurrency is **deterministic rather than hopeful**: every
/// caller reaches the ledger read's `await` before any caller can reach its `put`, so on the unfixed code
/// *all* of them read "no record" and all of them open. And the assertion is written so that it can fail
/// on the broken code — it is the *number of opens*, calibrated against one open's cost in this same
/// fixture, not "the call succeeded", which is green either way.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_calls_for_one_txn_id_open_the_transaction_once() {
    let manager = Arc::new(InMemoryStoreManager::default());
    let fx = Arc::new(fixture(manager).await);

    let legs_for = |amount: i64| -> Vec<GatewayLeg> {
        vec![
            GatewayLeg {
                shard_id: shard("/root"),
                amount,
                to: fx.destination.clone(),
            },
            GatewayLeg {
                shard_id: shard("/root/child"),
                amount,
                to: fx.destination.clone(),
            },
        ]
    };

    // What one open costs on this fixture, measured rather than assumed: a transaction with its own id
    // prepares and commits one leg on each shard.
    let before = fx.shard_a.height();
    fx.gateway
        .run(b"gw-concurrent-calibration", &legs_for(10))
        .await
        .expect("calibration run");
    let one_open = fx.shard_a.height() - before;
    assert!(
        one_open > 0,
        "one open must submit at least one deploy to shard A"
    );

    // Four callers, four *distinct* leg lists, released together.
    let after_calibration = fx.shard_a.height();
    let barrier = Arc::new(tokio::sync::Barrier::new(4));
    let mut handles = Vec::new();
    for amount in [30i64, 31, 32, 33] {
        let legs = legs_for(amount);
        let fx = Arc::clone(&fx);
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            fx.gateway.run(b"gw-concurrent", &legs).await
        }));
    }
    let mut records = Vec::new();
    for handle in handles {
        records.push(handle.await.expect("join").expect("run"));
    }

    for record in &records {
        assert_eq!(
            record, &records[0],
            "every caller must converge on the transaction that was opened, not return one of its own"
        );
    }
    let stored = fx
        .ledger
        .get(b"gw-concurrent")
        .await
        .unwrap()
        .expect("a record");
    assert_eq!(
        stored, records[0],
        "the durable record must be the one every caller saw"
    );

    assert_eq!(
        fx.shard_a.height() - after_calibration,
        one_open,
        "four concurrent calls must not produce four phase ones — the escrow would be prepared, and \
         possibly committed, more times than the record describes"
    );
}

/// A gateway that restarts mid-transaction finishes it from the ledger alone.
///
/// The fixture is dropped and rebuilt over the *same* ledger (an in-memory store standing in for the
/// node's `gateway-txn` database), which is what `recover_in_flight` sees on boot.
#[tokio::test]
async fn gateway_recovers_an_in_flight_transaction_from_the_ledger() {
    let manager = Arc::new(InMemoryStoreManager::default());

    // Drive a record to `Prepared` — one leg voted ready, the other not yet — then "crash".
    let fx = fixture(manager.clone()).await;
    let mut record = rchain_casper::gateway::CoordRecord {
        txn_id: b"gw-recover".to_vec(),
        state: CoordState::Proposed,
        coordinator: fx.pub_key.clone(),
        legs: vec![
            rchain_casper::gateway::LegRecord {
                shard_id: shard("/root"),
                amount: NonNegI64::try_from(30).unwrap(),
                to: fx.destination.clone(),
            },
            rchain_casper::gateway::LegRecord {
                shard_id: shard("/root/child"),
                amount: NonNegI64::try_from(40).unwrap(),
                to: fx.destination.clone(),
            },
        ],
        votes: Vec::new(),
        reason: None,
    };
    // One vote of two legs is the genuinely-in-flight state: `Prepared`, not yet decided.
    record.record_vote(shard("/root"), Vote::Ready, None);
    assert_eq!(record.state, CoordState::Prepared);
    assert!(!record.state.is_terminal());
    fx.ledger.put(&record).await.expect("put in-flight record");
    drop(fx);

    // A fresh gateway over the same ledger finds the record in flight and finishes it.
    let restarted = fixture(manager).await;
    let in_flight = restarted.ledger.in_flight().await.expect("in flight");
    assert_eq!(in_flight.len(), 1);
    assert_eq!(in_flight[0].state, CoordState::Prepared);

    let recovered = restarted
        .gateway
        .recover_in_flight()
        .await
        .expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].txn_id, b"gw-recover".to_vec());
    assert_eq!(recovered[0].state, CoordState::Committed);
    assert!(recovered[0].decision_is_deterministic());
    // Nothing is left in flight, and the decision is durable.
    assert!(restarted.gateway.list().await.unwrap().is_empty());
    assert_eq!(
        restarted
            .ledger
            .get(b"gw-recover")
            .await
            .unwrap()
            .expect("stored")
            .state,
        CoordState::Committed
    );
}

/// A transaction naming a shard this node is not a member of is rejected, never relayed.
#[tokio::test]
async fn gateway_rejects_a_non_member_shard() {
    let fx = fixture(Arc::new(InMemoryStoreManager::default())).await;
    let err = fx
        .gateway
        .run(
            b"gw-foreign",
            &[GatewayLeg {
                shard_id: shard("/elsewhere"),
                amount: 1,
                to: fx.destination.clone(),
            }],
        )
        .await
        .expect_err("a non-member shard must be rejected");
    assert!(err.contains("/elsewhere"), "{err}");
    assert!(err.contains("/root, /root/child"), "{err}");
}

/// A deploy addressed at the local service for a non-member shard is rejected there too — the
/// points of entry agree.
#[tokio::test]
async fn local_deploy_service_rejects_a_non_member_shard() {
    use rchain_casper::protocol::client::DeployService;

    let manager = Arc::new(InMemoryStoreManager::default());
    let fx = fixture(manager).await;
    let _ = &fx;

    let rm = Arc::new(build_runtime_manager().await);
    let api = Arc::new(TestShardApi::new("/root", Arc::clone(&rm)));
    let mut shards = BTreeMap::new();
    shards.insert(
        shard("/root"),
        LocalShard {
            shard_id: shard("/root"),
            block_api: api,
            max_listen_depth: 50,
        },
    );
    let local = LocalShardDeployService::new(shards);
    let deploy = SignedDeployData {
        data: DeployData {
            attachments: Vec::new(),
            term: "Nil".to_string(),
            timestamp: 0,
            phlo_price: 1,
            phlo_limit: 1,
            valid_after_block_number: 0,
            shard_id: "/elsewhere".to_string(),
        },
        deployer: vec![0u8; 65],
        sig: Vec::new(),
        sig_algorithm: "secp256k1".to_string(),
    };
    let err = local
        .deploy(&deploy)
        .await
        .expect_err("a non-member shard must be rejected");
    assert!(err.join(" ").contains("/elsewhere"), "{err:?}");
}
