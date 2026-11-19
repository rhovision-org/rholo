//! The netlayer abstraction: a bidirectional FIFO between two peers.
//!
//! `draft-specifications/Netlayers.md` and the implementation guide present two functions —
//! `new_outgoing_connection(ocapn_locator)` and `accept_incoming_connection()` — over a channel
//! that is "a bidirectional FIFO". CapTP stays "agnostic to latency, liveness, and privacy
//! characteristics of each underlying protocol": those are the netlayer's business, and the
//! protocol above sees only a queue of messages.
//!
//! A message on the channel is one CapTP operation, carried as one Syrup value. The netlayers
//! implemented here are [`crate::tcp_testing_only`] — the conformance suite's transport — the `unix`
//! transport (compiled on Unix only), a domain socket authenticated by the socket's file mode but
//! local to the host, and [`crate::noise`], which is the one a remote peer can use, with
//! [`crate::multi`] dispatching a dial by the locator's transport name.
//!
//! **Noise was once deliberately not built, and the reason it is now is worth keeping.** It is the
//! transport OCapN names for a real deployment, and a layer that only this repository speaks would
//! talk to itself — so it waited on **a reference that speaks it**. Agoric's endo repository now
//! carries one (`rust/ocapn_noise` and `packages/ocapn-noise`), which pins the parameters that would
//! otherwise be guesses and gives something to test against: `spec/audit/evidence/ocapn-noise/` drives
//! it, at a pinned commit, and the handshake completes. **That is the shape to keep for the next
//! transport** — the gate is a reachable reference, and a layer written before one exists is
//! un-verifiable whatever its arguments look like.
//!
//! What is still not built is `onion` (Tor), the only concrete transport in the OCapN draft: it needs
//! a `tor` daemon on the node, and the draft that fixes it says it is "likely to undergo significant
//! change". Any further netlayer (libp2p, IBC) implements the same two functions with nothing above
//! the seam moving.

use std::io;

use async_trait::async_trait;

use crate::locator::PeerLocator;

/// One open channel to a peer.
#[async_trait]
pub trait NetConn: Send {
    /// Write one CapTP message — a single Syrup value's bytes.
    async fn send(&mut self, message: &[u8]) -> io::Result<()>;

    /// **The peer's own address**, when the transport knows it (Law 62, AUDIT C225).
    ///
    /// The dial policy needs it to tell a *remote* peer from a local one: a peer on another host must
    /// not be able to make this node reach the node's own loopback services. A transport that cannot
    /// say is unchanged — the default is `None`, which the policy reads as "cannot be judged".
    fn peer_address(&self) -> Option<std::net::SocketAddr> {
        None
    }

    /// Read one CapTP message. `Ok(None)` is a clean end of stream *at a message boundary*; a
    /// stream that ends in the middle of a message is an error, never a silently dropped message.
    async fn recv(&mut self) -> io::Result<Option<Vec<u8>>>;

    /// **The peer's Ed25519 key, when the transport proved one** (HAZOP rows C242/C243).
    ///
    /// `None` is the honest answer for a transport that authenticates nobody — `tcp-testing-only`
    /// verifies no key, `unix` admits by the socket's file mode, and `websocket`'s handshake has only
    /// the *server* prove itself. A transport that did verify a key returns it here, so a session can
    /// be **named by the thing that was proved** rather than by the name the peer asserted for
    /// itself — which is public, and signed by nothing but the peer's own ephemeral session key.
    ///
    /// Read after the transport's handshake has run, which is what `accept_deferred`'s read of the
    /// peer's start-session guarantees.
    fn verified_peer(&self) -> Option<[u8; 32]> {
        None
    }
}

/// How a peer is dialled, and how it accepts dials — the two functions the netlayer standard
/// fixes.
#[async_trait]
pub trait Netlayer: Send + Sync {
    /// `new_outgoing_connection(ocapn_locator)` — dial the peer the locator names.
    async fn new_outgoing_connection(&self, locator: &PeerLocator) -> io::Result<Box<dyn NetConn>>;

    /// The same dial, told **which peer asked for it** (Law 62): `origin` is the address of the
    /// session the request arrived on, when the transport knows it. A dial a *remote* peer asked for
    /// is refused when the target is one of this node's own local addresses, so a peer cannot use this
    /// node to reach what the peer itself cannot.
    ///
    /// **Provided, and defaulting to the origin-free dial.** The netlayer standard fixes two
    /// functions, and a transport that cannot report an origin must stay a netlayer — so this is a
    /// refinement [`crate::dial_policy::PolicyNetlayer`] opts into, not a third function every
    /// implementation has to write.
    async fn new_outgoing_connection_from(
        &self,
        locator: &PeerLocator,
        _origin: Option<std::net::SocketAddr>,
    ) -> io::Result<Box<dyn NetConn>> {
        self.new_outgoing_connection(locator).await
    }

    /// `accept_incoming_connection()` — await the next peer that dials us.
    async fn accept_incoming_connection(&self) -> io::Result<Box<dyn NetConn>>;
}
