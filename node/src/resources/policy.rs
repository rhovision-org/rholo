//! The start-up policy: a provenance-carrying floor by chain length, and the verdict.
//!
//! The floor is a **policy table with a stated provenance**, not a measured replay curve multiplied by
//! height. `docs/src/node/validator-requirements.md` records that the per-block rate is not linear and
//! that the 3,200-block figure is unreproduced; turning either into a startup refusal would be a
//! fabricated precision. The table below is the conservative floor this unit ships, and every verdict
//! says which band and which provenance produced it.
//!
//! The enforcement default is **validators enforce, observers warn** (approved in the resource-safety
//! plan): a starved validator harms quorum, while an observer on a small host can still serve its user.
//! `--allow-under-minimum` / `resource-guard.allow-under-minimum` is the explicit, logged override.

use super::cgroup::{CgroupFacts, CgroupVersion};
use super::meminfo::MemInfo;

pub const MIB: u64 = 1024 * 1024;

/// The floor for a chain of `chain_blocks` persisted blocks, with where the number comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyFloor {
    pub mb: u64,
    pub provenance: &'static str,
    pub confidence: &'static str,
}

/// Bands chosen from the documented sizing (1 GiB minimal, 2–4 GiB for a ~1k-block start-up, 4 GiB+
/// above that). Above 5k blocks the floor stays at 4 GiB and says it is unmeasured: raising it without
/// a measurement would refuse hosts no one has shown to be too small.
pub fn minimum_for_chain(chain_blocks: u64) -> PolicyFloor {
    match chain_blocks {
        0..=100 => PolicyFloor {
            mb: 1024,
            provenance: "policy-table-v1",
            confidence: "documented-floor",
        },
        101..=1_000 => PolicyFloor {
            mb: 2048,
            provenance: "policy-table-v1",
            confidence: "documented-floor",
        },
        1_001..=5_000 => PolicyFloor {
            mb: 4096,
            provenance: "policy-table-v1",
            confidence: "documented-floor",
        },
        _ => PolicyFloor {
            mb: 4096,
            provenance: "policy-table-v1/unmeasured-above-5k",
            confidence: "unmeasured",
        },
    }
}

/// `auto` follows the node's role; `enforce`/`warn` let the operator choose explicitly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartupEnforcement {
    #[default]
    Auto,
    Enforce,
    Warn,
}

impl StartupEnforcement {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(StartupEnforcement::Auto),
            "enforce" => Some(StartupEnforcement::Enforce),
            "warn" => Some(StartupEnforcement::Warn),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StartupEnforcement::Auto => "auto",
            StartupEnforcement::Enforce => "enforce",
            StartupEnforcement::Warn => "warn",
        }
    }

    pub fn enforces(self, is_validator: bool) -> bool {
        match self {
            StartupEnforcement::Auto => is_validator,
            StartupEnforcement::Enforce => true,
            StartupEnforcement::Warn => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourcePolicy {
    pub enforcement: StartupEnforcement,
    pub allow_under_minimum: bool,
    /// Accept the below-floor validator risk; required with `allow_under_minimum`.
    pub accept_under_minimum_risk: bool,
    /// `None` uses [`minimum_for_chain`]; `Some(mb)` is an operator-declared floor.
    pub min_available_mb: Option<u64>,
    pub min_data_dir_free_mb: u64,
}

impl Default for ResourcePolicy {
    fn default() -> Self {
        ResourcePolicy {
            enforcement: StartupEnforcement::Auto,
            allow_under_minimum: false,
            accept_under_minimum_risk: false,
            min_available_mb: None,
            min_data_dir_free_mb: 1024,
        }
    }
}

/// The host facts a start-up verdict is made from. Every field is optional in the sense that an
/// unreadable fact is a warning, never a silent pass: the verdict says what it could not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupFacts {
    pub mem: Option<MemInfo>,
    pub cgroup: CgroupFacts,
    pub data_dir_free_bytes: Option<u64>,
    pub rnode_processes: usize,
    pub available_parallelism: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupVerdict {
    Ok,
    Warn(Vec<String>),
    Refuse(String),
}

impl StartupVerdict {
    pub fn refusal(&self) -> Option<&str> {
        match self {
            StartupVerdict::Refuse(reason) => Some(reason),
            _ => None,
        }
    }
}

/// The memory the node may actually use: the host's `MemAvailable`, capped by the cgroup's remaining
/// allowance when the cgroup reports both a limit and current usage.
pub fn effective_available_mb(facts: &StartupFacts) -> Option<u64> {
    let mem_mb = facts.mem.map(|m| m.available_kb / 1024)?;
    match (facts.cgroup.memory_max, facts.cgroup.memory_current) {
        (Some(max), Some(current)) => Some(mem_mb.min(max.saturating_sub(current) / MIB)),
        _ => Some(mem_mb),
    }
}

/// Decide the start-up verdict. `is_validator` selects the `auto` role; the override is explicit.
pub fn evaluate_startup(
    facts: &StartupFacts,
    chain_blocks: u64,
    policy: &ResourcePolicy,
    is_validator: bool,
) -> StartupVerdict {
    let mut warnings = Vec::new();

    // Data-directory free space: a readable fact, checked first because a full disk fails the next
    // write rather than the next allocation.
    if let Some(free) = facts.data_dir_free_bytes {
        let required = policy.min_data_dir_free_mb.saturating_mul(MIB);
        if free < required {
            if policy.allow_under_minimum {
                if is_validator && !policy.accept_under_minimum_risk {
                    return StartupVerdict::Refuse(format!(
                        "data directory has {} MiB free, below the {} MiB floor; --allow-under-minimum \
                         for a validator requires --accept-under-minimum-risk (read TERMS.md)",
                        free / MIB,
                        policy.min_data_dir_free_mb
                    ));
                }
                warnings.push(format!(
                    "data directory has {} MiB free, below the {} MiB floor; --allow-under-minimum \
                     overrides it{}",
                    free / MIB,
                    policy.min_data_dir_free_mb,
                    if is_validator {
                        "; operator risk accepted (TERMS.md)"
                    } else {
                        ""
                    }
                ));
            } else {
                return StartupVerdict::Refuse(format!(
                    "data directory has {} MiB free; at least {} MiB is required (allow-under-minimum \
                     is the explicit override)",
                    free / MIB,
                    policy.min_data_dir_free_mb
                ));
            }
        }
    } else {
        warnings.push(
            "data directory free space could not be read; the start-up preflight cannot judge it"
                .to_string(),
        );
    }

    // Memory: the policy floor, unless the operator declared one.
    let floor = match policy.min_available_mb {
        Some(mb) => PolicyFloor {
            mb,
            provenance: "config",
            confidence: "operator-declared",
        },
        None => minimum_for_chain(chain_blocks),
    };
    match effective_available_mb(facts) {
        Some(available) if available < floor.mb => {
            let detail = format!(
                "available memory is {} MiB (cgroup-aware), below the {} MiB floor for {} persisted \
                 block(s) ({} / {})",
                available, floor.mb, chain_blocks, floor.provenance, floor.confidence
            );
            if policy.allow_under_minimum {
                if is_validator && !policy.accept_under_minimum_risk {
                    return StartupVerdict::Refuse(format!(
                        "{detail}; --allow-under-minimum for a validator requires                          --accept-under-minimum-risk (read TERMS.md)"
                    ));
                }
                warnings.push(format!(
                    "{detail}; --allow-under-minimum overrides it{}",
                    if is_validator {
                        "; operator risk accepted (TERMS.md)"
                    } else {
                        ""
                    }
                ));
            } else if policy.enforcement.enforces(is_validator) {
                return StartupVerdict::Refuse(format!(
                    "{detail}; allow-under-minimum is the explicit override"
                ));
            } else {
                warnings.push(format!(
                    "{detail}; this node is not a validator, so the start is allowed with a warning"
                ));
            }
        }
        Some(_) => {}
        None => warnings.push(
            "host memory facts are unavailable; the start-up preflight cannot judge the floor"
                .to_string(),
        ),
    }

    // Advisories: the node cannot enforce these, but the operator can.
    if facts.cgroup.version == CgroupVersion::Unavailable {
        warnings.push(
            "no cgroup facts: the node is not demonstrably ringfenced; give it its own memory/CPU \
             cgroup (docs/src/node/resource-ringfence.md)"
                .to_string(),
        );
    } else if facts.cgroup.cpu_quota.is_none() {
        warnings.push(
            "no CPU quota is set for the node's cgroup; a busy host can starve it".to_string(),
        );
    }
    if facts.rnode_processes > 1 {
        warnings.push(format!(
            "{} rnode processes are visible in this namespace; co-located validators turn one host \
             failure into a shared stake loss",
            facts.rnode_processes
        ));
    }

    if warnings.is_empty() {
        StartupVerdict::Ok
    } else {
        StartupVerdict::Warn(warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(available_mb: u64) -> StartupFacts {
        StartupFacts {
            mem: Some(MemInfo {
                total_kb: 8 * 1024 * 1024,
                available_kb: available_mb * 1024,
            }),
            cgroup: CgroupFacts::default(),
            data_dir_free_bytes: Some(200 * 1024 * MIB),
            rnode_processes: 1,
            available_parallelism: 4,
        }
    }

    #[test]
    fn the_chain_length_table_is_monotone_and_names_its_provenance() {
        let floors: Vec<u64> = [0u64, 50, 101, 1_000, 1_001, 5_000, 5_001]
            .iter()
            .map(|n| minimum_for_chain(*n).mb)
            .collect();
        assert!(floors.windows(2).all(|w| w[0] <= w[1]), "{floors:?}");
        assert_eq!(minimum_for_chain(50).provenance, "policy-table-v1");
        assert_eq!(minimum_for_chain(5_001).confidence, "unmeasured");
    }

    #[test]
    fn a_validator_below_the_floor_is_refused_and_an_observer_is_warned() {
        let low = facts(512);
        let policy = ResourcePolicy::default();
        assert!(matches!(
            evaluate_startup(&low, 500, &policy, true),
            StartupVerdict::Refuse(_)
        ));
        assert!(matches!(
            evaluate_startup(&low, 500, &policy, false),
            StartupVerdict::Warn(_)
        ));
    }

    #[test]
    fn an_explicit_override_is_accepted_and_marked() {
        let policy = ResourcePolicy {
            allow_under_minimum: true,
            accept_under_minimum_risk: true,
            ..ResourcePolicy::default()
        };
        match evaluate_startup(&facts(512), 5_000, &policy, true) {
            StartupVerdict::Warn(warnings) => {
                assert!(
                    warnings.iter().any(|w| w.contains("overrides it")),
                    "{warnings:?}"
                );
            }
            other => panic!("expected a warned override, got {other:?}"),
        }
    }

    #[test]
    fn a_validator_override_needs_the_explicit_risk_acceptance() {
        let policy = ResourcePolicy {
            allow_under_minimum: true,
            ..ResourcePolicy::default()
        };
        match evaluate_startup(&facts(512), 5_000, &policy, true) {
            StartupVerdict::Refuse(reason) => {
                assert!(reason.contains("accept-under-minimum-risk"), "{reason}");
                assert!(reason.contains("TERMS.md"), "{reason}");
            }
            other => panic!("a validator override without acceptance must be refused: {other:?}"),
        }
        // An observer is not held to the validator floor and does not need the acceptance.
        assert!(matches!(
            evaluate_startup(&facts(512), 5_000, &policy, false),
            StartupVerdict::Warn(_)
        ));
    }

    #[test]
    fn enforcement_can_be_forced_either_way() {
        let mut policy = ResourcePolicy {
            enforcement: StartupEnforcement::Warn,
            ..ResourcePolicy::default()
        };
        assert!(matches!(
            evaluate_startup(&facts(512), 5_000, &policy, true),
            StartupVerdict::Warn(_)
        ));
        policy.enforcement = StartupEnforcement::Enforce;
        assert!(matches!(
            evaluate_startup(&facts(512), 5_000, &policy, false),
            StartupVerdict::Refuse(_)
        ));
    }

    #[test]
    fn a_full_data_directory_is_refused_unless_overridden() {
        let mut f = facts(8_000);
        f.data_dir_free_bytes = Some(10 * MIB);
        assert!(matches!(
            evaluate_startup(&f, 10, &ResourcePolicy::default(), false),
            StartupVerdict::Refuse(_)
        ));
        let policy = ResourcePolicy {
            allow_under_minimum: true,
            ..ResourcePolicy::default()
        };
        assert!(matches!(
            evaluate_startup(&f, 10, &policy, false),
            StartupVerdict::Warn(_)
        ));
    }

    #[test]
    fn the_cgroup_remaining_allowance_caps_the_host_figure() {
        let mut f = facts(4_000);
        f.cgroup.memory_max = Some(2048 * MIB);
        f.cgroup.memory_current = Some(1536 * MIB);
        assert_eq!(effective_available_mb(&f), Some(512));
    }

    #[test]
    fn an_unreadable_fact_is_a_warning_not_a_silent_pass() {
        let mut f = facts(8_000);
        f.mem = None;
        f.data_dir_free_bytes = None;
        match evaluate_startup(&f, 10, &ResourcePolicy::default(), false) {
            StartupVerdict::Warn(warnings) => {
                assert!(warnings
                    .iter()
                    .any(|w| w.contains("memory facts are unavailable")));
                assert!(warnings
                    .iter()
                    .any(|w| w.contains("free space could not be read")));
            }
            other => panic!("expected warnings, got {other:?}"),
        }
    }
}
