//! Free space on the data directory's filesystem.

/// Free and total bytes on the filesystem holding the data directory. `free_bytes` is what a non-root
/// process may write (`f_bavail`), not the filesystem's raw free count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskFacts {
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[cfg(unix)]
// `c_ulong`/`fsblkcnt_t` are `u64` on Linux but not on every Unix, so the conversion is not useless
// in general even though clippy sees it as such here.
#[allow(clippy::useless_conversion)]
pub fn data_dir_disk_facts(path: &std::path::Path) -> Result<DiskFacts, String> {
    let stats =
        nix::sys::statvfs::statvfs(path).map_err(|e| format!("statvfs {}: {e}", path.display()))?;
    let fragment = u64::try_from(stats.fragment_size())
        .map_err(|_| "statvfs fragment size does not fit u64".to_string())?;
    let available = u64::try_from(stats.blocks_available())
        .map_err(|_| "statvfs available block count does not fit u64".to_string())?;
    let total = u64::try_from(stats.blocks())
        .map_err(|_| "statvfs block count does not fit u64".to_string())?;
    Ok(DiskFacts {
        free_bytes: available.saturating_mul(fragment),
        total_bytes: total.saturating_mul(fragment),
    })
}

#[cfg(not(unix))]
pub fn data_dir_disk_facts(_path: &std::path::Path) -> Result<DiskFacts, String> {
    Err("disk facts are unsupported on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn statvfs_reports_a_temp_dir() {
        let dir = std::env::temp_dir();
        let facts = data_dir_disk_facts(&dir).expect("statvfs on the temp dir");
        assert!(facts.total_bytes > 0, "a mounted filesystem has a size");
        assert!(
            facts.free_bytes <= facts.total_bytes,
            "free is a subset of total"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_data_dir_is_an_error_not_zero_free() {
        let dir = std::env::temp_dir().join("rnode-disk-test-does-not-exist");
        assert!(data_dir_disk_facts(&dir).is_err());
    }
}
