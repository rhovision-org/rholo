//! Proposal result types (port of `blocks/proposer/ProposeResult.scala`).

use std::fmt;

use rchain_models::casper::protocol::casper_message::BlockMessage;

/// The outcome of a proposal (port of `ProposeStatus`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposeStatus {
    ProposeSuccess,
    /// **A self-replay refusal was recovered by an attestation-only block** (#564). The refused
    /// deploy set stays pooled; the block that was broadcast has no user deploys. This is a success
    /// for liveness and a recorded fault for the operator — not a silent retry.
    SelfReplayRecovered,
    InternalDeployError,
    BugError(String),
    NotBonded,
    NotEnoughNewBlocks,
    TooFarAheadOfLastFinalized,
    NoNewDeploys,
    /// Unfinalized work remains; a bounded automatic attempt must revisit its pacing gate.
    AwaitingFinality,
    /// The node's resource fallback state is `Critical` or `Unavailable`: it has stopped producing
    /// (unit C, barrier B37). The word is the state that withheld the proposal.
    ResourcePressure(String),
    /// A self-replay fault survived the attestation fallback and the reset either is not enabled or
    /// did not clear it. Every proposal trigger is stopped until an operator resumes. This is the
    /// loud terminal state the incident lacked.
    SelfReplayFaultHalted,
    /// **This validator's own key has equivocated** (#576): the DAG refused a block by this key
    /// because a different block by the same key already holds its sequence. Proposing would deepen
    /// the fork, so every trigger refuses — **permanently**: the proof is a durable verdict, not a
    /// transient fault, and `POST /api/v1/autopropose/resume` does not clear it. The reason names
    /// the refused block.
    KeyEquivocationHalted(String),
    /// **The durable record of this validator's own signatures is ahead of the round snapshot**
    /// (Law 122). A rewind or a light join dropped the block that carried a later sequence, so the
    /// snapshot's next number would duplicate one the network already has — an equivocation the
    /// network slashes. The node waits for the catch-up to re-adopt the block; it never signs below
    /// the record.
    OwnSeqNotReconciled {
        signed: i64,
        signed_height: i64,
        snapshot: Option<i64>,
    },
}

/// **What a proposal attempt is allowed to carry** (#564).
///
/// `Normal` selects pooled deploys as usual. `AttestOnly` is the safe recovery transition after a
/// self-replay refusal: the block is built over the same pre-state with **no user deploys** (the
/// system deploys the pre-state requires stay), so it cannot repeat the refused deploy replay and it
/// still moves this validator's latest message. The refused deploys remain in the pool and are
/// retried on a later pre-state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalMode {
    Normal,
    AttestOnly,
}

impl fmt::Display for ProposeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProposeStatus::ProposeSuccess => write!(f, "Propose succeed: Valid"),
            ProposeStatus::SelfReplayRecovered => {
                write!(
                    f,
                    "Propose succeeded by attestation after a self-replay refusal"
                )
            }
            ProposeStatus::SelfReplayFaultHalted => {
                write!(f, "Proposal halted: self-replay fault")
            }
            ProposeStatus::KeyEquivocationHalted(reason) => {
                write!(
                    f,
                    "Proposal halted: this validator's key equivocated ({reason})"
                )
            }
            ProposeStatus::OwnSeqNotReconciled {
                signed,
                signed_height,
                snapshot,
            } => write!(
                f,
                "Proposal deferred: this validator's own signature is recorded at sequence {signed} \
                 (height {signed_height}) but the round snapshot shows {snapshot:?}; waiting for the \
                 catch-up to re-adopt it rather than sign a duplicate (Law 122)"
            ),
            ProposeStatus::AwaitingFinality => {
                write!(f, "Proposal deferred: unfinalized work awaits its retry")
            }
            ProposeStatus::NoNewDeploys => write!(f, "Proposal failed: NoNewDeploys"),
            ProposeStatus::InternalDeployError => {
                write!(f, "Proposal failed: internal deploy error")
            }
            ProposeStatus::NotBonded => write!(f, "Proposal failed: ReadOnlyMode"),
            ProposeStatus::NotEnoughNewBlocks => {
                write!(
                    f,
                    "Proposal failed: Must wait for more blocks from other validators"
                )
            }
            ProposeStatus::TooFarAheadOfLastFinalized => {
                write!(
                    f,
                    "Proposal failed: too far ahead of the last finalized block"
                )
            }
            ProposeStatus::BugError(reason) => {
                write!(f, "Proposal failed: internal error ({reason})")
            }
            ProposeStatus::ResourcePressure(state) => {
                write!(f, "Proposal withheld: resource pressure ({state})")
            }
        }
    }
}

/// A proposal result (port of `ProposeResult`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposeResult {
    pub propose_status: ProposeStatus,
}

/// The result of checking propose constraints (port of `CheckProposeConstraintsResult`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckProposeConstraintsResult {
    Success,
    NotBonded,
    NotEnoughNewBlocks,
    TooFarAheadOfLastFinalized,
}

/// The result of creating a block (port of `BlockCreatorResult`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockCreatorResult {
    NoNewDeploys,
    /// Unfinalized work remains; a bounded automatic attempt must revisit its pacing gate.
    AwaitingFinality,
    /// This validator has already produced a block since the last round boundary, and a second one in the
    /// same round would equivocate against itself — see `DagMessageState::has_advanced_past_the_round`. The
    /// proposer takes this only while the round can still close; past the elapsed `ROUND_STALL_ESCAPE` interval it escapes.
    AlreadyProposedThisRound,
    /// The node's tip is beyond the bounded distance above its last finalized block. It must not
    /// extend a chain no peer has accepted; the proposer returns
    /// [`ProposeStatus::TooFarAheadOfLastFinalized`] and the node stops proposing/attesting.
    TooFarAheadOfLastFinalized,
    /// The validator's own signed-sequence record is ahead of the round snapshot (Law 122): the
    /// DAG no longer holds a block this key signed, so the snapshot's next sequence would duplicate
    /// one the network already has. The proposer waits for the catch-up to re-adopt it.
    OwnSeqNotReconciled {
        signed: i64,
        signed_height: i64,
        snapshot: Option<i64>,
    },
    /// A self-replay fault is latched: every proposal trigger is stopped until an operator resumes.
    SelfReplayFaultHalted,
    Created(BlockMessage),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propose_status_displays() {
        assert_eq!(
            ProposeStatus::NoNewDeploys.to_string(),
            "Proposal failed: NoNewDeploys"
        );
        assert_eq!(
            ProposeStatus::ProposeSuccess.to_string(),
            "Propose succeed: Valid"
        );
        // #576: the sentence names the refused block, and it is the phrase the arm and the
        // operator both read.
        assert_eq!(
            ProposeStatus::KeyEquivocationHalted("block #14".to_string()).to_string(),
            "Proposal halted: this validator's key equivocated (block #14)"
        );
    }
}
