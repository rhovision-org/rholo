//! Best-effort detection of other `rnode` processes in the same PID namespace.
//!
//! Advisory only: a container may hide its siblings, permission errors are skipped, and the count
//! includes this process. It is used to warn an operator who has co-located validators, never to refuse.

use std::path::Path;

pub fn count_rnode_processes(proc_root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .chars()
                .all(|c| c.is_ascii_digit())
        })
        .filter(|entry| {
            std::fs::read_to_string(entry.path().join("comm"))
                .map(|comm| comm.trim() == "rnode")
                .unwrap_or(false)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_processes_named_rnode() {
        let dir = std::env::temp_dir().join(format!("rnode-proc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("101")).unwrap();
        std::fs::create_dir_all(dir.join("202")).unwrap();
        std::fs::create_dir_all(dir.join("not-a-pid")).unwrap();
        std::fs::write(dir.join("101/comm"), "rnode\n").unwrap();
        std::fs::write(dir.join("202/comm"), "bash\n").unwrap();
        std::fs::write(dir.join("not-a-pid/comm"), "rnode\n").unwrap();
        assert_eq!(count_rnode_processes(&dir), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_count_is_exactly_the_numeric_rnode_entries() {
        let cases: &[(&[(&str, &str)], usize)] = &[
            (
                &[
                    ("101", "rnode\n"),
                    ("202", "bash\n"),
                    ("not-a-pid", "rnode\n"),
                ],
                1,
            ),
            (
                &[
                    ("101", "rnode\n"),
                    ("202", " rnode \n"),
                    ("303", "rnodex\n"),
                ],
                2,
            ),
            (&[("101", "Rnode\n"), ("202", "\n"), ("303", "")], 0),
            (&[("0001", "rnode\n")], 1),
            (&[], 0),
        ];
        for (index, (entries, expected)) in cases.iter().enumerate() {
            let dir =
                std::env::temp_dir().join(format!("rnode-proc-{}-{index}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            for (pid, comm) in *entries {
                std::fs::create_dir_all(dir.join(pid)).unwrap();
                if !comm.is_empty() {
                    std::fs::write(dir.join(pid).join("comm"), comm).unwrap();
                }
            }
            assert_eq!(count_rnode_processes(&dir), *expected, "case {index}");
            std::fs::remove_dir_all(&dir).ok();
        }
        let absent = std::env::temp_dir().join(format!("rnode-proc-absent-{}", std::process::id()));
        assert_eq!(count_rnode_processes(&absent), 0);
    }
}
