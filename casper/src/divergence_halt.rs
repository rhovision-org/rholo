//! **The in-process divergence-halt registry** (F-U26-02, issue #658).
//!
//! The durable record lives in each shard's control-record store. This cell is the same fact for the
//! running process: `note_unrestorable_if_exhausted` writes the record store-first and then notes it
//! here; the proposer and the autopropose timer read it; the operator's acknowledgement clears it.
//!
//! **Why a registry keyed by shard.** The detection site is inside casper's validation path, which
//! has the shard id and the DAG but neither the proposer nor the API health cell. A map gives every
//! reader the same per-shard fact without threading a handle through the validation signatures.
//!
//! **The acting rule is deliberately conservative across shards.** A proposer is per shard, but a
//! halted shard is a consensus-relevant fault on the node that runs it. [`any`] is what a caller that
//! does not know its shard consults, and a multi-shard node stops every member's production until the
//! named shard is acknowledged. Serving continues on every shard.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use rchain_block_storage::dag::dag_storage::DivergenceHalt;
use rchain_shared::lock::Unpoison;

fn halts() -> &'static Mutex<BTreeMap<String, DivergenceHalt>> {
    static HALTS: OnceLock<Mutex<BTreeMap<String, DivergenceHalt>>> = OnceLock::new();
    HALTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn cell() -> std::sync::MutexGuard<'static, BTreeMap<String, DivergenceHalt>> {
    halts().lock().unpoison()
}

/// Seed this shard's cell from its durable record at boot. `None` clears it; the caller keeps a read
/// failure as "unknown" at the surface rather than passing `None` for it.
pub fn seed(shard_id: &str, record: Option<DivergenceHalt>) {
    let mut halts = cell();
    match record {
        Some(record) => {
            halts.insert(shard_id.to_string(), record);
        }
        None => {
            halts.remove(shard_id);
        }
    }
}

/// Record a divergence halt for this shard. The caller has already written the durable record (or
/// has a `durable: false` record to say the write did not land) — this is the in-process half.
pub fn note(shard_id: &str, record: DivergenceHalt) {
    cell().insert(shard_id.to_string(), record);
}

/// **The one way out**: the operator's acknowledgement. Returns the record that stood, so the caller
/// can name the block it acknowledged.
pub fn clear(shard_id: &str) -> Option<DivergenceHalt> {
    cell().remove(shard_id)
}

/// This shard's halt, if it has one.
pub fn for_shard(shard_id: &str) -> Option<DivergenceHalt> {
    cell().get(shard_id).copied()
}

/// The first halt in the registry, by shard id. A caller that knows its shard should use
/// [`for_shard`]; this is the conservative answer for one that does not.
pub fn any() -> Option<(String, DivergenceHalt)> {
    cell()
        .iter()
        .next()
        .map(|(shard, record)| (shard.clone(), *record))
}

/// Whether any member shard is halted. What the proposer and the autopropose timer consult.
pub fn halted() -> bool {
    !cell().is_empty()
}

/// **The operator's acknowledgement must name the refused block.** The pure decision, so the resume
/// route's refusal is testable without a transport or a `BlockApiImpl`.
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

    /// The registry is process-global, so the test takes the lock and uses shard ids no other test
    /// reads.
    #[test]
    fn a_halt_is_per_shard_and_the_any_answer_is_conservative() {
        let shard = "test/divergence-a";
        let other = "test/divergence-b";
        seed(shard, None);
        seed(other, None);
        note(shard, record(0x01));
        assert!(for_shard(shard).is_some());
        assert!(for_shard(other).is_none());
        assert!(halted());
        assert_eq!(any().expect("a halt").0, shard);
        let cleared = clear(shard).expect("the acknowledged record");
        assert_eq!(cleared.block_hash, [0x01; 32]);
        assert!(for_shard(shard).is_none());
        seed(other, None);
    }

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

    #[test]
    fn a_boot_seed_replaces_rather_than_stacks() {
        let shard = "test/divergence-seed";
        seed(shard, Some(record(0x02)));
        seed(shard, Some(record(0x03)));
        assert_eq!(for_shard(shard).expect("seeded").block_hash, [0x03; 32]);
        seed(shard, None);
        assert!(for_shard(shard).is_none());
    }
}
