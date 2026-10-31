//! Start-up host facts and the enforcement seam (unit A2).
//!
//! The facts are read before the expensive DAG rebuild, with the persisted block count read cheaply
//! (`KeyValueTypedStore::count` uses the store's record count, not a `to_map`). The verdict is the
//! policy table's; this module only turns it into a log line and a refusal. Nothing here sets a cgroup
//! limit: that is the operator's boundary (`docs/src/node/resource-ringfence.md`).

use std::path::Path;

use rchain_shared::log::{Log, LogSource};

use crate::configuration::model::NodeConf;

use super::cgroup;
use super::colocation;
use super::disk;
use super::meminfo;
use super::policy::{
    evaluate_startup, ResourcePolicy, StartupEnforcement, StartupFacts, StartupVerdict,
};

/// One read of every host fact the preflight uses. An unreadable fact stays `None`; the verdict says so.
pub fn gather(data_dir: &Path) -> StartupFacts {
    StartupFacts {
        mem: meminfo::read_meminfo().ok(),
        cgroup: cgroup::read_cgroup_facts(),
        data_dir_free_bytes: disk::data_dir_disk_facts(data_dir)
            .ok()
            .map(|facts| facts.free_bytes),
        rnode_processes: colocation::count_rnode_processes(Path::new("/proc")),
        available_parallelism: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
    }
}

/// The configured policy. `min-available-mb = 0` selects the chain-length table; the config check has
/// already refused contradictory values, and this still propagates a conversion failure rather than
/// silently defaulting.
pub fn policy_from_conf(conf: &NodeConf) -> Result<ResourcePolicy, String> {
    policy_from_guard(&conf.resource_guard)
}

pub fn policy_from_guard(
    guard: &crate::configuration::model::ResourceGuardConf,
) -> Result<ResourcePolicy, String> {
    let enforcement = StartupEnforcement::parse(&guard.enforce_startup).ok_or_else(|| {
        format!(
            "resource-guard.enforce-startup must be auto, enforce or warn (got {:?})",
            guard.enforce_startup
        )
    })?;
    Ok(ResourcePolicy {
        enforcement,
        allow_under_minimum: guard.allow_under_minimum,
        accept_under_minimum_risk: guard.accept_under_minimum_risk,
        min_available_mb: if guard.min_available_mb > 0 {
            Some(
                u64::try_from(guard.min_available_mb)
                    .map_err(|_| "resource-guard.min-available-mb does not fit u64".to_string())?,
            )
        } else {
            None
        },
        min_data_dir_free_mb: u64::try_from(guard.min_data_dir_free_mb)
            .map_err(|_| "resource-guard.min-data-dir-free-mb must be positive".to_string())?,
    })
}

pub fn is_validator(conf: &NodeConf) -> bool {
    conf.casper.validator_private_key.is_some()
}

/// The operator risk notice. It is a log line, not a verdict: the notice must not turn a healthy start
/// into a warning, and a below-floor validator's refusal is the separate acceptance check.
pub const OPERATOR_RISK_NOTICE: &str = "validator operator risk notice: an invalid block or proven \
equivocation can cost a stated share of your bond; an absent validator is not currently slashed but \
forfeits income and can stall finality, and enough silence stops the >2/3 quorum. Read TERMS.md.";

/// Log the facts, then enforce the verdict: a refusal is returned (the caller's setup fails), a warning
/// is logged, and an unreadable fact is a warning rather than a silent pass.
pub fn enforce(
    conf: &NodeConf,
    facts: &StartupFacts,
    chain_blocks: u64,
    log: &dyn Log,
) -> Result<StartupVerdict, String> {
    let policy = policy_from_conf(conf)?;
    let verdict = evaluate_startup(facts, chain_blocks, &policy, is_validator(conf));
    let source = LogSource::new("resource-preflight");
    if is_validator(conf) {
        log.info(LogSource::new("operator-risk"), OPERATOR_RISK_NOTICE);
    }
    log.info(
        source,
        &format!(
            "host resources: {} MiB available, cgroup {}, data dir {} MiB free, {} rnode process(es), \
             {} CPU(s), {} persisted block(s)",
            facts.mem.map(|m| m.available_kb / 1024).unwrap_or(0),
            facts.cgroup.version.as_str(),
            facts.data_dir_free_bytes
                .map(|b| b / (1024 * 1024))
                .unwrap_or(0),
            facts.rnode_processes,
            facts.available_parallelism,
            chain_blocks
        ),
    );
    match &verdict {
        StartupVerdict::Refuse(reason) => {
            log.error(source, reason);
            Err(reason.clone())
        }
        StartupVerdict::Warn(warnings) => {
            for warning in warnings {
                log.warn(source, warning);
            }
            Ok(verdict.clone())
        }
        StartupVerdict::Ok => Ok(StartupVerdict::Ok),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::model::ResourceGuardConf;

    #[test]
    fn the_policy_follows_the_guard_block() {
        let mut guard = ResourceGuardConf {
            enforce_startup: "enforce".to_string(),
            allow_under_minimum: true,
            accept_under_minimum_risk: true,
            min_available_mb: 4096,
            min_data_dir_free_mb: 2048,
        };
        let policy = policy_from_guard(&guard).expect("a valid guard block");
        assert_eq!(policy.enforcement, StartupEnforcement::Enforce);
        assert!(policy.allow_under_minimum);
        assert_eq!(policy.min_available_mb, Some(4096));
        assert_eq!(policy.min_data_dir_free_mb, 2048);
        guard.enforce_startup = "sometimes".to_string();
        assert!(policy_from_guard(&guard).is_err());
    }

    #[test]
    fn an_unreadable_host_still_gathers_and_names_what_it_could() {
        let facts = gather(&std::env::temp_dir());
        assert!(facts.available_parallelism >= 1);
        #[cfg(unix)]
        assert!(facts.data_dir_free_bytes.is_some(), "statvfs on a temp dir");
    }
}
