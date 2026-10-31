//! **The deploy pool's admission policy** (Law 86a): count, term-byte, per-deployer share and rate.
//!
//! These are the only bounds that refuse an otherwise valid deploy. They are pure predicates here
//! so the runtime and the `admission` conformance corpus run the same arithmetic; `casper/src/dag.rs`
//! owns the state (the LMDB store, the per-deployer map and the byte counter) and calls these, and
//! `casper/tests/lean_admission_corpus.rs` drives them from the Lean-emitted cases.
//!
//! Memory is not an input. A node with a free slot admits whatever the host's headroom says; the
//! headroom permit budget (`casper::budget`) bounds how much executes, and the relay offers the rest
//! to peers (Law 86d/e).

use std::fmt;

/// The pool's bounds. `casper/src/dag.rs` builds one from its public constants; the corpus builds
/// the same shape with small numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolLimits {
    pub max_deploys: usize,
    pub max_bytes: usize,
    pub per_deployer: usize,
}

/// Why an admission or commit was refused. The variants are the pool's own, so the HTTP/gRPC
/// classification (`casper::api::block_api::deploy_refusal_is_bounded`) can name the bound instead of
/// matching prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolError {
    Full,
    ByteFull,
    DeployerShare,
    RateLimited,
    Duplicate,
}

impl PoolError {
    /// The stable phrase the API surfaces carry. A dynamic duplicate id is appended by the caller.
    pub fn message(self) -> &'static str {
        match self {
            PoolError::Full => "deploy pool is full",
            PoolError::ByteFull => "deploy pool is full: term-byte budget exhausted",
            PoolError::DeployerShare => "this deployer holds its share of the deploy pool",
            PoolError::RateLimited => "deploy rate limit exceeded for this deployer",
            PoolError::Duplicate => "deploy is already in the pool",
        }
    }
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

/// **One admission, in order**: rate (the submitter's own bound), then its share, then the node's
/// count. `rate_ok` is the limiter's answer, passed in so this module stays free of the limiter's
/// state.
pub fn check_admission(
    limits: &PoolLimits,
    count: usize,
    held: usize,
    rate_ok: bool,
) -> Result<(), PoolError> {
    if !rate_ok {
        return Err(PoolError::RateLimited);
    }
    if held >= limits.per_deployer {
        return Err(PoolError::DeployerShare);
    }
    if count >= limits.max_deploys {
        return Err(PoolError::Full);
    }
    Ok(())
}

/// **The commit's checks**, under the admission lock: duplicate, count, then bytes. The duplicate
/// is checked first because it is the one a resubmission reaches; the count and byte checks are the
/// exact bound that the early admission estimate cannot promise.
pub fn check_commit(
    limits: &PoolLimits,
    count: usize,
    bytes_in_use: usize,
    term_bytes: usize,
    duplicate: bool,
) -> Result<(), PoolError> {
    if duplicate {
        return Err(PoolError::Duplicate);
    }
    if count >= limits.max_deploys {
        return Err(PoolError::Full);
    }
    if bytes_in_use.saturating_add(term_bytes) > limits.max_bytes {
        return Err(PoolError::ByteFull);
    }
    Ok(())
}

/// Whether the pool has room under both the count and the term-byte bounds. The per-deployer share
/// and the rate are submitter bounds, not pool capacity.
pub fn has_room(limits: &PoolLimits, count: usize, bytes: usize) -> bool {
    count < limits.max_deploys && bytes < limits.max_bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> PoolLimits {
        PoolLimits {
            max_deploys: 10,
            max_bytes: 100,
            per_deployer: 3,
        }
    }

    #[test]
    fn a_free_slot_admits_and_each_bound_refuses_by_name() {
        assert_eq!(check_admission(&limits(), 0, 0, true), Ok(()));
        assert_eq!(
            check_admission(&limits(), 0, 0, false),
            Err(PoolError::RateLimited)
        );
        assert_eq!(
            check_admission(&limits(), 0, 3, true),
            Err(PoolError::DeployerShare)
        );
        assert_eq!(
            check_admission(&limits(), 10, 0, true),
            Err(PoolError::Full)
        );
    }

    #[test]
    fn a_commit_checks_duplicate_count_then_bytes() {
        assert_eq!(check_commit(&limits(), 0, 0, 10, false), Ok(()));
        assert_eq!(
            check_commit(&limits(), 0, 0, 10, true),
            Err(PoolError::Duplicate)
        );
        assert_eq!(
            check_commit(&limits(), 10, 0, 10, false),
            Err(PoolError::Full)
        );
        assert_eq!(
            check_commit(&limits(), 0, 95, 10, false),
            Err(PoolError::ByteFull)
        );
        assert_eq!(check_commit(&limits(), 0, 90, 10, false), Ok(()));
    }

    #[test]
    fn room_is_count_and_bytes_only() {
        assert!(has_room(&limits(), 9, 99));
        assert!(!has_room(&limits(), 10, 0));
        assert!(!has_room(&limits(), 0, 100));
    }
}
