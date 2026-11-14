//! **Where a joiner gets the checkpoint certificate** (sync/verification separation).
//!
//! The engine holds the sync protocol; it does not own an HTTP client. A production node wires an
//! implementation that fetches `GET /api/v1/checkpoint` from the bootstrap's client port; a test
//! wires a value. The trait is deliberately one method: a source either answers the certificate it
//! currently serves or says why it could not.

use async_trait::async_trait;
use rchain_models::casper::protocol::checkpoint::CheckpointCertificate;

#[async_trait]
pub trait CheckpointSource: Send + Sync {
    /// The certificate this source's peer serves for its current finalized checkpoint.
    async fn fetch_certificate(&self) -> Result<CheckpointCertificate, String>;
}
