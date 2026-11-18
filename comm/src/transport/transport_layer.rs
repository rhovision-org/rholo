//! The transport-layer abstraction.
//!
//! Mirrors `comm/src/main/scala/coop/rchain/comm/transport/TransportLayer.scala`.

use async_trait::async_trait;
use rchain_models::comm::protocol::Protocol;

use crate::errors::CommErr;
use crate::peer_node::PeerNode;
use crate::transport::chunker::Blob;

/// The transport interface (port of `TransportLayer[F]`): send a protocol message to a peer,
/// broadcast to many peers, or stream a blob to many peers.
#[async_trait]
pub trait TransportLayer: Send + Sync {
    async fn send(&self, peer: &PeerNode, msg: Protocol) -> CommErr<()>;

    async fn broadcast(&self, peers: &[PeerNode], msg: Protocol) -> Vec<CommErr<()>>;

    /// Stream a blob to each peer, returning **one outcome per input peer, in input order**.
    ///
    /// It used to return `()`, so a dropped reply was indistinguishable from a delivered one and
    /// the responder logged "sent" unconditionally. A caller that does not care now has to say so.
    async fn stream(&self, peers: &[PeerNode], blob: Blob) -> Vec<CommErr<()>>;
}
