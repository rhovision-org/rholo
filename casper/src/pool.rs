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

/// The per-deployer rate refusal. The ingress middleware uses this constant directly, so the
/// limiter and the pool cannot drift into two spellings the classifier does not know.
pub const RATE_LIMITED_MESSAGE: &str = "deploy rate limit exceeded";

impl PoolError {
    /// The stable phrase the API surfaces carry. Dynamic variants (a duplicate id, the byte
    /// counts in use) are built by [`PoolError::duplicate_message`],
    /// [`PoolError::admitting_message`] and [`PoolError::byte_full_message`], so this module is
    /// the only place the refusal prose is written.
    pub fn message(self) -> &'static str {
        match self {
            PoolError::Full => "deploy pool is full",
            PoolError::ByteFull => "deploy pool is full: term-byte budget exhausted",
            PoolError::DeployerShare => "this deployer holds its share of the deploy pool",
            PoolError::RateLimited => RATE_LIMITED_MESSAGE,
            PoolError::Duplicate => "deploy is already in the pool",
        }
    }

    /// The byte bound with the counts in use. This exact shape is what [`classify`] reads; a
    /// message built anywhere else is not a bound.
    pub fn byte_full_message(in_use: usize, max_bytes: usize) -> String {
        format!("deploy pool is full: {in_use} of {max_bytes} term bytes in use")
    }

    /// A deploy already committed to the pool.
    pub fn duplicate_message(id: &str) -> String {
        format!("deploy {id} is already in the pool")
    }

    /// A deploy another admission is currently committing.
    pub fn admitting_message(id: &str) -> String {
        format!("deploy {id} is already being admitted")
    }

    /// Whether this refusal is a bound ("come back"), not a malformed request. A duplicate is
    /// not: the request is well formed and the answer is 400.
    pub fn is_bounded(self) -> bool {
        matches!(
            self,
            PoolError::Full
                | PoolError::ByteFull
                | PoolError::DeployerShare
                | PoolError::RateLimited
        )
    }
}

/// **The one classifier** the HTTP and gRPC surfaces share. It reads the stable phrases and the
/// two dynamic shapes exactly; arbitrary prose that merely contains a phrase is not a bound.
pub fn classify(message: &str) -> Option<PoolError> {
    if message == PoolError::Full.message() {
        return Some(PoolError::Full);
    }
    if message == RATE_LIMITED_MESSAGE {
        return Some(PoolError::RateLimited);
    }
    if message == PoolError::DeployerShare.message() {
        return Some(PoolError::DeployerShare);
    }
    if message == PoolError::ByteFull.message() {
        return Some(PoolError::ByteFull);
    }
    if message == PoolError::Duplicate.message() {
        return Some(PoolError::Duplicate);
    }
    if byte_full_shape(message) {
        return Some(PoolError::ByteFull);
    }
    if duplicate_shape(message, " is already in the pool")
        || duplicate_shape(message, " is already being admitted")
    {
        return Some(PoolError::Duplicate);
    }
    None
}

fn byte_full_shape(message: &str) -> bool {
    let Some(body) = message.strip_prefix("deploy pool is full: ") else {
        return false;
    };
    let Some(numbers) = body.strip_suffix(" term bytes in use") else {
        return false;
    };
    let mut parts = numbers.split(" of ");
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(in_use), Some(max_bytes), None)
            if digits(in_use) && digits(max_bytes)
    )
}

fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn duplicate_shape(message: &str, suffix: &str) -> bool {
    message
        .strip_prefix("deploy ")
        .and_then(|rest| rest.strip_suffix(suffix))
        .is_some_and(|id| {
            !id.is_empty() && !id.bytes().any(|byte| byte.is_ascii_whitespace())
        })
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

    #[test]
    fn every_static_refusal_classifies_to_itself() {
        for error in [
            PoolError::Full,
            PoolError::ByteFull,
            PoolError::DeployerShare,
            PoolError::RateLimited,
            PoolError::Duplicate,
        ] {
            assert_eq!(classify(error.message()), Some(error), "{error:?}");
        }
        assert_eq!(PoolError::RateLimited.message(), RATE_LIMITED_MESSAGE);
    }

    #[test]
    fn only_the_exact_byte_full_shape_is_a_bound() {
        assert_eq!(
            classify(&PoolError::byte_full_message(5, 10)),
            Some(PoolError::ByteFull)
        );
        assert_eq!(
            classify(&PoolError::byte_full_message(0, 67108864)),
            Some(PoolError::ByteFull)
        );
        // Prose that merely starts with the phrase is not the bound.
        assert_eq!(classify("deploy pool is full of surprises"), None);
        assert_eq!(classify("deploy pool is full: soon"), None);
        assert_eq!(classify("deploy pool is full: 5 or 10 term bytes in use"), None);
        assert_eq!(classify("deploy pool is full: 5 of 10 term bytes"), None);
        // And the phrase inside another sentence is no longer a bound (the old `contains` bug).
        assert_eq!(
            classify("the deploy pool is full: 5 of 10 term bytes in use"),
            None
        );
        assert_eq!(
            classify("x holds its share of the deploy pool"),
            None
        );
        assert_eq!(classify("a deploy rate limit exceeded"), None);
    }

    #[test]
    fn duplicate_ids_classify_as_duplicates_not_bounds() {
        assert_eq!(
            classify(&PoolError::duplicate_message("ab12")),
            Some(PoolError::Duplicate)
        );
        assert_eq!(
            classify(&PoolError::admitting_message("ab12")),
            Some(PoolError::Duplicate)
        );
        assert_eq!(classify("deploy  is already in the pool"), None);
        assert_eq!(classify("deploy ab12 is already in the pool for ever"), None);
        assert!(!PoolError::Duplicate.is_bounded());
    }

    #[test]
    fn the_bounded_set_is_exactly_the_four_submitter_and_pool_bounds() {
        for error in [
            PoolError::Full,
            PoolError::ByteFull,
            PoolError::DeployerShare,
            PoolError::RateLimited,
        ] {
            assert!(error.is_bounded(), "{error:?}");
        }
        assert!(!PoolError::Duplicate.is_bounded());
        assert_eq!(classify("deploy signature is invalid"), None);
    }
}
