//! Cgroup v1/v2 facts for the cgroup the node is already inside.
//!
//! Detection is best-effort and never panics: an unreadable `/proc/self/cgroup` or a missing controller
//! file yields `Unavailable`/`None`, which the status reports as unknown — never as an unlimited host.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::MemInfo;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CgroupVersion {
    #[default]
    Unavailable,
    V1,
    V2,
}

impl CgroupVersion {
    pub fn as_str(self) -> &'static str {
        match self {
            CgroupVersion::V1 => "v1",
            CgroupVersion::V2 => "v2",
            CgroupVersion::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CgroupFacts {
    pub version: CgroupVersion,
    pub path: String,
    pub memory_max: Option<u64>,
    pub memory_current: Option<u64>,
    pub memory_high: Option<u64>,
    pub memory_peak: Option<u64>,
    pub oom_kill: Option<u64>,
    pub cpu_quota: Option<(u64, u64)>,
    pub io_weight: Option<u64>,
    pub pids_max: Option<u64>,
}

/// `/proc/self/cgroup`: a `0::/path` line is v2; a line whose controller list contains `memory` is v1.
/// A hybrid host that shows both is read as v2 (the unified hierarchy is the one with limits).
pub fn parse_self_cgroup(text: &str) -> Option<(CgroupVersion, String)> {
    let mut v2 = None;
    let mut v1 = None;
    for line in text.lines() {
        let mut parts = line.splitn(3, ':');
        let _id = parts.next()?;
        let controllers = parts.next()?;
        let path = parts.next()?;
        if controllers.is_empty() {
            v2 = Some(path.to_string());
        } else if v1.is_none() && controllers.split(',').any(|c| c == "memory") {
            v1 = Some(path.to_string());
        }
    }
    if let Some(path) = v2 {
        Some((CgroupVersion::V2, path))
    } else {
        v1.map(|path| (CgroupVersion::V1, path))
    }
}

/// `"max"` and the v1 unlimited sentinel (a value at or above 2^60) are **no limit**, not a huge one.
pub fn parse_memory_max(text: &str) -> Option<u64> {
    let value: u64 = text.trim().parse().ok()?;
    (value < (1u64 << 60)).then_some(value)
}

pub fn parse_memory_events(text: &str) -> BTreeMap<String, u64> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let key = parts.next()?.to_string();
            let value = parts.next()?.parse().ok()?;
            Some((key, value))
        })
        .collect()
}

/// `"max 100000"` is no quota; `"200000 100000"` is 200 % of one CPU.
pub fn parse_cpu_max(text: &str) -> Option<(u64, u64)> {
    let mut parts = text.split_whitespace();
    let quota = parts.next()?;
    let period = parts.next()?;
    Some((quota.parse().ok()?, period.parse().ok()?))
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

fn cgroup_dir(root: &str, path: &str) -> PathBuf {
    Path::new(root).join(path.trim_start_matches('/'))
}

fn read_v2(path: &str) -> CgroupFacts {
    let dir = cgroup_dir("/sys/fs/cgroup", path);
    CgroupFacts {
        version: CgroupVersion::V2,
        path: path.to_string(),
        memory_max: read_trimmed(&dir.join("memory.max")).and_then(|t| parse_memory_max(&t)),
        memory_current: read_u64(&dir.join("memory.current")),
        memory_high: read_trimmed(&dir.join("memory.high")).and_then(|t| parse_memory_max(&t)),
        memory_peak: read_u64(&dir.join("memory.peak")),
        oom_kill: read_trimmed(&dir.join("memory.events"))
            .map(|t| parse_memory_events(&t))
            .and_then(|m| m.get("oom_kill").copied()),
        cpu_quota: read_trimmed(&dir.join("cpu.max")).and_then(|t| parse_cpu_max(&t)),
        io_weight: read_u64(&dir.join("io.weight")),
        pids_max: read_trimmed(&dir.join("pids.max")).and_then(|t| parse_memory_max(&t)),
    }
}

fn read_v1(path: &str) -> CgroupFacts {
    let memory = cgroup_dir("/sys/fs/cgroup/memory", path);
    let cpu = cgroup_dir("/sys/fs/cgroup/cpu,cpuacct", path);
    let blkio = cgroup_dir("/sys/fs/cgroup/blkio", path);
    let pids = cgroup_dir("/sys/fs/cgroup/pids", path);
    let quota = read_u64(&cpu.join("cpu.cfs_quota_us"));
    let period = read_u64(&cpu.join("cpu.cfs_period_us"));
    CgroupFacts {
        version: CgroupVersion::V1,
        path: path.to_string(),
        memory_max: read_trimmed(&memory.join("memory.limit_in_bytes"))
            .and_then(|t| parse_memory_max(&t)),
        memory_current: read_u64(&memory.join("memory.usage_in_bytes")),
        memory_high: None,
        memory_peak: read_u64(&memory.join("memory.max_usage_in_bytes")),
        oom_kill: read_u64(&memory.join("memory.failcnt")),
        cpu_quota: quota.zip(period),
        io_weight: read_u64(&blkio.join("blkio.weight")),
        pids_max: read_u64(&pids.join("pids.max")),
    }
}

pub fn read_cgroup_facts() -> CgroupFacts {
    let Ok(text) = std::fs::read_to_string("/proc/self/cgroup") else {
        return CgroupFacts::default();
    };
    match parse_self_cgroup(&text) {
        Some((CgroupVersion::V2, path)) => read_v2(&path),
        Some((CgroupVersion::V1, path)) => read_v1(&path),
        _ => CgroupFacts::default(),
    }
}

/// The headroom the node may actually use: the smaller of the host's `MemAvailable` and the cgroup's
/// remaining allowance. `None` only when neither is known.
pub fn effective_available_bytes(mem: &MemInfo, cg: &CgroupFacts) -> Option<u64> {
    let mem_bytes = mem.available_kb.saturating_mul(1024);
    match (cg.memory_max, cg.memory_current) {
        (Some(max), Some(current)) => Some(mem_bytes.min(max.saturating_sub(current))),
        _ => Some(mem_bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v2_memory_cpu_and_events() {
        let (version, path) =
            parse_self_cgroup("0::/user.slice/user-1000.slice/session-1.scope\n").expect("v2 line");
        assert_eq!(version, CgroupVersion::V2);
        assert_eq!(path, "/user.slice/user-1000.slice/session-1.scope");
        assert_eq!(
            parse_memory_events("low 0\nhigh 2\nmax 1\noom 1\noom_kill 3\n").get("oom_kill"),
            Some(&3)
        );
        assert_eq!(parse_cpu_max("200000 100000"), Some((200_000, 100_000)));
        assert_eq!(parse_cpu_max("max 100000"), None);
    }

    #[test]
    fn parses_v1_memory_and_cpu_limits() {
        let (version, path) =
            parse_self_cgroup("4:memory:/docker/abc\n3:cpu,cpuacct:/docker/abc\n")
                .expect("v1 memory line");
        assert_eq!(version, CgroupVersion::V1);
        assert_eq!(path, "/docker/abc");
        assert_eq!(parse_memory_max("8589934592"), Some(8_589_934_592));
    }

    #[test]
    fn an_unlimited_limit_is_none_not_a_huge_number() {
        assert_eq!(parse_memory_max("max"), None);
        assert_eq!(parse_memory_max("9223372036854771712"), None);
    }

    #[test]
    fn effective_available_is_the_smaller_of_host_and_cgroup() {
        let mem = MemInfo {
            total_kb: 4 * 1024 * 1024,
            available_kb: 1024 * 1024,
        };
        let cg = CgroupFacts {
            memory_max: Some(2 * 1024 * 1024 * 1024),
            memory_current: Some(1536 * 1024 * 1024),
            ..Default::default()
        };
        assert_eq!(
            effective_available_bytes(&mem, &cg),
            Some(512 * 1024 * 1024)
        );
    }
}
