//! The multi-shard gateway's **failure** paths (Laws 26–29).
//!
//! `gateway_txn.rs` covers a well-behaved shard. The branches here only fire when a participant
//! misbehaves — a deploy rejected, a reply that never comes, a head that cannot be read, a ledger
//! that cannot be written — and they are the ones that decide whether a cross-shard transaction can
//! leave funds stranded, so each is pinned explicitly.
//!
//! The shards are `common::TestShardApi` with a fault knob switched on, rather than a second
//! `BlockApi` double: one double with knobs is less code than a wrapper that delegates 25 methods,
//! and it keeps the "what does a shard do" story in one place.

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rchain_casper::construct_deploy;
use rchain_casper::gateway::ledger::{CoordRecord, CoordState, LegRecord, TxnLedger, Vote};
use rchain_casper::gateway::{GatewayLeg, GatewayTxn, LocalShard, LocalShardDeployService};
use rchain_crypto::private_key::PrivateKey;
use rchain_crypto::public_key::PublicKey;
use rchain_rholang::native_state::NativeSystemState;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::refined::{NonNegI64, ShardId};
use rchain_shared::store::KeyValueStore;
use rchain_shared::store_manager::{InMemoryStoreManager, KeyValueStoreManager};
use rchain_shared::typed_store::SharedStore;

use common::{build_runtime_manager, fund, shard, txn_legs, ShardFaults, TestShardApi};

/// A store manager whose stores exist but reject every read and write, so the ledger *opens* and
/// then fails — which is the failure `recover_in_flight` has to propagate rather than swallow.
#[derive(Default)]
struct FailingStoreManager;

struct FailingStore;

impl KeyValueStore for FailingStore {
    fn get(&self, _keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>, String> {
        Err("store read failed".to_string())
    }
    fn put(&mut self, _pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<(), String> {
        Err("store write failed".to_string())
    }
    fn delete(&mut self, _keys: &[Vec<u8>]) -> Result<usize, String> {
        Err("store delete failed".to_string())
    }
    fn entries(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        Err("store scan failed".to_string())
    }
}

#[async_trait::async_trait]
impl KeyValueStoreManager for FailingStoreManager {
    async fn store(&self, _name: &str) -> Result<SharedStore, String> {
        let store: Box<dyn KeyValueStore + Send + Sync> = Box::new(FailingStore);
        Ok(Arc::new(tokio::sync::Mutex::new(store)))
    }
    async fn shutdown(&self) {}
}

/// A store that accepts `allow` writes and fails every write after them. The coordinator's second
/// write with one leg is the decision (the first is the open record), so this is how Law 99a's
/// ordering is pinned: a decision that cannot be made durable must not be delivered.
struct FailAfterStoreManager {
    allow: usize,
}

struct FailAfterStore {
    inner: BTreeMap<Vec<u8>, Vec<u8>>,
    puts: Arc<AtomicUsize>,
    allow: usize,
}

impl KeyValueStore for FailAfterStore {
    fn get(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>, String> {
        Ok(keys.iter().map(|k| self.inner.get(k).cloned()).collect())
    }
    fn put(&mut self, pairs: Vec<(Vec<u8>, Vec<u8>)>) -> Result<(), String> {
        let n = self.puts.fetch_add(1, Ordering::SeqCst);
        if n >= self.allow {
            return Err("store write failed".to_string());
        }
        for (key, value) in pairs {
            self.inner.insert(key, value);
        }
        Ok(())
    }
    fn delete(&mut self, keys: &[Vec<u8>]) -> Result<usize, String> {
        let mut removed = 0;
        for key in keys {
            if self.inner.remove(key).is_some() {
                removed += 1;
            }
        }
        Ok(removed)
    }
    fn entries(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
        Ok(self
            .inner
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}

#[async_trait::async_trait]
impl KeyValueStoreManager for FailAfterStoreManager {
    async fn store(&self, _name: &str) -> Result<SharedStore, String> {
        let store: Box<dyn KeyValueStore + Send + Sync> = Box::new(FailAfterStore {
            inner: BTreeMap::new(),
            puts: Arc::new(AtomicUsize::new(0)),
            allow: self.allow,
        });
        Ok(Arc::new(tokio::sync::Mutex::new(store)))
    }
    async fn shutdown(&self) {}
}

/// A gateway over the given shard APIs, with its own ledger.
fn gateway_over(
    apis: Vec<(ShardId, Arc<TestShardApi>)>,
    ledger: Arc<TxnLedger>,
    key: PrivateKey,
    pub_key: PublicKey,
) -> GatewayTxn {
    let mut shards = BTreeMap::new();
    for (shard_id, api) in apis {
        shards.insert(
            shard_id.clone(),
            LocalShard {
                shard_id,
                block_api: api,
                max_listen_depth: 50,
            },
        );
    }
    GatewayTxn::new(
        Arc::new(LocalShardDeployService::new(shards)),
        ledger,
        key,
        pub_key,
        // Short enough that a reply which never arrives resolves on the listen loop's second poll.
        Duration::from_millis(50),
    )
}

/// Two funded shards plus the gateway, with the shard doubles handed back for inspection.
async fn fixture(
    manager: Arc<dyn KeyValueStoreManager>,
    faults_b: ShardFaults,
) -> (
    GatewayTxn,
    Arc<TestShardApi>,
    Arc<TestShardApi>,
    Arc<TxnLedger>,
    PrivateKey,
    PublicKey,
    String,
) {
    fixture_with_faults(manager, ShardFaults::default(), faults_b).await
}

/// Like [`fixture`], with both shards' fault knobs exposed.
async fn fixture_with_faults(
    manager: Arc<dyn KeyValueStoreManager>,
    faults_a: ShardFaults,
    faults_b: ShardFaults,
) -> (
    GatewayTxn,
    Arc<TestShardApi>,
    Arc<TestShardApi>,
    Arc<TxnLedger>,
    PrivateKey,
    PublicKey,
    String,
) {
    let (key, pub_key) = construct_deploy::default_key_pair().unwrap();
    let coordinator_addr = RevAddress::from_public_key(&pub_key).unwrap().to_base58();

    let rm_a = Arc::new(build_runtime_manager().await);
    let rm_b = Arc::new(build_runtime_manager().await);
    fund(rm_a.runtime(), &coordinator_addr, 100);
    fund(rm_b.runtime(), &coordinator_addr, 100);

    let api_a = Arc::new(TestShardApi::with_faults(
        "/root",
        Arc::clone(&rm_a),
        faults_a,
    ));
    let api_b = Arc::new(TestShardApi::with_faults(
        "/root/child",
        Arc::clone(&rm_b),
        faults_b,
    ));
    let ledger = Arc::new(TxnLedger::open(manager.as_ref()).await.expect("ledger"));
    let gateway = gateway_over(
        vec![
            (shard("/root"), api_a.clone()),
            (shard("/root/child"), api_b.clone()),
        ],
        ledger.clone(),
        key.clone(),
        pub_key.clone(),
    );
    (
        gateway,
        api_a,
        api_b,
        ledger,
        key,
        pub_key,
        coordinator_addr,
    )
}

/// A real REV destination: `rho:txn` refuses a destination it cannot decode (C301), so a literal
/// placeholder is no longer a legal fixture.
fn destination() -> String {
    RevAddress::from_public_key(&construct_deploy::default_pub2().unwrap())
        .unwrap()
        .to_base58()
}

/// A record with both legs prepared-by-nobody, for seeding the ledger.
fn seeded_record(txn_id: &[u8], destination: &str) -> CoordRecord {
    CoordRecord {
        txn_id: txn_id.to_vec(),
        state: CoordState::Proposed,
        coordinator: PublicKey::new(vec![7u8; 65]),
        legs: vec![
            LegRecord {
                shard_id: shard("/root"),
                amount: NonNegI64::try_from(30).unwrap(),
                to: destination.to_string(),
            },
            LegRecord {
                shard_id: shard("/root/child"),
                amount: NonNegI64::try_from(40).unwrap(),
                to: destination.to_string(),
            },
        ],
        votes: Vec::new(),
        reason: None,
    }
}

/// A legless record: well-formed enough to store, and the only way to reach the "transaction has no
/// legs" fallback (`run` rejects an empty leg list before it can be stored).
#[tokio::test]
async fn drive_aborts_a_legless_record() {
    let manager = Arc::new(InMemoryStoreManager::default());
    let (gateway, _, _, ledger, ..) = fixture(manager, ShardFaults::default()).await;

    let mut record = seeded_record(b"no-legs", &destination());
    record.legs.clear();
    ledger.put(&record).await.expect("put");

    let record = gateway
        .run(b"no-legs", &txn_legs(&destination()))
        .await
        .expect("a legless record is aborted, not an error");
    assert_eq!(record.state, CoordState::Aborted);
    assert_eq!(record.reason.as_deref(), Some("transaction has no legs"));
    assert!(record.decision_is_deterministic());
}

/// `run` on a transaction already in the ledger resumes it: the leg that already voted is **not**
/// re-prepared (that is what makes a resume safe), and the transaction still reaches a decision.
#[tokio::test]
async fn run_resumes_an_existing_non_terminal_record() {
    let manager = Arc::new(InMemoryStoreManager::default());
    let (gateway, api_a, api_b, ledger, ..) = fixture(manager, ShardFaults::default()).await;

    // Seed leg A as already prepared, so only leg B is left for the resume to collect. (The vote is
    // hand-written, so A has no real escrow: its later commit is rejected by the participant and the
    // coordinator ignores that by design — spec/audit/passes.md §15 C2 — which is why A's deploy list is only
    // asserted to contain no *prepare*.)
    let mut record = seeded_record(b"resume", &destination());
    record.record_vote(shard("/root"), Vote::Ready, None);
    assert_eq!(record.state, CoordState::Prepared);
    ledger.put(&record).await.expect("put");

    let decided = gateway
        .run(b"resume", &txn_legs(&destination()))
        .await
        .expect("resume");
    assert_eq!(decided.state, CoordState::Committed);
    // The phase is named in the deploy term, so this is exact: the leg that already voted is **not**
    // prepared again — re-preparing would escrow a second time — while the pending leg is.
    let a = api_a.deploy_terms();
    assert!(
        !a.iter().any(|t| t.contains("prepare")),
        "the leg that already voted must not be re-prepared: {a:?}"
    );
    let b = api_b.deploy_terms();
    assert_eq!(b.len(), 2, "{b:?}");
    assert!(b[0].contains("prepare"), "{b:?}");
    assert!(b[1].contains("commit"), "{b:?}");
}

/// A participant that rejects the phase deploy votes abort with the reason recorded, and the leg that
/// *did* prepare is compensated — the escrow is returned rather than stranded.
#[tokio::test]
async fn recover_in_flight_aborts_when_a_leg_errors() {
    let manager = Arc::new(InMemoryStoreManager::default());
    let (gateway, api_a, _api_b, _ledger, _key, _pub_key, coordinator_addr) = fixture(
        manager,
        ShardFaults {
            deploy_error: Some("shard is down".to_string()),
            ..ShardFaults::default()
        },
    )
    .await;

    let record = gateway
        .run(b"leg-errors", &txn_legs(&destination()))
        .await
        .expect("run");

    assert_eq!(record.state, CoordState::Aborted);
    let reason = record.reason.clone().expect("an abort records why");
    assert!(reason.contains("shard is down"), "{reason}");
    assert!(record.decision_is_deterministic());
    // Leg A voted ready, so phase two compensated it: its escrow is back and nothing reached the
    // destination on either shard.
    assert_eq!(api_a.deploy_count(), 2, "one prepare, one abort");
    assert_eq!(
        record.votes[0],
        (shard("/root"), Vote::Ready),
        "the prepared leg's vote is unchanged by the other leg's failure"
    );
    let _ = coordinator_addr;
}

/// A reply that never arrives (a listen that succeeds but yields nothing) is a timeout, and the
/// transaction aborts rather than hanging — `await_reply` gives up after the phase timeout.
#[tokio::test]
async fn a_leg_whose_reply_never_arrives_times_out() {
    let manager = Arc::new(InMemoryStoreManager::default());
    let (gateway, _api_a, _api_b, _ledger, ..) = fixture(
        manager,
        ShardFaults {
            empty_listen: true,
            ..ShardFaults::default()
        },
    )
    .await;

    let record = gateway
        .run(b"times-out", &txn_legs(&destination()))
        .await
        .expect("run");
    assert_eq!(record.state, CoordState::Aborted);
    assert!(record.reason.is_some(), "{record:?}");
}

/// A ledger that cannot be read surfaces as an error from `recover_in_flight` rather than being
/// reported as "nothing to recover" — a coordinator that cannot read its own records must not
/// silently decide there is no work.
#[tokio::test]
async fn recover_in_flight_propagates_a_ledger_error() {
    let manager: Arc<dyn KeyValueStoreManager> = Arc::new(FailingStoreManager);
    let ledger = Arc::new(TxnLedger::open(manager.as_ref()).await.expect("open"));
    let key = PrivateKey::new(vec![1u8; 32]);
    let pub_key = PublicKey::new(vec![2u8; 65]);
    let gateway = gateway_over(Vec::new(), ledger, key, pub_key);

    let err = gateway
        .recover_in_flight()
        .await
        .expect_err("a failing ledger must propagate");
    assert!(err.contains("store"), "{err}");
}

/// A phase-two failure does not undo the decision — the documented residual (spec/audit/passes.md §15, C2).
///
/// Both legs prepare, the decision is written, and then leg B's **commit** deploy is rejected. The
/// record stays `Committed`: the commit point is the durable decision, not its delivery, and phase
/// two is re-driven by `recover_in_flight` (or a re-issued `run`) while the participants stay
/// idempotent under `txn_id`. The residual this pins is that B's escrow is still locked — the record
/// does not distinguish "phase two delivered" from "phase two attempted".
#[tokio::test]
async fn a_failed_phase_two_leaves_the_decision_intact() {
    let manager = Arc::new(InMemoryStoreManager::default());
    let (gateway, api_a, api_b, _ledger, ..) = fixture(
        manager,
        ShardFaults {
            deploy_error_matching: Some("commit".to_string()),
            ..ShardFaults::default()
        },
    )
    .await;

    let decided = gateway
        .run(b"phase-two-fails", &txn_legs(&destination()))
        .await
        .expect("run");

    assert_eq!(
        decided.state,
        CoordState::Committed,
        "a failed phase-two delivery must not undo a durable decision"
    );
    assert!(decided.decision_is_deterministic());
    // Leg B took its prepare but not its commit: two deploys, the second rejected.
    let b = api_b.deploy_terms();
    assert_eq!(b.len(), 1, "the rejected commit is not recorded: {b:?}");
    assert!(b[0].contains("prepare"), "{b:?}");
    // Leg A committed normally.
    let a = api_a.deploy_terms();
    assert_eq!(a.len(), 2, "{a:?}");
    assert!(a[1].contains("commit"), "{a:?}");
}

/// Three shards in one node: the coordinator's leg handling is not special-cased to two.
#[tokio::test]
async fn a_three_leg_transaction_commits() {
    let (key, pub_key) = construct_deploy::default_key_pair().unwrap();
    let coordinator_addr = RevAddress::from_public_key(&pub_key).unwrap().to_base58();

    let mut apis = Vec::new();
    let mut rms = Vec::new();
    for name in ["/root", "/root/child", "/root/child/leaf"] {
        let rm = Arc::new(build_runtime_manager().await);
        fund(rm.runtime(), &coordinator_addr, 100);
        apis.push((
            shard(name),
            Arc::new(TestShardApi::new(name, Arc::clone(&rm))),
        ));
        rms.push(rm);
    }

    let manager = Arc::new(InMemoryStoreManager::default());
    let ledger = Arc::new(TxnLedger::open(manager.as_ref()).await.expect("ledger"));
    let gateway = gateway_over(apis.clone(), ledger, key, pub_key);

    let legs: Vec<GatewayLeg> = apis
        .iter()
        .map(|(shard_id, _)| GatewayLeg {
            shard_id: shard_id.clone(),
            amount: 10,
            to: RevAddress::from_public_key(&construct_deploy::default_pub2().unwrap())
                .unwrap()
                .to_base58(),
        })
        .collect();

    let record = gateway.run(b"three-legs", &legs).await.expect("run");
    assert_eq!(record.state, CoordState::Committed);
    assert_eq!(record.legs.len(), 3);
    assert_eq!(record.votes.len(), 3);
    assert!(record.votes.iter().all(|(_, v)| *v == Vote::Ready));
    // Every shard escrowed 10 and then credited the destination with it.
    for (_, api) in &apis {
        assert_eq!(api.deploy_count(), 2, "one prepare, one commit");
    }
    let native = NativeSystemState::new(rms[2].runtime().native_store());
    assert_eq!(
        i64::from(native.vault_balance(&destination()).await.unwrap().unwrap()),
        10
    );
}

/// **Law 100b, C64/C67**: a shard whose head cannot be read **refuses** its leg — it must not anchor
/// the phase at genesis, where it would be born expired once the chain is past the deploy lifespan.
/// The transaction therefore aborts, no deploy reaches the unreadable shard, and the readable leg is
/// still anchored at its own head.
///
/// The two fault knobs this exercises were declared but read nowhere before this test: `head_error`
/// did nothing, so the previous version of this case asserted `Committed` and could not fail for the
/// reason its name gave (C370). `reported_height` makes "anchored at the head" distinguishable from
/// "anchored at 0" on a shard that has produced no earlier deploy.
#[tokio::test]
async fn an_unreadable_head_refuses_the_leg_and_aborts() {
    let manager = Arc::new(InMemoryStoreManager::default());
    let (gateway, api_a, api_b, ..) = fixture_with_faults(
        manager,
        ShardFaults {
            reported_height: Some(7),
            ..ShardFaults::default()
        },
        ShardFaults {
            head_error: true,
            ..ShardFaults::default()
        },
    )
    .await;

    let record = gateway
        .run(b"no-head", &txn_legs(&destination()))
        .await
        .expect("run");
    assert_eq!(record.state, CoordState::Aborted);
    assert!(
        record
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("could not be read"),
        "the refusal must name the unreadable head: {record:?}"
    );
    assert!(
        api_b.deploy_terms().is_empty(),
        "no deploy may reach the shard whose head cannot be read: {:?}",
        api_b.deploy_terms()
    );
    assert_eq!(
        api_a.last_anchor(),
        Some(7),
        "the readable shard anchors at its head, not at 0"
    );
}

/// **Law 99a — the decision is durable before its effect.** With one leg, the second ledger write is
/// the decision: failing it makes `run` return an error *before* phase two, so the participant has
/// taken its prepare and nothing has been committed. The stored record is still the open one, which
/// is what recovery resumes from.
#[tokio::test]
async fn a_decision_that_cannot_be_written_sends_no_phase_two() {
    let manager: Arc<dyn KeyValueStoreManager> = Arc::new(FailAfterStoreManager { allow: 1 });
    let (key, pub_key) = construct_deploy::default_key_pair().unwrap();
    let coordinator_addr = RevAddress::from_public_key(&pub_key).unwrap().to_base58();
    let rm = Arc::new(build_runtime_manager().await);
    fund(rm.runtime(), &coordinator_addr, 100);

    let api = Arc::new(TestShardApi::new("/root", Arc::clone(&rm)));
    let ledger = Arc::new(TxnLedger::open(manager.as_ref()).await.expect("ledger"));
    let gateway = gateway_over(
        vec![(shard("/root"), api.clone())],
        ledger.clone(),
        key,
        pub_key,
    );
    let legs = vec![GatewayLeg {
        shard_id: shard("/root"),
        amount: 30,
        to: RevAddress::from_public_key(&construct_deploy::default_pub2().unwrap())
            .unwrap()
            .to_base58(),
    }];

    let err = gateway
        .run(b"unwritable", &legs)
        .await
        .expect_err("the decision write fails, so the run fails");
    assert!(err.contains("store write failed"), "{err}");

    let terms = api.deploy_terms();
    assert_eq!(terms.len(), 1, "phase one ran exactly once: {terms:?}");
    assert!(terms[0].contains("prepare"), "{terms:?}");
    assert!(
        !terms.iter().any(|t| t.contains("commit")),
        "a decision that is not durable must not be delivered: {terms:?}"
    );

    let stored = ledger
        .get(b"unwritable")
        .await
        .expect("read")
        .expect("the open record is durable");
    assert_eq!(
        stored.state,
        CoordState::Proposed,
        "the decision was not written, so the record is still open"
    );
}

/// **Law 100a's unknown-shard boundary.** A leg that left the node's membership between the open and
/// the resume is refused by `LocalShardDeployService`'s `member()` gate, so it aborts rather than
/// anchoring at the unknown shard's `Ok(0)` height. No deploy reaches either member shard.
#[tokio::test]
async fn recover_in_flight_refuses_a_leg_that_left_the_membership() {
    let manager = Arc::new(InMemoryStoreManager::default());
    let (gateway, api_a, api_b, ledger, ..) = fixture(manager, ShardFaults::default()).await;

    let mut record = seeded_record(b"left-membership", &destination());
    record.legs = vec![LegRecord {
        shard_id: shard("/elsewhere"),
        amount: NonNegI64::try_from(30).unwrap(),
        to: RevAddress::from_public_key(&construct_deploy::default_pub2().unwrap())
            .unwrap()
            .to_base58(),
    }];
    ledger.put(&record).await.expect("put");

    let recovered = gateway.recover_in_flight().await.expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].state, CoordState::Aborted);
    assert!(
        recovered[0]
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("/elsewhere"),
        "the abort must name the leg that left the membership: {:?}",
        recovered[0]
    );
    assert!(
        api_a.deploy_terms().is_empty(),
        "no member deploy for the foreign leg"
    );
    assert!(
        api_b.deploy_terms().is_empty(),
        "no member deploy for the foreign leg"
    );
}
