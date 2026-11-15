//! **The audit tier's process-wide status cell.**
//!
//! `/api/v1/status` reports whether this node offers audit attestations, whether a replay is
//! running, and the last verdict it produced. The service is process-wide (one key, one scratch
//! pool), so the cell is too, in the same shape as the sync and backfill gauges: the writer is the
//! audit service, the reader is the status surface, and a test may drive its own process image.

use std::sync::{Mutex, OnceLock};

use rchain_models::casper::protocol::deploy_service::AuditAvailability;
use rchain_shared::lock::Unpoison;

fn cell() -> &'static Mutex<AuditAvailability> {
    static CELL: OnceLock<Mutex<AuditAvailability>> = OnceLock::new();
    CELL.get_or_init(|| {
        Mutex::new(AuditAvailability {
            available: false,
            running: false,
            last_verdict: None,
            last_checkpoint: None,
            last_run_at_unix_ms: None,
        })
    })
}

/// The current reading, cloned out of the cell.
pub fn audit_availability() -> AuditAvailability {
    cell().lock().unpoison().clone()
}

/// This node mounts the attested-audit service (it holds an audit key).
pub fn set_audit_available(available: bool) {
    cell().lock().unpoison().available = available;
}

/// A replay started or finished.
pub fn set_audit_running(running: bool) {
    cell().lock().unpoison().running = running;
}

/// Record a completed verdict. The timestamp is the caller's wall clock in Unix milliseconds; the
/// cell does not read a clock, so a test can drive it deterministically.
pub fn record_audit_verdict(verdict: &str, checkpoint: &str, at_unix_ms: u64) {
    let mut status = cell().lock().unpoison();
    status.running = false;
    status.last_verdict = Some(verdict.to_string());
    status.last_checkpoint = Some(checkpoint.to_string());
    status.last_run_at_unix_ms = Some(at_unix_ms);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cell is read back exactly as written, and a verdict clears `running`.
    #[test]
    fn the_audit_status_cell_round_trips() {
        set_audit_available(true);
        set_audit_running(true);
        record_audit_verdict("AGREE", "digest", 1_700_000_000_000);
        let status = audit_availability();
        assert!(status.available);
        assert!(!status.running, "a completed verdict is not still running");
        assert_eq!(status.last_verdict.as_deref(), Some("AGREE"));
        assert_eq!(status.last_checkpoint.as_deref(), Some("digest"));
        assert_eq!(status.last_run_at_unix_ms, Some(1_700_000_000_000));
    }
}
