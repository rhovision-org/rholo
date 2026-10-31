//! The sturdyref enlivener: the object that turns a sturdyref back into a live reference.
//!
//! The conformance suite fetches it by swiss number (`fixtures::STURDYREF_ENLIVENER`), delivers it a
//! sturdyref, and expects the implementation to **dial the peer that sturdyref names and fetch the
//! object there** — which is what makes a sturdyref a capability rather than a name, and what breaks
//! the "a `fetch` of a peer I am not connected to does nothing" shape a vat would otherwise have.
//!
//! It is also the *dial-out* case the node did not have: `Session::dial` existed and was called from
//! no production code, and nothing owned a second session's loop. This object does, through
//! [`SessionHandle`]/[`SessionLoop`], and it registers every session it opens in the
//! [`SessionRegistry`] so the crossed-hello rule can see it.
//!
//! **What it answers with.** The object it fetched lives on the session it just made, so the answer
//! is a [`Forward`] — an object that routes deliveries back to that session. A peer on the original
//! session therefore holds a live reference to something it never had a session with, which is the
//! enlivening the suite is testing.

use rchain_shared::lock::Unpoison;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::bootstrap::Bootstrap;
use crate::captp::Desc;
use crate::conn::{Act, ConnectionError, Export, Session};
use crate::handoff::{Envelope, HandoffGive};
use crate::locator::{PeerLocator, Sturdyref};
use crate::netlayer::Netlayer;
use crate::owner::{HandOff, SessionHandle, SessionRegistry};
use crate::proxy::{catcher, Forward};
use crate::syrup::Value;

/// How long the enliven's own dial-and-fetch may take before it answers with a break.
///
/// Bounded because the suite's crossed-hello tests hand it a sturdyref to a peer that never answers
/// the fetch — they are testing the *dial*, not the object — so an unbounded wait would sit in that
/// session's loop for the life of the process.
pub const ENLIVEN_TIMEOUT: Duration = Duration::from_secs(30);

/// The enlivener object. One per session (it holds no state between deliveries beyond what each
/// delivery's own session does).
pub struct Enlivener {
    /// The netlayer to dial with — the same one this peer listens on, so the peer it dials can dial
    /// back to our advertised location.
    netlayer: Arc<dyn Netlayer>,
    /// Our own advertised location, which goes in the start-session of every session we open.
    location: PeerLocator,
    /// The live-session registry, so the crossed-hello rule sees the sessions this opens.
    registry: Arc<SessionRegistry>,
    /// The session this object is serving. A Gifter has to name that session's peer in the give's
    /// `receiver-key` and sign the give with its secret, and neither is knowable otherwise.
    session: crate::owner::SessionSlot,
    /// **The ceiling a dial draws from** (HAZOP row C241), or `None` for a caller with no budget to
    /// share — the conformance fixtures, whose dials happen inside a session that already holds an
    /// accept permit. A held permit travels with the dialed session's loop task, so the slot is
    /// released when the session ends and not before.
    dials: Option<Arc<tokio::sync::Semaphore>>,
    timeout: Duration,
}

impl Enlivener {
    pub fn new(
        netlayer: Arc<dyn Netlayer>,
        location: PeerLocator,
        registry: Arc<SessionRegistry>,
        session: crate::owner::SessionSlot,
    ) -> Enlivener {
        Enlivener::with_dial_budget(netlayer, location, registry, session, None)
    }

    /// [`Enlivener::new`] with the dial ceiling spelled out — see the `dials` field.
    pub fn with_dial_budget(
        netlayer: Arc<dyn Netlayer>,
        location: PeerLocator,
        registry: Arc<SessionRegistry>,
        session: crate::owner::SessionSlot,
        dials: Option<Arc<tokio::sync::Semaphore>>,
    ) -> Enlivener {
        Enlivener {
            netlayer,
            location,
            registry,
            session,
            dials,
            timeout: ENLIVEN_TIMEOUT,
        }
    }

    /// The Gifter's half: enliven, deposit the object with the exporter, and answer the requester
    /// with a signed **give** naming the receiver (the peer that asked).
    ///
    /// The deposit is the object's own descriptor, echoed back to the peer we fetched it from — its
    /// import names its own export, so sending the same record back tells it "the object you export
    /// at N" — and the give is signed with the secret of the session the request arrived on, because
    /// that session's key is what the *receiver* will use to sign its claim.
    async fn gift(
        &self,
        sturdyref: &Sturdyref,
        serving: &crate::owner::SessionContext,
    ) -> Result<Act, String> {
        let (handle, _to, fetched) = self
            .dial_and_fetch(&sturdyref.peer, &sturdyref.swiss_num)
            .await?;
        let session_id = handle
            .id
            .clone()
            .ok_or_else(|| "the exporter session has no id yet".to_string())?;
        let gift_id = sturdyref.swiss_num.clone();

        handle
            .deliver(HandOff {
                to: Desc::Export(0u64.into()),
                args: vec![
                    Value::Symbol("deposit-gift".to_string()),
                    Value::Bytes(gift_id.clone()),
                    fetched,
                ],
                resolve_me: None,
            })
            .await
            .map_err(|e| e.to_string())?;

        let give = HandoffGive {
            receiver_key: crate::handoff::key_record(
                serving
                    .handle
                    .peer_key
                    .as_deref()
                    .ok_or_else(|| "the requesting peer has not completed its handshake")?,
            ),
            exporter_location: sturdyref.peer.clone(),
            session: session_id.to_vec(),
            gifter_side: handle.own_pi.to_vec(),
            gift_id,
        };
        let envelope = Envelope::sign(give.to_syrup(), &serving.secret)?;
        Ok(Act::value(envelope.to_syrup()))
    }

    /// Reach the object a peer's locator and swiss number name: **reuse the session we already have
    /// with that peer, or dial one**. Returns the session that owns the object, and how to address it
    /// there.
    ///
    /// Reuse is not an optimization: the suite's handoff fixture hands the enlivener a sturdyref to a
    /// peer whose session *it* dialed, and expects the fetch on that session — dialing a second one
    /// would put the fetch on a connection nobody is reading. And a dial is *deferred* rather than
    /// completed, because the crossed-hello fixture reads our start-session and never sends one.
    ///
    /// **This is also the node's own dial-out** (issue #249): a dial the *node* starts and one a peer
    /// asked for differ in **who asked**, not in what a dial is, so there is one implementation rather
    /// than two that could drift. The origin a dial carries
    /// ([`crate::owner::session_origin`]) is `None` for a node-started dial — the slot it is built over
    /// is empty — which is what makes Law 62's origin rule inapplicable to it, and correct: there is no
    /// peer origin to judge. The guard on such a dial is the target policy alone, which the netlayer
    /// applies.
    pub async fn dial_and_fetch(
        &self,
        peer: &PeerLocator,
        swiss: &[u8],
    ) -> Result<(SessionHandle, Desc, Value), String> {
        let handle = match self.registry.live(peer) {
            Some(existing) => existing,
            None => {
                // **A dialed session draws from the ceiling too** (HAZOP row C231's neighbour, C241).
                // Taken before the connect — a dial that cannot be afforded must not open a socket —
                // and held by the loop task below, so the slot lasts exactly as long as the session
                // and a peer that closes releases it. Without this the node's *own* dials were
                // bounded by nothing: a dialer could hold `MAX_PEERS` sessions against a ceiling of
                // 64, which is the number its operator was told.
                let permit = crate::capacity::reserve_dial_slot(&self.dials)?;
                let connection = self
                    .netlayer
                    .new_outgoing_connection_from(peer, crate::owner::session_origin(&self.session))
                    .await
                    .map_err(|e| e.to_string())?;
                let identity = crate::conn::Identity::fresh(self.location.clone())
                    .map_err(|e| e.to_string())?;
                let session =
                    Session::dial_deferred(connection, &identity, Arc::new(Bootstrap::default()))
                        .await
                        .map_err(|e| e.to_string())?;
                let (handle, loop_, _context) = session.split();
                // The registry decides the crossing; when *this* session is the loser its owner must
                // abort it, which is the suite's expectation on one of the two variants.
                match self.registry.admit(peer, &handle) {
                    Ok(losers) => {
                        for loser in losers {
                            loser.abort().await;
                        }
                    }
                    Err(_) => {
                        // The loser must *write* its `op:abort`, and its loop owns the socket — so it
                        // runs once, for that message, rather than being dropped unsent.
                        handle.abort().await;
                        let _ = loop_.run().await;
                        self.registry.forget(peer, &handle.own_pi, handle.dialed);
                        return Err("crossed hellos: this session was aborted".to_string());
                    }
                }
                // The session serves until the peer closes it, so its loop runs in its own task —
                // which is the whole point of the handle: this delivery is blocked on the fetch below,
                // and the session must be able to serve meanwhile (a *later* start-session from the
                // peer completes the handshake right there in the loop).
                tokio::spawn(async move {
                    // The permit lives exactly as long as the session: held for the loop's whole run,
                    // released when this task ends, whichever way it ends.
                    let _permit = permit;
                    let _ = loop_.run().await;
                });
                handle
            }
        };

        let (catcher, rx) = catcher();
        handle
            .deliver(HandOff {
                // Position 0 is the peer's bootstrap, by definition.
                to: Desc::Export(0u64.into()),
                args: vec![
                    Value::Symbol("fetch".to_string()),
                    // **Bytes, and that is a decision with a citation** (C224 item 3). The Locators
                    // draft calls the swiss number a string and Endo sends one; the *reference suite*
                    // sends bytes and **asserts we send bytes back** — `third_party_handoffs.py`'s
                    // `test_provides_valid_handoff_give` compares this very argument against
                    // `sturdyref.swiss_num`, which the suite builds as `…encode("ascii")`. Following
                    // the draft here would fail a conformance test, so the port follows the
                    // implementation, as it does for `op:start-session`'s field count (AUDIT C216).
                    // Inbound, the bootstrap accepts either, which is what lets us *serve* a peer that
                    // speaks the draft's spelling (AUDIT C217).
                    Value::Bytes(swiss.to_vec()),
                ],
                resolve_me: Some(catcher),
            })
            .await
            .map_err(|e: ConnectionError| e.to_string())?;

        let fetched = match tokio::time::timeout(self.timeout, rx).await {
            Err(_) => return Err("the peer did not answer the fetch in time".to_string()),
            Ok(Err(_)) => return Err("the session ended before the fetch was answered".to_string()),
            Ok(Ok(Err(reason))) => return Err(reason),
            Ok(Ok(Ok(value))) => value,
        };
        // The answer is a descriptor from that peer's table: `<desc:import-object N>` names *its*
        // export N, which we address as `Desc::Export(N)` (the inversion `Session::fulfil` makes on
        // the reply path).
        let to = Forward::address_of(&fetched).ok_or_else(|| {
            "the peer answered the fetch with something that is not an object".to_string()
        })?;
        // The raw value goes back too: a Gifter deposits the *descriptor* it was handed, unchanged.
        Ok((handle, to, fetched))
    }
}

#[async_trait]
impl Export for Enlivener {
    async fn deliver(&self, args: &[Value]) -> Result<Act, String> {
        let Some(sturdyref) = args.first() else {
            return Err("the enlivener expects a sturdyref".to_string());
        };
        let sturdyref = Sturdyref::from_syrup(sturdyref).map_err(|e| e.to_string())?;
        // With the session it is serving known, enlivening *is* a handoff's Gifter half — deposit
        // the object with the exporter and answer with the give — and the suite's handoff fixture is
        // exactly that. The session slot is filled in as soon as the session exists, and this
        // delivery cannot have arrived before it did.
        let serving = self.session.lock().unpoison().clone();
        match serving {
            Some(serving) => self.gift(&sturdyref, &serving).await,
            // A peer that published the enlivener without a session (a bare fixture, not a session's
            // bootstrap) still gets the enlivening itself, which is the part that needs no give.
            None => {
                let (handle, to, _) = self
                    .dial_and_fetch(&sturdyref.peer, &sturdyref.swiss_num)
                    .await?;
                Ok(Act::object(Arc::new(Forward::new(handle, to))))
            }
        }
    }
}
