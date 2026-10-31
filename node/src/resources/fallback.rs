//! The pressure fallback state (unit C, barrier B37).
//!
//! One sampled, hysteretic state drives admission, proposal and attestation. The states are
//! `Normal → Throttled → Critical`, plus `Unavailable` when the watchdog's sample is stale or the host
//! facts cannot be read. **`Halt` is deliberately not here**: the autopropose halt is an
//! operator/integrity decision (`HaltRecord`), never an automatic response to memory pressure — an
//! automatic halt on pressure would restart the node into the same OOM.
//!
//! The thresholds are **policy, not measurement**: `critical` is the admission floor B2 already refuses
//! at (`max(256 MiB, 10 % of the effective limit)`); `throttle` is one margin above it. A marginal node
//! sheds work before the kernel chooses an OOM victim, and the operator sees the state and its reason.
//!
//! Hysteresis: entering a worse state needs [`ENTER_SAMPLES`] consecutive samples (two, ten seconds at
//! the five-second watchdog interval); recovering needs [`RECOVER_SAMPLES`] above the boundary it is
//! leaving **plus** a margin, so a reading that hovers at the threshold cannot oscillate the state. An
//! OOM-kill event is a one-sample move to `Critical` — it is not a trend, it is a kill.

use super::admission::{effective_available_mb, threshold_mb, AdmissionPolicy};

/// Consecutive samples needed to enter a worse state.
pub const ENTER_SAMPLES: u32 = 2;
/// Consecutive samples needed to recover to a better state.
pub const RECOVER_SAMPLES: u32 = 3;
/// Absolute part of the throttle margin above the critical floor.
pub const THROTTLE_MARGIN_MB: u64 = 64;
/// Percent of the effective limit added to the throttle margin and to the recovery boundary.
pub const MARGIN_PERCENT: u64 = 5;
/// A sample older than this is not evidence of headroom; the watchdog samples every five seconds.
pub const MAX_SAMPLE_AGE_MS: u64 = 30_000;

/// The node's fallback state. Ordering is by severity for the transition rules, except that
/// `Unavailable` is orthogonal: it is reported when the sample cannot be trusted and does not overwrite
/// the latched pressure state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResourceState {
    #[default]
    Normal,
    Throttled,
    Critical,
    Unavailable,
}

impl ResourceState {
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceState::Normal => "normal",
            ResourceState::Throttled => "throttled",
            ResourceState::Critical => "critical",
            ResourceState::Unavailable => "unavailable",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "normal" => Some(ResourceState::Normal),
            "throttled" => Some(ResourceState::Throttled),
            "critical" => Some(ResourceState::Critical),
            "unavailable" => Some(ResourceState::Unavailable),
            _ => None,
        }
    }

    /// Stop proposing and attesting? Only the states where the node is not fit to produce.
    pub fn stops_production(self) -> bool {
        matches!(self, ResourceState::Critical | ResourceState::Unavailable)
    }

    fn severity(self) -> u8 {
        match self {
            ResourceState::Normal => 0,
            ResourceState::Throttled => 1,
            ResourceState::Critical => 2,
            ResourceState::Unavailable => 3,
        }
    }
}

fn margin_mb(sample: &super::state::ResourceSnapshot) -> u64 {
    let limit = sample.cgroup_limit_mb.unwrap_or(sample.mem_total_mb);
    THROTTLE_MARGIN_MB.max(limit.saturating_mul(MARGIN_PERCENT) / 100)
}

/// The floor at which ingress is refused and production stops: the same `max(256 MiB, 10 %)` B2 uses.
pub fn critical_floor_mb(sample: &super::state::ResourceSnapshot) -> u64 {
    threshold_mb(sample, &AdmissionPolicy::default())
}

/// The floor at which the node announces `Throttled`: one margin above the critical floor.
pub fn throttle_floor_mb(sample: &super::state::ResourceSnapshot) -> u64 {
    critical_floor_mb(sample).saturating_add(margin_mb(sample))
}

/// **The headroom at which the node may leave `Throttled`**: the throttle floor plus one margin — the
/// same exit rule `observe` applies after three good samples. Exposed so an operator can see how far
/// below recovery a node is, rather than inferring it from a word (issue #477).
pub fn recovery_floor_mb(sample: &super::state::ResourceSnapshot) -> u64 {
    throttle_floor_mb(sample).saturating_add(margin_mb(sample))
}

/// The hysteresis tracker. Owned by the watchdog thread and read through the snapshot.
#[derive(Clone, Debug)]
pub struct Fallback {
    latched: ResourceState,
    reported: ResourceState,
    reason: &'static str,
    since_ms: u64,
    streak: u32,
    last_oom_kills: u64,
}

impl Default for Fallback {
    fn default() -> Self {
        Fallback {
            latched: ResourceState::Normal,
            reported: ResourceState::Normal,
            reason: "headroom above the admission floor",
            since_ms: 0,
            streak: 0,
            last_oom_kills: 0,
        }
    }
}

impl Fallback {
    pub fn state(&self) -> ResourceState {
        self.reported
    }

    pub fn reason(&self) -> &'static str {
        self.reason
    }

    pub fn since_ms(&self) -> u64 {
        self.since_ms
    }

    fn set_latched(&mut self, state: ResourceState, reason: &'static str, now_ms: u64) {
        if self.latched != state {
            self.since_ms = now_ms;
        }
        self.latched = state;
        self.reason = reason;
    }

    /// Feed one sample. `now_ms` is the caller's clock so a test can age a sample without sleeping.
    pub fn observe(&mut self, sample: &super::state::ResourceSnapshot, now_ms: u64) {
        // An OOM kill is not a trend: it is the kernel already having chosen a victim. One event moves
        // the state to Critical without waiting for a second sample.
        if sample.cgroup_oom_kills > self.last_oom_kills {
            self.last_oom_kills = sample.cgroup_oom_kills;
            self.set_latched(
                ResourceState::Critical,
                "the cgroup OOM killer has fired",
                now_ms,
            );
            self.reported = ResourceState::Critical;
            self.streak = 0;
            return;
        }
        self.last_oom_kills = sample.cgroup_oom_kills;

        // A sample that cannot be trusted is reported immediately and breaks any transition streak: a
        // stopped watchdog must never read as normal, and a stale sample is not a run of good ones.
        if sample.state != "normal" {
            self.reported = ResourceState::Unavailable;
            self.reason = "resource facts are unreadable";
            self.streak = 0;
            return;
        }
        if now_ms.saturating_sub(sample.sampled_at_ms) > MAX_SAMPLE_AGE_MS {
            self.reported = ResourceState::Unavailable;
            self.reason = "resource sample is stale";
            self.streak = 0;
            return;
        }

        let available = effective_available_mb(sample);
        let critical = critical_floor_mb(sample);
        let throttle = throttle_floor_mb(sample);
        let target = if available < critical {
            ResourceState::Critical
        } else if available < throttle {
            ResourceState::Throttled
        } else {
            ResourceState::Normal
        };

        if target == self.latched {
            self.streak = 0;
            self.reported = self.latched;
            return;
        }

        // **Recovery needs the margin above the boundary it is leaving**, not merely being on the
        // better side of it: a reading hovering at the threshold would otherwise oscillate.
        if target.severity() < self.latched.severity() {
            let exit_floor = match self.latched {
                ResourceState::Critical => critical,
                ResourceState::Throttled => throttle,
                _ => 0,
            };
            if available < exit_floor.saturating_add(margin_mb(sample)) {
                self.streak = 0;
                self.reported = self.latched;
                return;
            }
        }

        let needed = if target.severity() > self.latched.severity() {
            ENTER_SAMPLES
        } else {
            RECOVER_SAMPLES
        };
        self.streak = self.streak.saturating_add(1);
        if self.streak >= needed {
            let reason = match target {
                ResourceState::Normal => "headroom recovered above the admission floor",
                ResourceState::Throttled => "headroom below the throttle floor",
                ResourceState::Critical => "headroom below the critical floor",
                ResourceState::Unavailable => "resource facts are unreadable",
            };
            self.set_latched(target, reason, now_ms);
            self.streak = 0;
        }
        self.reported = self.latched;
    }

    /// **The operator's resume**: clear the latch to `Normal` if a *fresh* sample is actually clear,
    /// otherwise report the measured state. It cannot override live pressure.
    pub fn resume(
        &mut self,
        sample: &super::state::ResourceSnapshot,
        now_ms: u64,
    ) -> ResourceState {
        let fresh = sample.state == "normal"
            && now_ms.saturating_sub(sample.sampled_at_ms) <= MAX_SAMPLE_AGE_MS;
        let clear = fresh
            && effective_available_mb(sample)
                >= throttle_floor_mb(sample).saturating_add(margin_mb(sample));
        if clear {
            self.set_latched(
                ResourceState::Normal,
                "operator resumed after headroom returned",
                now_ms,
            );
            self.reported = ResourceState::Normal;
            self.streak = 0;
            self.last_oom_kills = sample.cgroup_oom_kills;
            return ResourceState::Normal;
        }
        self.observe(sample, now_ms);
        self.reported
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::state::ResourceSnapshot;

    fn sample(available_mb: u64) -> ResourceSnapshot {
        ResourceSnapshot {
            state: "normal",
            mem_available_mb: available_mb,
            mem_total_mb: 8_192,
            sampled_at_ms: 1_000,
            source: "meminfo".to_string(),
            ..ResourceSnapshot::default()
        }
    }

    fn observe_n(fb: &mut Fallback, sample: &ResourceSnapshot, n: u32, at: u64) {
        for _ in 0..n {
            fb.observe(sample, at);
        }
    }

    #[test]
    fn normal_to_throttled_needs_hysteresis() {
        let mut fb = Fallback::default();
        // 900 MiB: below throttle (819 + 410 margin = 1,229) but above critical (819).
        let low = sample(900);
        fb.observe(&low, 1_000);
        assert_eq!(
            fb.state(),
            ResourceState::Normal,
            "one sample is not a trend"
        );
        fb.observe(&low, 1_001);
        assert_eq!(fb.state(), ResourceState::Throttled);
        assert!(!fb.state().stops_production());
    }

    #[test]
    fn recovery_needs_sustained_headroom() {
        let mut fb = Fallback::default();
        let low = sample(900);
        observe_n(&mut fb, &low, 2, 1_000);
        assert_eq!(fb.state(), ResourceState::Throttled);

        // The recovery boundary is throttle + margin: 2,000 MiB is above it.
        let high = sample(2_000);
        observe_n(&mut fb, &high, RECOVER_SAMPLES - 1, 2_000);
        assert_eq!(
            fb.state(),
            ResourceState::Throttled,
            "two good samples do not recover"
        );
        fb.observe(&high, 2_010);
        assert_eq!(fb.state(), ResourceState::Normal);
    }

    #[test]
    fn a_reading_at_the_boundary_does_not_oscillate() {
        let mut fb = Fallback::default();
        observe_n(&mut fb, &sample(900), 2, 1_000);
        assert_eq!(fb.state(), ResourceState::Throttled);
        // Just above the *entry* boundary is not enough to leave: the recovery margin applies.
        for at in 2_000..2_100 {
            fb.observe(&sample(1_300), at);
        }
        assert_eq!(fb.state(), ResourceState::Throttled);
    }

    #[test]
    fn critical_stops_production() {
        let mut fb = Fallback::default();
        // Below the critical floor (819 MiB on an 8 GiB host).
        observe_n(&mut fb, &sample(100), ENTER_SAMPLES, 1_000);
        assert_eq!(fb.state(), ResourceState::Critical);
        assert!(fb.state().stops_production());
    }

    #[test]
    fn an_oom_kill_transition_is_immediate() {
        let mut fb = Fallback::default();
        let mut killed = sample(8_000);
        killed.cgroup_oom_kills = 1;
        fb.observe(&killed, 1_000);
        assert_eq!(
            fb.state(),
            ResourceState::Critical,
            "a kill is a fact, not a trend"
        );
    }

    #[test]
    fn a_stale_watchdog_is_unavailable_not_normal() {
        let mut fb = Fallback::default();
        let mut stale = sample(8_000);
        stale.sampled_at_ms = 1_000;
        fb.observe(&stale, 1_000 + MAX_SAMPLE_AGE_MS + 1);
        assert_eq!(fb.state(), ResourceState::Unavailable);
        assert!(fb.state().stops_production());

        // A fresh sample returns to the latched state, which is still Normal.
        let mut fresh = sample(8_000);
        fresh.sampled_at_ms = 2_000_000;
        fb.observe(&fresh, 2_000_000);
        assert_eq!(fb.state(), ResourceState::Normal);
    }

    #[test]
    fn an_unreadable_sample_is_unavailable() {
        let mut fb = Fallback::default();
        let mut dark = sample(8_000);
        dark.state = "unavailable";
        fb.observe(&dark, 1_000);
        assert_eq!(fb.state(), ResourceState::Unavailable);
        assert_eq!(fb.reason(), "resource facts are unreadable");
    }

    #[test]
    fn resume_clears_only_when_a_fresh_sample_is_clear() {
        let mut fb = Fallback::default();
        observe_n(&mut fb, &sample(900), 2, 1_000);
        assert_eq!(fb.state(), ResourceState::Throttled);

        // Pressured: the resume reports the measured state and does not clear.
        assert_eq!(fb.resume(&sample(900), 2_000), ResourceState::Throttled);
        assert_eq!(fb.state(), ResourceState::Throttled);

        // Clear: the operator's resume clears the latch immediately.
        assert_eq!(fb.resume(&sample(4_000), 3_000), ResourceState::Normal);
        assert_eq!(fb.state(), ResourceState::Normal);
    }
}
