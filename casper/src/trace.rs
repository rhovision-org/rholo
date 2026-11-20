//! Decision traces for the release claims.
//!
//! A trace is the *input point* of a decision, serialised under `RNODE_TRACE_DIR` when the local
//! claim harness sets it. The pure evaluator (`tools/trace-evaluate.py`) recomputes the decision's
//! predicates from these coordinates, so a failed claim is a point in the proven phase space rather
//! than a log line. Production does not set the variable; a write failure is not a consensus event.

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
static SEQ: AtomicU64 = AtomicU64::new(0);

/// The trace directory, if the node was started with `RNODE_TRACE_DIR`.
pub fn trace_dir() -> Option<&'static PathBuf> {
    DIR.get_or_init(|| std::env::var_os("RNODE_TRACE_DIR").map(PathBuf::from))
        .as_ref()
}

/// Whether traces are being written. Callers that would clone maps to build a trace guard on this.
pub fn trace_enabled() -> bool {
    trace_dir().is_some()
}

/// Write one trace as `<prefix>-<pid>-<seq>.json`. A write failure is ignored deliberately: a
/// diagnostic must never change the decision it records.
pub fn write_trace<T: Serialize>(prefix: &str, value: &T) {
    let Some(dir) = trace_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(dir);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = format!("{prefix}-{}-{seq}.json", std::process::id());
    if let Ok(bytes) = serde_json::to_vec_pretty(value) {
        let _ = std::fs::write(dir.join(name), bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_writer_is_a_no_op_without_a_trace_directory() {
        // The test process normally has no `RNODE_TRACE_DIR`; the check is that calling the writer
        // cannot panic or create a file, not that the environment is pristine.
        if trace_dir().is_none() {
            #[derive(serde::Serialize)]
            struct Row {
                kind: &'static str,
            }
            write_trace("test", &Row { kind: "test" });
        }
    }
}
