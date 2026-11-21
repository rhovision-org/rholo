//! **Where a joiner gets the checkpoint certificate** (sync/verification separation).
//!
//! The engine holds the sync protocol; it does not own an HTTP client. A production node wires an
//! implementation that fetches `GET /api/v1/checkpoint` from the bootstrap's client port; a test
//! wires a value. A source either answers the certificate it currently serves or says why it could
//! not — and, for a restart whose accepted fringe is behind the served head, it can answer for a
//! **named anchor** the caller chose from its own history.

use async_trait::async_trait;
use rchain_models::casper::protocol::checkpoint::CheckpointCertificate;

#[async_trait]
pub trait CheckpointSource: Send + Sync {
    /// The certificate this source's peer serves for its current finalized checkpoint.
    async fn fetch_certificate(&self) -> Result<CheckpointCertificate, String>;

    /// **The certificate this source holds for an anchor the caller names** (hex block hash).
    ///
    /// A validator that restarts after being down has an accepted fringe *behind* the head its
    /// peers now serve; the head certificate cannot anchor it. The caller names an anchor from its
    /// own accepted fringe, so the choice is its own history rather than the peer's. Encoding the
    /// hash is the caller's; a source that cannot answer by anchor says so and the caller falls
    /// back to `fetch_certificate`.
    async fn fetch_certificate_at_anchor(
        &self,
        _anchor: &str,
    ) -> Result<CheckpointCertificate, String> {
        Err("this source cannot serve a checkpoint certificate by anchor".to_string())
    }
}
