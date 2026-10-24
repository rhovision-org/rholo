//! **Law 80 / B40, HAZOP F-U17-10: `CURRENT_ROOT` names only an accepted block.**
//!
//! Validation replays a block's deploys, writes a checkpoint and only then calls `dag.insert`
//! (`casper/src/blocks/block_processor.rs`). The checkpoint used to publish the candidate post-state,
//! so a block refused for a bad state hash — or dropped by a rewind — left the durable pointer naming
//! a state no accepted block carried. The fix splits the write: every checkpoint records its root as
//! **known**, and only an accepted block publishes it.
//!
//! What these tests pin, at the level where the split is observable:
//!
//! 1. a checkpoint moves no `CURRENT_ROOT` and records the root the acceptance will publish;
//! 2. the publish refuses a root no checkpoint recorded (so "publish" cannot mint a pointer);
//! 3. an insert that never accepts publishes nothing — the state a refused block leaves behind is the
//!    one the store was already at.
//!
//! The processor's own failure path is the **absence** of the publish call, so arm 3 exercises the
//! store's invariant the path relies on rather than a fabricated block fixture. The live chain arm is
//! `tools/n420-fork-point.py` and the boot reconciliation in `casper/src/engine/node_launch.rs`.

mod common;

use std::collections::BTreeSet;

use rchain_casper::runtime_manager::RuntimeManager;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_crypto::public_key::PublicKey;
use rchain_models::validator::Validator;
use rchain_rholang::native_state::NativeSystemState;
use rchain_rholang::util::rev_address::RevAddress;
use rchain_shared::refined::NonNegI64;

use common::build_runtime_manager;

const DEPLOYER: [u8; 65] = [1u8; 65];

/// A native write, so the next checkpoint has a root distinct from the one before it.
async fn seed(rm: &RuntimeManager) {
    let native = NativeSystemState::new(rm.runtime().native_store());
    let pk = PublicKey::new(DEPLOYER.to_vec());
    let addr = RevAddress::from_public_key(&pk)
        .expect("the test key is a valid address")
        .to_base58();
    native.set_vault_balance(
        &addr,
        NonNegI64::try_from(1_000_000_000).expect("a test balance"),
    );
    native.set_trusted(&BTreeSet::from([Validator::new(DEPLOYER)]));
}

#[tokio::test]
async fn a_candidate_checkpoint_records_the_root_without_publishing_it() {
    let rm = build_runtime_manager().await;
    let before = rm
        .get_history_repo()
        .published_root()
        .await
        .expect("the current root");

    seed(&rm).await;
    let candidate = rm
        .runtime()
        .create_checkpoint()
        .await
        .expect("a candidate checkpoint")
        .root;
    assert_ne!(
        Some(candidate),
        before,
        "the fixture must produce a new root"
    );
    assert_eq!(
        rm.get_history_repo()
            .published_root()
            .await
            .expect("the current root"),
        before,
        "a checkpoint records a candidate root as known; it does not move CURRENT_ROOT (law 80)"
    );

    // The acceptance half: a known root publishes, and only then does the pointer move.
    assert!(
        rm.publish_accepted_root(candidate)
            .await
            .expect("the publish runs"),
        "a root the checkpoint recorded is publishable"
    );
    assert_eq!(
        rm.get_history_repo()
            .published_root()
            .await
            .expect("the current root"),
        Some(candidate),
        "acceptance moves CURRENT_ROOT to the accepted post-state"
    );
}

/// The HAZOP-named falsifier: a validation that checkpoints and then fails to insert leaves the
/// pointer where it was, because nothing published the candidate.
#[tokio::test]
async fn a_failed_insert_leaves_the_current_root_where_it_was() {
    let rm = build_runtime_manager().await;
    let before = rm
        .get_history_repo()
        .published_root()
        .await
        .expect("the current root");

    seed(&rm).await;
    let candidate = rm
        .runtime()
        .create_checkpoint()
        .await
        .expect("validation's checkpoint")
        .root;
    assert_ne!(Some(candidate), before);
    // The insert fails: the publish call is never reached.
    assert_eq!(
        rm.get_history_repo()
            .published_root()
            .await
            .expect("the current root"),
        before,
        "a refused block must not move CURRENT_ROOT"
    );

    // And the publish cannot be used to mint a pointer to a state no checkpoint recorded.
    let unknown = Blake2b256Hash::from_bytes([0xAB; 32]);
    assert!(
        !rm.publish_accepted_root(unknown)
            .await
            .expect("the refusal runs"),
        "an unknown root is refused"
    );
    assert_eq!(
        rm.get_history_repo()
            .published_root()
            .await
            .expect("the current root"),
        before
    );
}
