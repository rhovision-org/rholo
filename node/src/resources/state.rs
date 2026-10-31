//! The sampled resource snapshot and its watchdog.
//!
//! The snapshot is read by the status surface (A1) and will gate admission (units B/C). The watchdog is a
//! dedicated `std::thread`: a signal carried by the executor it protects is not a signal, and a stale
//! sample is reported as `unavailable`, never as `normal`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rchain_shared::lock::Unpoison;

use super::fallback::{Fallback, ResourceState};
use super::{cgroup, colocation, disk, meminfo};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceSnapshot {
    /// The **raw read**: `normal` when the host facts were read, `unavailable` when the memory facts
    /// could not be. The pressure state machine's word is [`Self::fallback`].
    pub state: &'static str,
    pub mem_available_mb: u64,
    pub mem_total_mb: u64,
    pub cgroup_version: &'static str,
    pub cgroup_limit_mb: Option<u64>,
    pub cgroup_current_mb: Option<u64>,
    pub cgroup_oom_kills: u64,
    /// **This process's resident set**, in MiB, from `/proc/self/status` `VmRSS`; 0 when it could not
    /// be read. The host's `MemAvailable` is the admission input; this is the node's own share, which
    /// is what an operator needs to tell the node from everything else on the box (issue #477).
    pub rss_mb: u64,
    /// `(quota, period)` from `cpu.max`/`cpu.cfs_quota_us`.
    pub cpu_quota: Option<(u64, u64)>,
    pub data_dir_free_mb: Option<u64>,
    pub rnode_processes: usize,
    /// Milliseconds since the Unix epoch at the sample.
    pub sampled_at_ms: u64,
    /// Which readers contributed, e.g. `meminfo+cgroup+disk`.
    pub source: String,
    /// **The fallback state** (unit C): `normal`, `throttled`, `critical`, or `unavailable` when the
    /// sample is stale or unreadable. The hysteresis lives in [`Fallback`]; the watchdog writes the
    /// resulting word and reason here beside the sample that produced them.
    pub fallback: ResourceState,
    /// Why the fallback state is what it is, in a stable phrase.
    pub fallback_reason: &'static str,
    /// When the fallback state last changed, in milliseconds since the Unix epoch.
    pub fallback_since_ms: u64,
}

impl Default for ResourceSnapshot {
    fn default() -> Self {
        ResourceSnapshot {
            state: "unavailable",
            mem_available_mb: 0,
            mem_total_mb: 0,
            cgroup_version: "unavailable",
            cgroup_limit_mb: None,
            cgroup_current_mb: None,
            cgroup_oom_kills: 0,
            rss_mb: 0,
            cpu_quota: None,
            data_dir_free_mb: None,
            rnode_processes: 0,
            sampled_at_ms: 0,
            source: "none".to_string(),
            fallback: ResourceState::Unavailable,
            fallback_reason: "no sample has been taken",
            fallback_since_ms: 0,
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// This process's resident set in MiB, from `/proc/self/status` `VmRSS`. `None` when the file is
/// unreadable (a non-Linux host, a hardened `/proc`), which is not an error: the host facts still
/// carry the admission signal, and the operator status says the figure is unavailable.
pub fn read_rss_mb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_rss_kb(&status).map(|kb| kb / 1024)
}

fn parse_rss_kb(status: &str) -> Option<u64> {
    status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse().ok())
    })
}

fn gather(data_dir: &Path) -> ResourceSnapshot {
    let mem = meminfo::read_meminfo().ok();
    let cg = cgroup::read_cgroup_facts();
    let disk = disk::data_dir_disk_facts(data_dir).ok();
    let mut source = Vec::new();
    if mem.is_some() {
        source.push("meminfo");
    }
    if cg.version != cgroup::CgroupVersion::Unavailable {
        source.push("cgroup");
    }
    if disk.is_some() {
        source.push("disk");
    }
    let rss = read_rss_mb();
    if rss.is_some() {
        source.push("proc-self");
    }
    ResourceSnapshot {
        state: if mem.is_some() {
            "normal"
        } else {
            "unavailable"
        },
        mem_available_mb: mem.map(|m| m.available_kb / 1024).unwrap_or(0),
        mem_total_mb: mem.map(|m| m.total_kb / 1024).unwrap_or(0),
        cgroup_version: cg.version.as_str(),
        cgroup_limit_mb: cg.memory_max.map(|b| b / (1024 * 1024)),
        cgroup_current_mb: cg.memory_current.map(|b| b / (1024 * 1024)),
        cgroup_oom_kills: cg.oom_kill.unwrap_or(0),
        rss_mb: rss.unwrap_or(0),
        cpu_quota: cg.cpu_quota,
        data_dir_free_mb: disk.map(|d| d.free_bytes / (1024 * 1024)),
        rnode_processes: colocation::count_rnode_processes(Path::new("/proc")),
        sampled_at_ms: now_ms(),
        source: if source.is_empty() {
            "none".to_string()
        } else {
            source.join("+")
        },
        // Overlaid by the watchdog's state machine; a bare `gather` has no trend to judge.
        fallback: ResourceState::Unavailable,
        fallback_reason: "the pressure state machine has not sampled yet",
        fallback_since_ms: 0,
    }
}

struct Inner {
    snapshot: RwLock<ResourceSnapshot>,
    /// The hysteresis tracker, owned by the watchdog thread. `Mutex` rather than `RwLock`: every access
    /// mutates the streak.
    fallback: Mutex<Fallback>,
    data_dir: PathBuf,
}

impl Inner {
    /// Run the state machine over one sample and publish it with the sample it was derived from.
    fn observe_and_store(&self, mut snapshot: ResourceSnapshot) {
        let now = now_ms();
        {
            let mut fb = self.fallback.lock().unpoison();
            fb.observe(&snapshot, now);
            snapshot.fallback = fb.state();
            snapshot.fallback_reason = fb.reason();
            snapshot.fallback_since_ms = fb.since_ms();
        }
        if let Ok(mut current) = self.snapshot.write() {
            *current = snapshot;
        }
    }
}

/// A snapshot that a dedicated thread refreshes; the thread stops once the guard is dropped.
pub struct ResourceGuard {
    inner: Arc<Inner>,
}

impl ResourceGuard {
    pub fn start(data_dir: PathBuf, interval: Duration) -> Result<Arc<Self>, String> {
        let inner = Arc::new(Inner {
            snapshot: RwLock::new(gather(&data_dir)),
            fallback: Mutex::new(Fallback::default()),
            data_dir,
        });
        // The first sample establishes the state; the loop keeps it current.
        inner.observe_and_store(gather(&inner.data_dir));
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("resource-watchdog".to_string())
            .spawn(move || loop {
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                inner.observe_and_store(gather(&inner.data_dir));
                drop(inner);
                std::thread::sleep(interval);
            })
            .map_err(|e| format!("spawn the resource watchdog: {e}"))?;
        Ok(Arc::new(ResourceGuard { inner }))
    }

    /// A fixed snapshot for tests and callers that have no host to sample.
    pub fn inert(snapshot: ResourceSnapshot) -> Arc<Self> {
        Arc::new(ResourceGuard {
            inner: Arc::new(Inner {
                snapshot: RwLock::new(snapshot),
                fallback: Mutex::new(Fallback::default()),
                data_dir: PathBuf::new(),
            }),
        })
    }

    pub fn snapshot(&self) -> ResourceSnapshot {
        self.inner
            .snapshot
            .read()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    pub fn sample_now(&self) -> ResourceSnapshot {
        gather(&self.inner.data_dir)
    }

    /// **The operator's resume** (unit C): a fresh sample is taken and a latched pressure state is
    /// cleared **only if that sample is actually clear**. It cannot override live pressure: a pressured
    /// node answers with its measured state. Returns the resulting state.
    pub fn resume(&self) -> ResourceState {
        let mut snapshot = gather(&self.inner.data_dir);
        let now = now_ms();
        let state = {
            let mut fb = self.inner.fallback.lock().unpoison();
            let state = fb.resume(&snapshot, now);
            snapshot.fallback = fb.state();
            snapshot.fallback_reason = fb.reason();
            snapshot.fallback_since_ms = fb.since_ms();
            state
        };
        if let Ok(mut current) = self.inner.snapshot.write() {
            *current = snapshot;
        }
        state
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_vmrss_from_proc_status() {
        let status = "Name:\trnode\nVmRSS:\t 12345 kB\n";
        assert_eq!(super::parse_rss_kb(status), Some(12_345));
        assert_eq!(super::parse_rss_kb("Name:\trnode\n"), None);
    }

    use super::*;

    #[test]
    fn a_snapshot_names_its_source_and_age() {
        let dir = std::env::temp_dir();
        let snapshot = gather(&dir);
        assert!(snapshot.sampled_at_ms > 0, "the sample carries a time");
        assert!(!snapshot.source.is_empty(), "the source is named");
        #[cfg(unix)]
        assert!(
            snapshot.source.contains("disk"),
            "the data dir is statvfs'd on unix"
        );
    }

    #[test]
    fn an_inert_guard_returns_its_snapshot() {
        let snapshot = ResourceSnapshot {
            state: "normal",
            fallback: ResourceState::Normal,
            mem_available_mb: 512,
            source: "test".to_string(),
            ..Default::default()
        };
        let guard = ResourceGuard::inert(snapshot.clone());
        assert_eq!(guard.snapshot(), snapshot);
    }
}
