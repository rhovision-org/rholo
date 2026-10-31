//! The OCapN conformance suite's fixture objects.
//!
//! The suite reaches these by `fetch`ing fixed swiss numbers from the bootstrap and then delivering
//! to the result; the names and behaviours are the ones in `ocapn-test-suite/tests/`. They exist to
//! make the protocol testable, not to be useful: the echo, the car factory, the promise resolver,
//! the greeter and the *sturdyref enlivener* are all implemented. The last two dial, so they are
//! published by [`publish_dialing_fixtures`] rather than by [`conformance_bootstrap`] — they need
//! the netlayer, our location, the session registry and the slot that tells them which session they
//! are serving.

use rchain_shared::lock::Unpoison;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;

use crate::bootstrap::Bootstrap;
use crate::captp::Desc;
use crate::conn::{Act, Export, ListenOutcome, Outgoing, Reply};
use crate::locator::PeerLocator;
use crate::netlayer::Netlayer;
use crate::owner::{SessionRegistry, SessionSlot};
use crate::syrup::Value;

/// The swiss numbers the suite fetches, in its own spelling.
pub const CAR_FACTORY_BUILDER: &[u8] = b"JadQ0++RzsD4M+40uLxTWVaVqM10DcBJ";
pub const ECHO_GC: &[u8] = b"IO58l1laTyhcrgDKbEzFOO32MDd6zE5w";
pub const GREETER: &[u8] = b"VMDDd1voKWarCe2GvgLbxbVFysNzRPzx";
pub const PROMISE_RESOLVER: &[u8] = b"IokCxYmMj04nos2JN1TDoY1bT8dXh6Lr";
pub const STURDYREF_ENLIVENER: &[u8] = b"gi02I1qghIwPiKGKleCQAOhpy3ZtYRpB";

/// A bootstrap carrying every fixture that needs nothing but a directory: the ones that only answer.
///
/// The greeter and the enlivener are **not** here: both dial (the enlivener to enliven a sturdyref,
/// the greeter as a handoff's Receiver), so they need the netlayer, our location, the session
/// registry and the session slot — see [`publish_dialing_fixtures`].
pub fn conformance_bootstrap() -> Bootstrap {
    let mut bootstrap = Bootstrap::default();
    bootstrap.publish(CAR_FACTORY_BUILDER, Arc::new(CarFactoryBuilder));
    bootstrap.publish(ECHO_GC, Arc::new(Echo));
    bootstrap.publish(PROMISE_RESOLVER, Arc::new(PromiseResolver));
    bootstrap
}

/// The same fixtures, in a bootstrap wired for **handoffs and dialing**: the shared gift store, this
/// session's slot, and the registry. A peer that will serve the suite's greeter or enlivener needs
/// this one, because those two dial and because a handoff is deposited on one session and withdrawn
/// on another.
pub fn conformance_bootstrap_with(
    handoffs: Arc<crate::handoff::Handoffs>,
    session: crate::owner::SessionSlot,
    registry: Arc<crate::owner::SessionRegistry>,
) -> Bootstrap {
    let mut directory: std::collections::BTreeMap<Vec<u8>, Arc<dyn Export>> =
        std::collections::BTreeMap::new();
    directory.insert(CAR_FACTORY_BUILDER.to_vec(), Arc::new(CarFactoryBuilder));
    directory.insert(ECHO_GC.to_vec(), Arc::new(Echo));
    directory.insert(PROMISE_RESOLVER.to_vec(), Arc::new(PromiseResolver));
    Bootstrap::with_handoffs(directory, handoffs, session, registry)
}

/// Publish the two fixtures that dial: the sturdyref enlivener and the greeter.
///
/// Both are per-peer objects — they dial, so they need the netlayer to dial with, our own location to
/// advertise, the registry the crossed-hello rule lives in, and the slot that tells them **which
/// session they are serving** (the greeter signs a handoff receive with that session's secret, and
/// the enlivener names its peer's key in a give).
///
/// The suite fetches both by swiss number, so every peer the suite runs against publishes both.
pub fn publish_dialing_fixtures(
    bootstrap: &mut Bootstrap,
    netlayer: Arc<dyn crate::netlayer::Netlayer>,
    location: crate::locator::PeerLocator,
    registry: Arc<crate::owner::SessionRegistry>,
    session: crate::owner::SessionSlot,
) {
    publish_dialing_fixtures_with_budget(bootstrap, netlayer, location, registry, session, None);
}

/// [`publish_dialing_fixtures`] with the **dial ceiling** the node shares with the admin route
/// (HAZOP row C241), so a fixture that dials draws from the same budget as a dial the operator
/// starts.
pub fn publish_dialing_fixtures_with_budget(
    bootstrap: &mut Bootstrap,
    netlayer: Arc<dyn crate::netlayer::Netlayer>,
    location: crate::locator::PeerLocator,
    registry: Arc<crate::owner::SessionRegistry>,
    session: crate::owner::SessionSlot,
    dials: Option<Arc<tokio::sync::Semaphore>>,
) {
    bootstrap.publish(
        GREETER,
        Arc::new(Greeter {
            netlayer: netlayer.clone(),
            location: location.clone(),
            registry: registry.clone(),
            session: session.clone(),
            // The greeter dials too, so it draws from the **same** budget as the enlivener and the
            // admin route (C347). It used to draw from nothing, which capped it at `MAX_PEERS`.
            dials: dials.clone(),
        }),
    );
    bootstrap.publish(
        STURDYREF_ENLIVENER,
        Arc::new(crate::enliven::Enlivener::with_dial_budget(
            netlayer, location, registry, session, dials,
        )),
    );
}

/// The greeter: the fixture the suite hands objects to.
///
/// It has **two jobs**, because the suite uses it for both:
///
/// * handed an object, it delivers `["Hello"]` to it, with an `answer-position` and a sink to resolve
///   into — the path `op_deliver`/`op_gc` drive;
/// * handed a **signed handoff give**, it is the *Receiver* of a third-party handoff: it dials the
///   exporter the give names, and claims the gift with a signed *receive*. That is what
///   `third_party_handoffs`' receiver role expects of whichever object the give is delivered to.
///
/// The dialing half needs the netlayer and the registry, so the greeter is built per peer like the
/// enlivener is; and the receive is signed with **this session's** secret, which the give's
/// `receiver-key` names — so the slot has to be filled in by whoever owns the session.
struct Greeter {
    netlayer: Arc<dyn Netlayer>,
    location: PeerLocator,
    registry: Arc<SessionRegistry>,
    session: SessionSlot,
    /// The node's own-dial budget, shared with the enlivener and the admin route (C347). `None` is
    /// the conformance harness, which installs none.
    dials: Option<Arc<tokio::sync::Semaphore>>,
}

#[async_trait]
impl Export for Greeter {
    async fn deliver(&self, args: &[Value]) -> Result<Act, String> {
        let Some(arg) = args.first() else {
            return Err("the greeter needs an object to greet".to_string());
        };
        // A signed handoff give is the other thing this object is handed. Anything else is an object
        // reference to greet — including a bare envelope for another record, which the greeting path
        // then refuses as not-an-object.
        if let Ok(envelope) = crate::handoff::Envelope::from_syrup(arg) {
            if crate::handoff::HandoffGive::from_syrup(&envelope.object).is_ok() {
                return self.receive(envelope).await;
            }
        }
        // The argument is a descriptor for an object of the peer's. Address it back the way the
        // reference does: an `import` becomes the matching `export`.
        let to = match Desc::from_syrup(arg) {
            Ok(Desc::ImportObject(n) | Desc::ImportPromise(n)) => Desc::Export(n),
            Ok(other) => other,
            Err(_) => return Err("the greeter expects an object reference".to_string()),
        };
        // The greeting carries an `answer-position` and hands over a sink, so the peer can resolve
        // the delivery — which is what the suite's `op:gc-answers` test drives.
        Ok(Act {
            out: vec![Outgoing {
                to,
                args: vec![Value::String("Hello".to_string())],
                answer: true,
                hand_out: Some(Arc::new(Sink)),
            }],
            reply: Reply::Nothing,
        })
    }
}

impl Greeter {
    /// The Receiver's half of a handoff: dial the exporter the give names, and claim the gift with a
    /// receive signed by this session's key.
    async fn receive(&self, signed_give: crate::handoff::Envelope) -> Result<Act, String> {
        let give = crate::handoff::HandoffGive::from_syrup(&signed_give.object)?;
        let session = self
            .session
            .lock()
            .unpoison()
            .clone()
            .ok_or_else(|| "the greeter has no session to sign with".to_string())?;

        // Dial the exporter — reusing a session if there is one, exactly as the enlivener does, and
        // for the same reason: a second connection to a peer we are already talking to is a session
        // nobody is reading.
        let peer = give.exporter_location.clone();
        let handle = match self.registry.live(&peer) {
            Some(existing) => existing,
            None => {
                // **A greeter's dial draws from the budget too** (C347). It dialled with no permit,
                // so a peer's handoff could make the node hold `MAX_PEERS` sessions against a
                // published ceiling of `MAX_DIALED_SESSIONS`. Taken before the connect and held by
                // the loop task below, exactly as the enlivener does it.
                let permit = crate::capacity::reserve_dial_slot(&self.dials)?;
                let connection = self
                    .netlayer
                    .new_outgoing_connection_from(
                        &peer,
                        crate::owner::session_origin(&self.session),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                let identity = crate::conn::Identity::fresh(self.location.clone())
                    .map_err(|e| e.to_string())?;
                let s = crate::conn::Session::dial(
                    connection,
                    &identity,
                    Arc::new(crate::bootstrap::Bootstrap::default()),
                )
                .await
                .map_err(|e| e.to_string())?;
                let (handle, loop_, _context) = s.split();
                match self.registry.admit(&peer, &handle) {
                    Ok(losers) => {
                        for loser in losers {
                            loser.abort().await;
                        }
                    }
                    Err(_) => {
                        handle.abort().await;
                        let _ = loop_.run().await;
                        self.registry.forget(&peer, &handle.own_pi, handle.dialed);
                        return Err("crossed hellos: this session was aborted".to_string());
                    }
                }
                tokio::spawn(async move {
                    // Held for the session's whole life, whichever way the loop ends.
                    let _permit = permit;
                    let _ = loop_.run().await;
                });
                handle
            }
        };

        // The receive names the session it is for — the one we just made with the exporter — and is
        // signed with **this** session's secret, which is the key the give's `receiver-key` names.
        let id = handle
            .id
            .clone()
            .ok_or_else(|| "the exporter session has no id yet".to_string())?;
        let receive = crate::handoff::HandoffReceive {
            receiving_session: id.to_vec(),
            receiving_side: handle.own_pi.to_vec(),
            handoff_count: 0,
            signed_give,
        };
        let claim = crate::handoff::Envelope::sign(receive.to_syrup(), &session.secret)?;
        handle
            .deliver(crate::owner::HandOff {
                to: Desc::Export(0u64.into()),
                args: vec![Value::Symbol("withdraw-gift".to_string()), claim.to_syrup()],
                resolve_me: None,
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok(Act::nothing())
    }
}

/// Accepts a resolution and does nothing with it: where a greeting's fulfilment lands.
struct Sink;

#[async_trait]
impl Export for Sink {
    async fn deliver(&self, _args: &[Value]) -> Result<Act, String> {
        Ok(Act::nothing())
    }
}

/// Replies with its arguments, unchanged.
struct Echo;

#[async_trait]
impl Export for Echo {
    async fn deliver(&self, args: &[Value]) -> Result<Act, String> {
        Ok(Act::value(Value::List(args.to_vec())))
    }
}

/// `fetch`ed first; hands back a [`CarFactory`].
struct CarFactoryBuilder;

#[async_trait]
impl Export for CarFactoryBuilder {
    async fn deliver(&self, _args: &[Value]) -> Result<Act, String> {
        Ok(Act::object(Arc::new(CarFactory)))
    }
}

/// Builds a [`Car`] from one `[colour, model]` pair of symbols.
struct CarFactory;

#[async_trait]
impl Export for CarFactory {
    async fn deliver(&self, args: &[Value]) -> Result<Act, String> {
        let Some(Value::List(pair)) = args.first() else {
            return Err("a car factory expects one [colour, model] pair".to_string());
        };
        let [Value::Symbol(colour), Value::Symbol(model)] = pair.as_slice() else {
            return Err("a car needs a colour and a model, as symbols".to_string());
        };
        Ok(Act::object(Arc::new(Car {
            colour: colour.clone(),
            model: model.clone(),
        })))
    }
}

/// Says what it is.
struct Car {
    colour: String,
    model: String,
}

#[async_trait]
impl Export for Car {
    async fn deliver(&self, _args: &[Value]) -> Result<Act, String> {
        Ok(Act::value(Value::String(format!(
            "Vroom! I am a {} {} car!",
            self.colour, self.model
        ))))
    }
}

/// Lock without letting a poisoned mutex take the session down: a poisoned lock means an earlier
/// delivery panicked, and the honest response is to keep answering rather than to panic again.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unpoison()
}

/// Where a promise has got to.
#[derive(Default, Clone)]
enum PromiseState {
    #[default]
    Pending,
    Fulfilled(Value),
    Broken(Value),
}

/// One cell shared by a [`Vow`] and its [`Resolver`], so a listener registered before the
/// settlement and one registered after see the same answer.
#[derive(Default)]
struct PromiseCell {
    state: Mutex<PromiseState>,
    /// Descriptors to deliver the settlement to, in registration order.
    listeners: Mutex<Vec<Desc>>,
}

/// `fetch`ed at `PROMISE_RESOLVER`: hands back a fresh `[vow, resolver]` pair.
struct PromiseResolver;

#[async_trait]
impl Export for PromiseResolver {
    async fn deliver(&self, _args: &[Value]) -> Result<Act, String> {
        let cell = Arc::new(PromiseCell::default());
        Ok(Act::objects(vec![
            Arc::new(Vow(cell.clone())),
            Arc::new(Resolver(cell)),
        ]))
    }
}

/// The promise half: listenable, and not deliverable until it resolves.
struct Vow(Arc<PromiseCell>);

#[async_trait]
impl Export for Vow {
    async fn deliver(&self, _args: &[Value]) -> Result<Act, String> {
        Err("this promise has not resolved into something deliverable".to_string())
    }

    fn listen(&self, listener: Desc) -> Option<ListenOutcome> {
        let settled = lock(&self.0.state).clone();
        match settled {
            PromiseState::Pending => {
                let mut listeners = lock(&self.0.listeners);
                // **A bounded promise refuses rather than forgets.** An unresolved vow accepted one
                // `op:listen` per message for ever, so a peer grew this list without limit
                // (AUDIT C223, measured); the cap is per cell, and the cell count is itself bounded
                // by the session's export table.
                if listeners.len() >= crate::capacity::MAX_LISTENERS {
                    return Some(ListenOutcome::Refused(format!(
                        "this promise already has {} listeners, its cap",
                        crate::capacity::MAX_LISTENERS
                    )));
                }
                listeners.push(listener);
                Some(ListenOutcome::Registered)
            }
            PromiseState::Fulfilled(v) => Some(ListenOutcome::Settled(vec![
                Value::Symbol("fulfill".to_string()),
                v,
            ])),
            PromiseState::Broken(e) => Some(ListenOutcome::Settled(vec![
                Value::Symbol("break".to_string()),
                e,
            ])),
        }
    }
}

/// The resolver half: `[<fulfill> <value>]` or `[<break> <reason>]` settles the pair and notifies
/// everyone who listened.
struct Resolver(Arc<PromiseCell>);

#[async_trait]
impl Export for Resolver {
    async fn deliver(&self, args: &[Value]) -> Result<Act, String> {
        let [Value::Symbol(verb), value] = args else {
            return Err("a resolver takes [<fulfill|break> <value>]".to_string());
        };
        let (settled, notification) = match verb.as_str() {
            "fulfill" => (
                PromiseState::Fulfilled(value.clone()),
                vec![Value::Symbol("fulfill".to_string()), value.clone()],
            ),
            "break" => (
                PromiseState::Broken(value.clone()),
                vec![Value::Symbol("break".to_string()), value.clone()],
            ),
            other => return Err(format!("unknown resolver verb {other:?}")),
        };
        *lock(&self.0.state) = settled;
        let listeners: Vec<Desc> = std::mem::take(&mut *lock(&self.0.listeners));
        Ok(Act {
            out: listeners
                .into_iter()
                .map(|to| Outgoing::to(to, notification.clone()))
                .collect(),
            reply: Reply::Nothing,
        })
    }
}
