#![forbid(unsafe_code)]
//! OCapN for RNode — the object-capability network (`https://ocapn.org/`), CapTP over a pluggable
//! netlayer.
//!
//! This is the transport that lets an Agoric vat hold a live reference to an RChain object and
//! invoke it, and vice versa. It is Layer 2 of the cross-shard design record
//! ([`docs/src/node/shard-invoke.md`](../../docs/src/node/shard-invoke.md)): the delegation layer
//! built over Layer 1's primitive, *a cross-shard call is a caller-signed deploy*.
//!
//! **The implementation guide is built, not a stage of it** (the design record is
//! `docs/src/node/ocapn.md`): the Syrup codec and the locators (`syrup`, `locator`, `peer`); the
//! netlayer trait with the `tcp-testing-only` and `unix` transports (the latter compiled on Unix
//! only, issue #615; [`multi`] dispatches a dial by the locator's transport name);
//! `op:start-session`; the import/export and answer tables with `op:deliver`, `op:listen`, pipeline
//! answers and GC; third-party handoffs and the sturdyref enlivener; and the bridge — a delivery
//! to a chain-backed export becomes a signed deploy.
//!
//! Nothing here is on-chain by itself: sessions, wire bytes, and answer bookkeeping are node-local
//! state. What reaches consensus is only the signed deploy the bridge produces.

pub mod bootstrap;
pub mod capacity;
pub mod captp;
pub mod conn;
pub mod dial_policy;
pub mod enliven;
pub mod fixtures;
pub mod framed;
pub mod handoff;
pub mod locator;
pub mod multi;
pub mod netlayer;
pub mod netstring;
pub mod noise;
pub mod owner;
pub mod par_value;
pub mod peer;
pub mod proxy;
pub mod session;
pub mod session_id;
pub mod syrup;
pub mod tcp_testing_only;
// The Unix-domain-socket transport: `std::os::unix` and `tokio::net::Unix*` exist only on Unix, so
// the module is compiled only there (issue #615). Nothing else in the crate names it — `multi`
// dispatches a dial by the locator's transport-name string — so a non-Unix build loses the transport
// and nothing else.
#[cfg(unix)]
pub mod unix;
pub mod websocket;
