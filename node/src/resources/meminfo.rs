//! `/proc/meminfo` — the two fields the node needs, parsed without assuming the rest.

/// `MemTotal` and `MemAvailable`, in KiB. `MemAvailable` is the kernel's own estimate of what a new
/// workload may allocate without swapping; it is the number the watchdog uses, not `MemFree`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemInfo {
    pub total_kb: u64,
    pub available_kb: u64,
}

fn parse_kb(rest: &str) -> Option<u64> {
    rest.split_whitespace().next()?.parse().ok()
}

/// Parse `MemTotal` and `MemAvailable`, refusing a file that lacks either. A missing field is not zero:
/// an unknown host is reported as unavailable, never as an empty one.
pub fn parse_meminfo(text: &str) -> Option<MemInfo> {
    let mut total_kb = None;
    let mut available_kb = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available_kb = parse_kb(rest);
        }
    }
    Some(MemInfo {
        total_kb: total_kb?,
        available_kb: available_kb?,
    })
}

pub fn read_meminfo() -> Result<MemInfo, String> {
    let text =
        std::fs::read_to_string("/proc/meminfo").map_err(|e| format!("read /proc/meminfo: {e}"))?;
    parse_meminfo(&text).ok_or_else(|| "/proc/meminfo lacks MemTotal or MemAvailable".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo_and_reports_available() {
        let info = parse_meminfo(
            "MemTotal:        2048000 kB\nMemFree: 1000 kB\nMemAvailable:  1024000 kB\n",
        )
        .expect("both fields");
        assert_eq!(info.total_kb, 2_048_000);
        assert_eq!(info.available_kb, 1_024_000);
    }

    #[test]
    fn a_missing_meminfo_is_unavailable_not_zero() {
        assert!(parse_meminfo("MemTotal: 2048000 kB\n").is_none());
        assert!(parse_meminfo("MemAvailable: 1024 kB\n").is_none());
        assert!(parse_meminfo("").is_none());
    }
}
