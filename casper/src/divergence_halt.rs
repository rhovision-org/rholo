//! **The in-process divergence-halt registry** (F-U26-02, F-U26-04, F-U26-05; issue #658).
//!
//! The durable record lives in each shard's control-record store. This cell is the same fact for the
//! running process: `record_halt` writes the record store-first and then notes it here; the proposer
//! and the autopropose timer read it; the operator's acknowledgement clears it.
//!
//! **Unknown is not absence** (F-U26-04). A boot read that fails used to leave this registry empty,
//! which the proposer could not tell from "no halt on record" — so a node whose durable halt it could
//! not read would propose on a divergent view. `HaltCell::Unreadable` makes the failed read a halt of
//! its own: production stops, serving continues, and only a **successful** read retires it.
//!
//! **An acknowledgement is bound to the halt it read** (F-U26-05). Read → acknowledge → delete →
//! forget were separate awaits, so a concurrent halt B could be deleted by the acknowledgement of an
//! older A. `transition` serialises the write and the clear per shard, and the clear acknowledges the
//! **newest** standing halt, not merely the record it happened to read.
//!
//! **Why a registry keyed by shard.** The detection site is inside casper's validation path, which
//! has the shard id and the DAG but neither the proposer nor the API health cell. A map gives every
//! reader the same per-shard fact without threading a handle through the validation signatures.
//!
//! **The acting rule is deliberately conservative across shards.** A proposer is per shard, but a
//! halted shard is a consensus-relevant fault on the node that runs it. [`any`] and [`any_state`]
//! are what a caller that does not know its shard consults, and a multi-shard node stops every
//! member's production until the named shard is acknowledged. Serving continues on every shard.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use rchain_block_storage::dag::dag_storage::{BlockDagStorage, DivergenceHalt};
use rchain_shared::lock::Unpoison;

/// One shard's halt state. `Unreadable` is a halt: a record this process could not read is not a
/// record it may treat as absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HaltCell {
    Halted(DivergenceHalt),
    /// The durable record could not be read. **Unknown is not absence.**
    Unreadable {
        reason: String,
    },
}

fn halts() -> &'static Mutex<BTreeMap<String, HaltCell>> {
    static HALTS: OnceLock<Mutex<BTreeMap<String, HaltCell>>> = OnceLock::new();
    HALTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn cell() -> std::sync::MutexGuard<'static, BTreeMap<String, HaltCell>> {
    halts().lock().unpoison()
}

fn transitions() -> &'static Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// **The per-shard write/clear transaction lock.** The writer and the acknowledgement take this
/// across the store operation and the registry transition, so a halt recorded while an older
/// acknowledgement is in flight is never deleted by it. The std registry lock is taken only for the
/// map operation and is never held across an await, so the order is always transition → registry.
pub async fn transition(shard_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut locks = transitions().lock().unpoison();
        locks
            .entry(shard_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}

/// Explicitly set this shard's cell. `Some` records a halt; `None` clears whatever is there. Used by
/// tests and by callers that have already decided the state; the boot path uses [`seed_boot`].
pub fn seed(shard_id: &str, record: Option<DivergenceHalt>) {
    let mut halts = cell();
    match record {
        Some(record) => {
            halts.insert(shard_id.to_string(), HaltCell::Halted(record));
        }
        None => {
            halts.remove(shard_id);
        }
    }
}

/// **The boot transition.** A successful read resolves an `Unreadable` cell, but it never weakens a
/// `Halted` cell a live detection recorded while the read was in flight — the live halt is the newer
/// fact, and it names the block the operator must acknowledge.
pub fn seed_boot(shard_id: &str, record: Option<DivergenceHalt>) {
    let mut halts = cell();
    if matches!(halts.get(shard_id), Some(HaltCell::Halted(_))) {
        return;
    }
    match record {
        Some(record) => {
            halts.insert(shard_id.to_string(), HaltCell::Halted(record));
        }
        None => {
            halts.remove(shard_id);
        }
    }
}

/// Mark this shard unknown because its record could not be read. A named halt is the stronger fact,
/// so an existing `Halted` cell is kept.
pub fn seed_unreadable(shard_id: &str, reason: impl Into<String>) {
    cell()
        .entry(shard_id.to_string())
        .or_insert_with(|| HaltCell::Unreadable {
            reason: reason.into(),
        });
}

/// Record a divergence halt for this shard. The caller has already written the durable record (or
/// has a `durable: false` record to say the write did not land) — this is the in-process half.
pub fn note(shard_id: &str, record: DivergenceHalt) {
    cell().insert(shard_id.to_string(), HaltCell::Halted(record));
}

/// **One way out**: the operator's acknowledgement. Returns the record that stood, so the caller can
/// name the block it acknowledged. The production path is [`acknowledge_divergence_halt`], which
/// also performs the durable clear under the transition lock.
pub fn clear(shard_id: &str) -> Option<DivergenceHalt> {
    match cell().remove(shard_id) {
        Some(HaltCell::Halted(record)) => Some(record),
        _ => None,
    }
}

/// This shard's named halt, if it has one. An `Unreadable` cell is a halt but has no block to name;
/// use [`state_for_shard`] for the full state.
pub fn for_shard(shard_id: &str) -> Option<DivergenceHalt> {
    match cell().get(shard_id) {
        Some(HaltCell::Halted(record)) => Some(*record),
        _ => None,
    }
}

/// This shard's full state, if any.
pub fn state_for_shard(shard_id: &str) -> Option<HaltCell> {
    cell().get(shard_id).cloned()
}

/// The first named halt in the registry, by shard id.
pub fn any() -> Option<(String, DivergenceHalt)> {
    cell().iter().find_map(|(shard, state)| match state {
        HaltCell::Halted(record) => Some((shard.clone(), *record)),
        HaltCell::Unreadable { .. } => None,
    })
}

/// The first halt of either kind, by shard id, preferring a named halt so a message can name a block
/// whenever one exists.
pub fn any_state() -> Option<(String, HaltCell)> {
    let halts = cell();
    halts
        .iter()
        .find(|(_, state)| matches!(state, HaltCell::Halted(_)))
        .or_else(|| halts.iter().next())
        .map(|(shard, state)| (shard.clone(), state.clone()))
}

/// Whether any member shard is halted, **including unknown**. What the proposer and the autopropose
/// timer consult.
pub fn halted() -> bool {
    !cell().is_empty()
}

/// **The writer's one path**: write the record store-first, then note it. The transition lock makes
/// the write and the latch one transaction against an acknowledgement of an older halt.
pub async fn record_halt(
    dag: &dyn BlockDagStorage,
    shard_id: &str,
    mut halt: DivergenceHalt,
) -> Result<(), String> {
    let _transition = transition(shard_id).await;
    let result = dag.write_divergence_halt(halt).await;
    if result.is_err() {
        // The surface must not claim the next process will know about this halt.
        halt.durable = false;
    }
    note(shard_id, halt);
    result
}

/// What an acknowledgement did, for the caller's message and the tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AckOutcome {
    /// No halt stood (and the record read successfully).
    NoHalt,
    /// The named halt was acknowledged and cleared.
    Cleared { block: String },
}

/// **The one way out, as one transaction.** Read the standing halt, require the acknowledgement to
/// name it, clear the durable record, and forget the registry cell — all under the shard's
/// transition lock.
///
/// A read failure is a refusal, not a resume: the node stays halted because the record cannot be
/// read. A **successful** read is knowledge, so it retires an `Unreadable` cell even when the record
/// names no halt. If the store holds no record but the registry holds a non-durable halt (the halt's
/// write failed), that halt still requires its acknowledgement — otherwise its only exit would be
/// unreachable.
pub async fn acknowledge_divergence_halt(
    dag: &dyn BlockDagStorage,
    shard_id: &str,
    ack: Option<&str>,
) -> Result<AckOutcome, String> {
    let _transition = transition(shard_id).await;
    let stored = match dag.read_divergence_halt().await {
        Ok(record) => record,
        Err(e) => {
            seed_unreadable(shard_id, format!("the resume read failed: {e}"));
            return Err(format!(
                "the divergence halt record for shard {shard_id} could not be read, so the resume \
                 is refused (unknown, not absent): {e}"
            ));
        }
    };
    let standing = for_shard(shard_id).or(stored);
    let outcome = match standing {
        Some(halt) => {
            acknowledgement(ack, &halt)?;
            if stored.is_some() {
                dag.clear_divergence_halt().await?;
            }
            AckOutcome::Cleared {
                block: rchain_shared::base16::encode(&halt.block_hash),
            }
        }
        None => AckOutcome::NoHalt,
    };
    // A successful read is knowledge: it retires `Unreadable`. The transition lock means no halt can
    // be written between the read and this line.
    seed(shard_id, None);
    Ok(outcome)
}

/// **The operator's acknowledgement must name the refused block.** The pure decision, so the resume
/// route's refusal is testable without a transport or a DAG.
pub fn acknowledgement(ack: Option<&str>, halt: &DivergenceHalt) -> Result<(), String> {
    let expected = rchain_shared::base16::encode(&halt.block_hash);
    match ack {
        Some(ack) if ack.eq_ignore_ascii_case(&expected) => Ok(()),
        Some(ack) => Err(format!(
            "the divergence halt names block {expected} (seq {}); the acknowledgement {ack} does \
             not match it",
            halt.seq_num
        )),
        None => Err(format!(
            "a divergence halt stands for block {expected} (seq {}); resume with \
             acknowledgeDivergence={expected}, or it stands until it is acknowledged",
            halt.seq_num
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(block_byte: u8) -> DivergenceHalt {
        DivergenceHalt {
            at_ms: 1,
            block_hash: [block_byte; 32],
            seq_num: 7,
            episodes: 1,
            durable: true,
        }
    }

    /// The registry-mutating tests live in `casper/tests/divergence_halt.rs`, under that file's
    /// `TEST_LOCK`: this module is compiled into the same test binary as the proposer tests, which
    /// read the process-global registry, and Rust runs unit tests in parallel. Only the pure decision
    /// belongs here.
    #[test]
    fn an_acknowledgement_for_another_block_does_not_clear_the_halt() {
        let halt = record(0x04);
        let expected = rchain_shared::base16::encode(&halt.block_hash);
        assert!(acknowledgement(None, &halt).is_err());
        assert!(acknowledgement(Some("deadbeef"), &halt).is_err());
        assert!(acknowledgement(Some(&expected), &halt).is_ok());
        assert!(
            acknowledgement(Some(&expected.to_uppercase()), &halt).is_ok(),
            "the hash comparison is case-insensitive"
        );
        // The refusal names the block the operator must pass.
        let message = acknowledgement(None, &halt).expect_err("a missing ack is refused");
        assert!(message.contains(&expected));
    }
}
