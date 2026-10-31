//! **The execution permit budget** (Law 86d): capacity bounds the *schedule*, not the queue.
//!
//! The public testnet's `throttled` state was a boolean gate: no memory, no work. The replacement
//! computes how many executions the current headroom can pay for, with a floor of one while any
//! headroom exists, and hands that number to the scheduler. Admission never reads it — a deploy with
//! a free pool slot is accepted and stays pending — so a small host makes smaller batches, not a
//! closed node.
//!
//! The arithmetic is deliberately a policy with a stated provenance rather than a measured curve:
//! `per_mb` is the budget one execution is charged, defaulted to 64 MiB and configurable. The
//! scale is a first cut; the differential batch suite and the 4 GiB/two-validator arm are what tune
//! it, and the law only claims the shape (`headroom = 0 → 0`, else at least one permit).

/// The default charge for one execution, in MiB. A deploy's own memory is not measured; this is the
/// conservative policy figure the first release uses, named so an operator can change it.
pub const DEFAULT_PER_EXECUTION_MB: u64 = 64;

/// The most explore permits the split will hand out, so read-only work cannot starve block
/// production on a small host.
pub const DEFAULT_MAX_EXPLORE: usize = 2;

/// **How many executions the headroom pays for.** `critical_mb` is the policy floor
/// (`resources::admission::threshold_mb`), not zero: a node below it has no honest execution budget.
/// `cap` is the CPU/parallelism ceiling, so a large host pays for cores rather than memory.
pub fn permits_for(available_mb: u64, critical_mb: u64, per_mb: u64, cap: usize) -> usize {
    if cap == 0 {
        return 0;
    }
    let headroom = available_mb.saturating_sub(critical_mb);
    if headroom == 0 {
        return 0;
    }
    let per_mb = per_mb.max(1);
    let by_memory = usize::try_from(headroom / per_mb).unwrap_or(usize::MAX);
    by_memory.clamp(1, cap)
}

/// The split of the total budget between execution and exploration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermitSplit {
    pub exec: usize,
    pub explore: usize,
}

/// **Execution is the floor; exploration takes the surplus.** A total of one goes to execution, so a
/// node with one permit still makes chain progress; explore is refused (429) until headroom grows.
/// `max_explore` caps read-only work so it cannot starve production.
pub fn split(total: usize, max_explore: usize) -> PermitSplit {
    if total == 0 {
        return PermitSplit {
            exec: 0,
            explore: 0,
        };
    }
    let explore = (total / 4).min(max_explore);
    let exec = total.saturating_sub(explore).max(1);
    PermitSplit {
        // `exec >= 1` and `explore <= total/4`, so the two never exceed the total.
        exec,
        explore: total.saturating_sub(exec),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_headroom_is_no_permit_and_a_little_headroom_is_one() {
        assert_eq!(permits_for(409, 409, 64, 8), 0);
        assert_eq!(permits_for(400, 409, 64, 8), 0);
        assert_eq!(permits_for(410, 409, 64, 8), 1);
        assert_eq!(permits_for(547, 409, 64, 8), 2);
    }

    #[test]
    fn the_cpu_cap_bounds_a_large_host() {
        assert_eq!(permits_for(8_000, 819, 64, 8), 8);
        assert_eq!(permits_for(8_000, 819, 64, 4), 4);
        assert_eq!(permits_for(8_000, 819, 64, 0), 0);
    }

    #[test]
    fn execution_is_the_floor_and_explore_the_surplus() {
        assert_eq!(
            split(0, DEFAULT_MAX_EXPLORE),
            PermitSplit {
                exec: 0,
                explore: 0
            }
        );
        assert_eq!(
            split(1, DEFAULT_MAX_EXPLORE),
            PermitSplit {
                exec: 1,
                explore: 0
            }
        );
        assert_eq!(
            split(4, DEFAULT_MAX_EXPLORE),
            PermitSplit {
                exec: 3,
                explore: 1
            }
        );
        assert_eq!(
            split(8, DEFAULT_MAX_EXPLORE),
            PermitSplit {
                exec: 6,
                explore: 2
            }
        );
        for total in 0..64usize {
            let s = split(total, DEFAULT_MAX_EXPLORE);
            assert!(s.exec + s.explore <= total, "split({total}) overspent");
            if total > 0 {
                assert!(s.exec >= 1, "split({total}) starved execution");
            }
        }
    }
}
