//! Runtime admission: refuse new deploy and explore work when the node has no headroom (unit B34).
//!
//! This is the runtime counterpart of the start-up preflight (A2). It reads the **same sampled
//! snapshot** the watchdog publishes — never the host directly — so the decision is as fresh as one
//! sample, and a **stale or unreadable sample is a refusal**, not a pass: a signal carried by the
//! executor it protects is not a signal, and "could not read" is not "healthy".
//!
//! The floor is a policy with a stated provenance, not a measured curve: an absolute floor, or a
//! fraction of the effective limit (the cgroup's, else the host's total), whichever is larger. A
//! marginal node chokes transactions **before** the kernel chooses an OOM victim; the operator sees a
//! `503` with `Retry-After` and the reason, not a dead process.
//!
//! What this does not do (unit C's job): it does not *stay* refused, does not withhold proposals or
//! attestations, and does not change the public `degraded` word. It is one gate on the ingress,
//! evaluated per request against the last sample.

use super::fallback::ResourceState;
use super::state::{ResourceGuard, ResourceSnapshot};

/// The absolute floor: below 256 MiB of available memory there is no honest deploy to admit. This is
/// a policy floor, not a measurement; it is deliberately conservative and is named as such.
pub const MIN_AVAILABLE_MB: u64 = 256;

/// The fraction of the effective limit reserved as headroom, in percent. On an 8 GiB host this is
/// 819 MiB; on a 1 GiB host the absolute floor above dominates.
pub const HEADROOM_PERCENT: u64 = 10;

/// A snapshot older than this is not evidence of headroom. The watchdog samples every five seconds,
/// so this is six missed samples: long enough not to refuse a healthy node on one scheduling delay,
/// short enough that a stopped watchdog cannot keep admission open on a stale reading.
pub const MAX_SAMPLE_AGE_MS: u64 = 30_000;

/// The admission thresholds. Defaults are [`MIN_AVAILABLE_MB`], [`HEADROOM_PERCENT`] and
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

/// Why a request was refused. The strings are stable so a test, a metric label or a log parser can
/// name the gate that fired; the detail carries the numbers for a human.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionVerdict {
    Admit,
    Refuse {
        reason: &'static str,
        detail: String,
    },
}

impl AdmissionVerdict {
    pub fn refusal(&self) -> Option<(&'static str, &str)> {
        match self {
            AdmissionVerdict::Admit => None,
            AdmissionVerdict::Refuse { reason, detail } => Some((reason, detail)),
        }
    }
}

/// The memory the node may actually use: the host's `MemAvailable`, capped by the cgroup's remaining
/// allowance when both sides of the cgroup are known.
pub fn effective_available_mb(snapshot: &ResourceSnapshot) -> u64 {
    match (snapshot.cgroup_limit_mb, snapshot.cgroup_current_mb) {
        (Some(limit), Some(current)) => {
            snapshot.mem_available_mb.min(limit.saturating_sub(current))
        }
        _ => snapshot.mem_available_mb,
    }
}

/// The floor for this host: the larger of the absolute floor and the headroom fraction of the
/// effective limit (cgroup limit when set, else the host's total memory).
pub fn threshold_mb(snapshot: &ResourceSnapshot, policy: &AdmissionPolicy) -> u64 {
    let effective_limit = snapshot.cgroup_limit_mb.unwrap_or(snapshot.mem_total_mb);
    let percent = effective_limit.saturating_mul(policy.headroom_percent) / 100;
    policy.min_available_mb.max(percent)
}

/// Decide one admission. `now_ms` is the caller's clock so a test can age a sample without sleeping.
pub fn evaluate(
    snapshot: &ResourceSnapshot,
    now_ms: u64,
    policy: &AdmissionPolicy,
) -> AdmissionVerdict {
    if snapshot.state != "normal" {
        return AdmissionVerdict::Refuse {
            reason: "resource sample unavailable",
            detail: format!(
                "the resource watchdog last reported state={} (source {}); admission refuses \
                 rather than treating an unreadable host as healthy",
                snapshot.state, snapshot.source
            ),
        };
    }

    // **The fallback state is the primary gate** (unit C): a node that has latched Throttled or
    // Critical refuses new work whatever a single sample says, and a stale or unreadable sample is
    // its own reason rather than being folded into "unavailable".
    match snapshot.fallback {
        ResourceState::Normal => {}
        ResourceState::Throttled => {
            return AdmissionVerdict::Refuse {
                reason: "resource state throttled",
                detail: format!(
                "{} ({} MiB available); new deploy and explore work is refused while the node is \
                     throttled",
                snapshot.fallback_reason,
                effective_available_mb(snapshot)
            ),
            }
        }
        ResourceState::Critical => {
            return AdmissionVerdict::Refuse {
                reason: "resource state critical",
                detail: format!(
                    "{} ({} MiB available); the node has stopped producing and refuses new work",
                    snapshot.fallback_reason,
                    effective_available_mb(snapshot)
                ),
            }
        }
        ResourceState::Unavailable => {
            return AdmissionVerdict::Refuse {
                reason: "resource state unavailable",
                detail: format!(
                    "{}; admission refuses rather than treating an untrusted sample as healthy",
                    snapshot.fallback_reason
                ),
            }
        }
    }

    let age_ms = now_ms.saturating_sub(snapshot.sampled_at_ms);
    if age_ms > policy.max_sample_age_ms {
        return AdmissionVerdict::Refuse {
            reason: "resource sample stale",
            detail: format!(
                "the newest resource sample is {age_ms} ms old (limit {} ms); a stopped watchdog \
                 must not keep admission open",
                policy.max_sample_age_ms
            ),
        };
    }
    let available = effective_available_mb(snapshot);
    let threshold = threshold_mb(snapshot, policy);
    if available < threshold {
        return AdmissionVerdict::Refuse {
            reason: "memory headroom below the admission floor",
            detail: format!(
                "available memory is {available} MiB (cgroup-aware), below the {threshold} MiB \
                 admission floor; new deploy and explore work is refused until headroom returns"
            ),
        };
    }
    AdmissionVerdict::Admit
}

/// The gate the ingress layers hold: the watchdog's guard and the policy, evaluated per request.
#[derive(Clone)]
pub struct PressureGate {
    resources: std::sync::Arc<ResourceGuard>,
    policy: AdmissionPolicy,
}

impl PressureGate {
    pub fn new(resources: std::sync::Arc<ResourceGuard>, policy: AdmissionPolicy) -> Self {
        PressureGate { resources, policy }
    }

    /// Evaluate against the newest sample. The watchdog writes the snapshot under a read lock, so
    /// this is one lock and one comparison — no I/O on the request path.
    pub fn evaluate_now(&self) -> AdmissionVerdict {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        evaluate(&self.resources.snapshot(), now_ms, &self.policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn headroom_admits_above_the_floor_and_refuses_below_it() {
        let policy = AdmissionPolicy::default();
        // 8 GiB total, no cgroup: the floor is 10% = 819 MiB.
        assert_eq!(
            evaluate(&snapshot(900), 1_000, &policy),
            AdmissionVerdict::Admit
        );
        let refusal = evaluate(&snapshot(700), 1_000, &policy);
        assert_eq!(
            refusal.refusal().map(|(reason, _)| reason),
            Some("memory headroom below the admission floor")
        );
    }

    #[test]
    fn the_absolute_floor_dominates_on_a_small_host() {
        let policy = AdmissionPolicy::default();
        let mut small = snapshot(200);
        small.mem_total_mb = 1_024;
        // 10% of 1 GiB is 102 MiB, so the absolute 256 MiB floor is the one that fires.
        assert_eq!(threshold_mb(&small, &policy), MIN_AVAILABLE_MB);
        assert!(evaluate(&small, 1_000, &policy).refusal().is_some());
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
        assert!(evaluate(&capped, 1_000, &policy).refusal().is_some());
    }

    #[test]
    fn a_stale_sample_is_a_refusal_not_a_pass() {
        let policy = AdmissionPolicy::default();
        let age = MAX_SAMPLE_AGE_MS + 1;
        let refusal = evaluate(&snapshot(8_000), 1_000 + age, &policy);
        assert_eq!(
            refusal.refusal().map(|(reason, _)| reason),
            Some("resource sample stale")
        );
    }

    #[test]
    fn an_unreadable_sample_is_a_refusal_not_a_pass() {
        let policy = AdmissionPolicy::default();
        let mut unreadable = snapshot(8_000);
        unreadable.state = "unavailable";
        assert_eq!(
            evaluate(&unreadable, 1_000, &policy)
                .refusal()
                .map(|(reason, _)| reason),
            Some("resource sample unavailable")
        );
    }
}
