//! Runtime resource policy: the headroom floor and the permit budget's inputs (Law 86).
//!
//! **This module used to be the ingress gate** (unit B34): it evaluated the watchdog's newest sample
//! per request and refused every deploy and explore with a 503 while the node was `throttled`. Law 86
//! removed that verdict. A pool with a free slot admits; memory pressure bounds the **execution
//! concurrency** (the permit budget), never the admission decision. A node that cannot execute now
//! holds the work and drains it later, and a node that cannot drain it relays it to a peer.
//!
//! What remains here is the policy the budget needs: the absolute floor, the headroom fraction of
//! the effective limit, the cgroup-aware available figure, and the staleness limit. Those facts are
//! still a *signal* for the operator and for `stops_production()`; they are not a client-facing
//! refusal.

/// The absolute floor: below 256 MiB of available memory there is no honest execution permit. This is
/// a policy floor, not a measurement; it is deliberately conservative and is named as such.
pub const MIN_AVAILABLE_MB: u64 = 256;

/// The fraction of the effective limit reserved as headroom, in percent. On an 8 GiB host this is
/// 819 MiB; on a 1 GiB host the absolute floor above dominates.
pub const HEADROOM_PERCENT: u64 = 10;

/// A snapshot older than this is not evidence of headroom. The watchdog samples every five seconds,
/// so this is six missed samples: long enough not to act on one scheduling delay, short enough that a
/// stopped watchdog cannot keep a stale reading alive.
pub const MAX_SAMPLE_AGE_MS: u64 = 30_000;

/// The resource policy. Defaults are [`MIN_AVAILABLE_MB`], [`HEADROOM_PERCENT`] and
/// [`MAX_SAMPLE_AGE_MS`]; the struct exists so tests can set a boundary exactly rather than infer it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionPolicy {
    pub min_available_mb: u64,
    pub headroom_percent: u64,
    pub max_sample_age_ms: u64,
}

impl Default for AdmissionPolicy {
    fn default() -> Self {
        AdmissionPolicy {
            min_available_mb: MIN_AVAILABLE_MB,
            headroom_percent: HEADROOM_PERCENT,
            max_sample_age_ms: MAX_SAMPLE_AGE_MS,
        }
    }
}

/// The memory the node may actually use: the host's `MemAvailable`, capped by the cgroup's remaining
/// allowance when both sides of the cgroup are known.
pub fn effective_available_mb(snapshot: &super::state::ResourceSnapshot) -> u64 {
    match (snapshot.cgroup_limit_mb, snapshot.cgroup_current_mb) {
        (Some(limit), Some(current)) => {
            snapshot.mem_available_mb.min(limit.saturating_sub(current))
        }
        _ => snapshot.mem_available_mb,
    }
}

/// The floor for this host: the larger of the absolute floor and the headroom fraction of the
/// effective limit (cgroup limit when set, else the host's total memory).
pub fn threshold_mb(snapshot: &super::state::ResourceSnapshot, policy: &AdmissionPolicy) -> u64 {
    let effective_limit = snapshot.cgroup_limit_mb.unwrap_or(snapshot.mem_total_mb);
    let percent = effective_limit.saturating_mul(policy.headroom_percent) / 100;
    policy.min_available_mb.max(percent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::fallback::ResourceState;
    use crate::resources::state::ResourceSnapshot;

    fn snapshot(available_mb: u64) -> ResourceSnapshot {
        ResourceSnapshot {
            state: "normal",
            fallback: ResourceState::Normal,
            mem_available_mb: available_mb,
            mem_total_mb: 8_192,
            sampled_at_ms: 1_000,
            source: "meminfo".to_string(),
            ..ResourceSnapshot::default()
        }
    }

    #[test]
    fn the_effective_floor_is_the_larger_of_the_absolute_and_percent_terms() {
        let policy = AdmissionPolicy::default();
        // 8 GiB total, no cgroup: the floor is 10% = 819 MiB.
        assert_eq!(threshold_mb(&snapshot(900), &policy), 819);

        let mut small = snapshot(200);
        small.mem_total_mb = 1_024;
        // 10% of 1 GiB is 102 MiB, so the absolute 256 MiB floor is the one that applies.
        assert_eq!(threshold_mb(&small, &policy), MIN_AVAILABLE_MB);
    }

    #[test]
    fn the_cgroup_remaining_allowance_caps_the_host_figure() {
        let policy = AdmissionPolicy::default();
        let mut capped = snapshot(8_000);
        capped.mem_total_mb = 16_384;
        capped.cgroup_limit_mb = Some(2_048);
        capped.cgroup_current_mb = Some(1_900);
        assert_eq!(effective_available_mb(&capped), 148);
        assert_eq!(threshold_mb(&capped, &policy), 256);
    }
}
