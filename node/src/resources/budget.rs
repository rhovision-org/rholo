//! **The runtime permit budget** (Law 86d): headroom bounds the schedule, not admission.
//!
//! The boolean gate this replaces refused every deploy and explore request below the throttle floor.
//! The replacement hands out a small number of execution permits — at least one while any headroom
//! exists, zero below the policy floor — and refuses only when the permits are genuinely spent. A
//! deploy with a free pool slot is always accepted; the budget governs how much runs at once.
//!
//! The limit is computed from the watchdog's newest snapshot on each attempt, so it follows the same
//! five-second sample the rest of the node reads. There is no separate refresh task and no state to
//! drift: the count in flight is the only mutable part, and a lowered limit lets in-flight work
//! finish rather than revoking it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rchain_casper::budget::{permits_for, split, DEFAULT_MAX_EXPLORE, DEFAULT_PER_EXECUTION_MB};

use super::admission::{effective_available_mb, threshold_mb, AdmissionPolicy};
use super::state::ResourceGuard;

/// Which half of the split a budget spends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetKind {
    Execution,
    Explore,
}

/// A permit held for the duration of one execution or exploratory read. Dropping it returns the
/// place; a panic also returns it, because `Drop` runs on unwind.
pub struct Permit {
    budget: Arc<PermitBudget>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.budget.in_use.fetch_sub(1, Ordering::AcqRel);
    }
}

/// One half of the budget: the snapshot it reads, the kind it spends, and the in-flight count.
pub struct PermitBudget {
    resources: Arc<ResourceGuard>,
    policy: AdmissionPolicy,
    kind: BudgetKind,
    in_use: AtomicUsize,
    per_execution_mb: u64,
    max_explore: usize,
    cap: usize,
}

impl PermitBudget {
    pub fn new(
        resources: Arc<ResourceGuard>,
        policy: AdmissionPolicy,
        kind: BudgetKind,
    ) -> Arc<Self> {
        let cap = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(8);
        Arc::new(PermitBudget {
            resources,
            policy,
            kind,
            in_use: AtomicUsize::new(0),
            per_execution_mb: DEFAULT_PER_EXECUTION_MB,
            max_explore: DEFAULT_MAX_EXPLORE,
            cap,
        })
    }

    /// The permits this half may have in flight now. Recomputed from the latest snapshot, so a
    /// recovering node raises it without a restart.
    pub fn limit(&self) -> usize {
        let snapshot = self.resources.snapshot();
        let total = permits_for(
            effective_available_mb(&snapshot),
            threshold_mb(&snapshot, &self.policy),
            self.per_execution_mb,
            self.cap,
        );
        let split = split(total, self.max_explore);
        match self.kind {
            BudgetKind::Execution => split.exec,
            BudgetKind::Explore => split.explore,
        }
    }

    pub fn in_flight(&self) -> usize {
        self.in_use.load(Ordering::Acquire)
    }

    /// Take a place if one is free. Non-blocking: an exhausted budget refuses the caller with 429
    /// rather than queueing an unbounded number of handler futures.
    pub fn try_acquire(self: &Arc<Self>) -> Option<Permit> {
        let limit = self.limit();
        let mut current = self.in_use.load(Ordering::Acquire);
        loop {
            if current >= limit {
                return None;
            }
            match self.in_use.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(Permit {
                        budget: self.clone(),
                    })
                }
                Err(actual) => current = actual,
            }
        }
    }
}

/// The node's two budgets, over one snapshot.
pub struct CapacityBudget {
    pub execution: Arc<PermitBudget>,
    pub explore: Arc<PermitBudget>,
}

impl CapacityBudget {
    pub fn new(resources: Arc<ResourceGuard>) -> Self {
        let policy = AdmissionPolicy::default();
        CapacityBudget {
            execution: PermitBudget::new(resources.clone(), policy, BudgetKind::Execution),
            explore: PermitBudget::new(resources, policy, BudgetKind::Explore),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::fallback::ResourceState;
    use crate::resources::state::ResourceSnapshot;

    fn guard(available_mb: u64, total_mb: u64) -> Arc<ResourceGuard> {
        ResourceGuard::inert(ResourceSnapshot {
            state: "normal",
            fallback: ResourceState::Normal,
            mem_available_mb: available_mb,
            mem_total_mb: total_mb,
            sampled_at_ms: 1,
            source: "test".to_string(),
            ..ResourceSnapshot::default()
        })
    }

    #[test]
    fn a_host_with_headroom_gets_at_least_one_execution_and_explore_waits() {
        // 8 GiB total, 10% floor = 819; 900 MiB is 81 MiB over the floor -> one permit, all exec.
        let budget = CapacityBudget::new(guard(900, 8_192));
        assert_eq!(budget.execution.limit(), 1);
        assert_eq!(budget.explore.limit(), 0);
        let p = budget.execution.try_acquire().expect("one permit");
        assert!(budget.execution.try_acquire().is_none());
        drop(p);
        assert!(budget.execution.try_acquire().is_some());
    }

    #[test]
    fn explore_gets_a_share_when_the_host_has_room() {
        let budget = CapacityBudget::new(guard(8_000, 8_192));
        assert!(budget.execution.limit() >= 1);
        assert!(budget.explore.limit() >= 1);
    }

    #[test]
    fn below_the_floor_there_is_no_permit() {
        let budget = CapacityBudget::new(guard(100, 8_192));
        assert_eq!(budget.execution.limit(), 0);
        assert!(budget.execution.try_acquire().is_none());
    }

    #[test]
    fn the_in_flight_count_returns_on_drop() {
        let budget = CapacityBudget::new(guard(8_000, 8_192));
        let a = budget.explore.try_acquire().expect("a permit");
        assert_eq!(budget.explore.in_flight(), 1);
        drop(a);
        assert_eq!(budget.explore.in_flight(), 0);
    }
}
