//! RSpace errors.
//!
//! Hard error type for the rspace crate's declared partiality boundaries (lock poisoning, codec
//! decode, history sum-type invariants, storage commit, replay invariants). Mirrors the typed-fix
//! column of `spec/TYPE-SYSTEM.md` §3.2.

use std::fmt;

/// An rspace error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RSpaceError {
    /// A lock was poisoned (a panic occurred while it was held).
    LockPoisoned,
    /// A codec decode failed at the serialization boundary.
    Codec(&'static str),
    /// A history leaf had an unexpected variant for the requested key.
    UnexpectedLeaf(&'static str),
    /// A history commit failed.
    HistoryCommitFailed,
    /// `install` was attempted outside of startup.
    InstallNotAllowed,
    /// A **different** continuation was installed on a channel that already carries one.
    ///
    /// `installed_continuations` is keyed by the channel and holds one `WaitingContinuation`, so a
    /// second install used to replace the first **silently** — which is how a minted vault handle's
    /// `balance` arm came to have never existed while its `transfer` arm worked (AUDIT C219, and its
    /// law is `Rchain`'s Law 60: an install that cannot take effect is refused rather than dropped).
    /// A contract that serves several methods installs **one** continuation at
    /// `arity: 1, remainder: true` and dispatches on the method inside it, as `rev_vault` and `ertp`
    /// do; an idempotent re-install of the same consume stays a no-op (`locked_install` skips it
    /// before reaching here).
    InstallConflict(&'static str),
    /// A cached key was unexpectedly missing.
    CachedKeyMissing,
    /// **A soft checkpoint's revert was refused because the history base moved since the snapshot**
    /// (Law 74b). The snapshot's overlay belongs over the base it was taken over; rebuilding it over
    /// a newer base would apply the old overlay to the wrong trie, so the revert does nothing.
    SoftCheckpointBaseMoved,
    /// A radix action key had an empty prefix.
    EmptyPrefix,
    /// Replay data was expected to be empty at checkpoint.
    ReplayDataNotEmpty,
    /// A recomputed COMM event was not present in the recorded replay trace (a peer-supplied
    /// event log is self-inconsistent — the block is invalid, not a reason to panic).
    ReplayCommNotInTrace,
    /// A scheduled consume had an empty channel set or a channel/pattern arity mismatch (a caller
    /// error at the scheduling boundary, not an internal invariant to panic on).
    ConsumeArity(&'static str),
    /// The matcher itself failed, as opposed to the pattern not matching the datum.
    ///
    /// The oracle's `Match.get` returns `F[Option[A]]` (`rspace/.../Match.scala:11`), so "no match"
    /// and "the match could not be decided" are different answers; the port's trait returned a bare
    /// `Option`, which made them the same one (AUDIT C52). The payload is the message from the layer
    /// that knows — `RholangError::BugFoundError` for a pattern whose declared `free_count` outruns
    /// what the matcher bound.
    MatcherFailed(String),
}

impl fmt::Display for RSpaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RSpaceError::LockPoisoned => write!(f, "lock poisoned"),
            RSpaceError::Codec(what) => write!(f, "decode {what} failed"),
            RSpaceError::UnexpectedLeaf(what) => {
                write!(f, "unexpected leaf while looking for {what}")
            }
            RSpaceError::HistoryCommitFailed => write!(f, "history commit failed"),
            RSpaceError::InstallNotAllowed => write!(f, "installing can be done only on startup"),
            RSpaceError::InstallConflict(what) => {
                write!(
                    f,
                    "a channel already carries a different installed continuation: {what}"
                )
            }
            RSpaceError::CachedKeyMissing => write!(f, "cached key must be present"),
            RSpaceError::SoftCheckpointBaseMoved => write!(
                f,
                "soft checkpoint base moved since the snapshot; revert refused (Law 74b)"
            ),
            RSpaceError::EmptyPrefix => write!(f, "prefix must be non-empty"),
            RSpaceError::ReplayDataNotEmpty => write!(f, "replay data must be empty at checkpoint"),
            RSpaceError::ReplayCommNotInTrace => {
                write!(f, "COMM event was not contained in the trace")
            }
            RSpaceError::ConsumeArity(what) => write!(f, "invalid scheduled consume: {what}"),
            RSpaceError::MatcherFailed(what) => write!(f, "matcher failed: {what}"),
        }
    }
}

impl std::error::Error for RSpaceError {}

/// Convenience result alias.
pub type Result<A> = std::result::Result<A, RSpaceError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant renders, and the three that carry a `&'static str` interpolate it — an error
    /// that dropped its detail would leave a reader with "decode  failed" and no idea which decoder.
    #[test]
    fn every_variant_renders_and_the_carrying_variants_keep_their_detail() {
        let cases: [(RSpaceError, &str); 12] = [
            (RSpaceError::LockPoisoned, "lock poisoned"),
            (RSpaceError::Codec("rnd"), "decode rnd failed"),
            (
                RSpaceError::UnexpectedLeaf("produces"),
                "unexpected leaf while looking for produces",
            ),
            (RSpaceError::HistoryCommitFailed, "history commit failed"),
            (
                RSpaceError::InstallNotAllowed,
                "installing can be done only on startup",
            ),
            (
                RSpaceError::InstallConflict("install one continuation at arity 1"),
                "a channel already carries a different installed continuation: install one \
                 continuation at arity 1",
            ),
            (RSpaceError::CachedKeyMissing, "cached key must be present"),
            (RSpaceError::EmptyPrefix, "prefix must be non-empty"),
            (
                RSpaceError::ReplayDataNotEmpty,
                "replay data must be empty at checkpoint",
            ),
            (
                RSpaceError::ReplayCommNotInTrace,
                "COMM event was not contained in the trace",
            ),
            (
                RSpaceError::ConsumeArity("2 channels, 1 pattern"),
                "invalid scheduled consume: 2 channels, 1 pattern",
            ),
            (
                RSpaceError::SoftCheckpointBaseMoved,
                "soft checkpoint base moved since the snapshot; revert refused (Law 74b)",
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
            // The detail is not a constant: two errors of the same variant differ.
            assert!(
                !error.to_string().contains("{}"),
                "no unformatted placeholder"
            );
        }
        assert_ne!(
            RSpaceError::Codec("a").to_string(),
            RSpaceError::Codec("b").to_string()
        );
    }

    /// It is a real `Error` value (so `?` into `Box<dyn Error>` works) and it compares structurally,
    /// which is what lets callers assert on a specific failure.
    #[test]
    fn it_is_an_error_value_that_compares_structurally() {
        let boxed: Box<dyn std::error::Error> = Box::new(RSpaceError::EmptyPrefix);
        assert_eq!(boxed.to_string(), "prefix must be non-empty");

        assert_eq!(RSpaceError::Codec("x"), RSpaceError::Codec("x"));
        assert_ne!(RSpaceError::Codec("x"), RSpaceError::Codec("y"));
        assert_ne!(
            RSpaceError::CachedKeyMissing,
            RSpaceError::HistoryCommitFailed
        );
    }
}
