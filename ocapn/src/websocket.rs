//! The `websocket` netlayer — the transport `@endo/ocapn` speaks, and the one a *published* peer can
//! be pointed at.
//!
//! **Why this exists when [`crate::noise`] is stronger.** Noise encrypts and authenticates *both*
//! ends. This one, as the reference implementation writes it, does neither: Endo's netlayer is `ws://`
//! with **no TLS**, and its in-band handshake has the **server** prove its identity while the client
//! proves nothing. What it has instead is a peer that exists today — `@endo/ocapn` 1.1.1 is on npm and
//! already interoperates with this repository (`spec/audit/evidence/endo-spike/`) — so it is the
//! transport to test *against*, and the one to reach a deployment that speaks nothing else. `wss://`
//! is **implemented but not reachable from the node's configuration** — `bind_tls` has no caller and
//! no config key selects it — so the node today advertises `ws://` and nothing else. That half's
//! reference is Goblins rather than Endo, since Endo has no TLS at all.
//!
//! **The framing is one WebSocket frame per CapTP message, with nothing inside it.** Endo's
//! `socketOps.write(bytes)` goes straight to `ws.send(bytes, { binary: true })` and its reader takes a
//! whole frame as a whole message: no length prefix and no netstring, because WebSocket already frames.
//! So [`crate::framed`] is *not* reused — its own doc says a transport with different framing writes
//! its own module. **The reference sets no size limit and no timeout anywhere**; the bound and the
//! deadline below are this port's additions, and are marked as such rather than presented as parity.
//!
//! **The in-band handshake, before any CapTP byte.** The client sends one `init:peer-auth` record
//! carrying 32 random bytes; the server signs **the bytes it received, verbatim**, with its designator
//! key, and replies with a `desc:sig-envelope` carrying that record and the signature; the client
//! checks the signature against the challenge it sent. Only then does CapTP start. The record wrapper
//! is what keeps the server from being a signing oracle — it signs a typed payload, not arbitrary
//! bytes — and this port mirrors it for that reason rather than for convenience.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use rchain_crypto::signatures::ed25519::Ed25519;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{accept_async_with_config, connect_async_with_config, WebSocketStream};

use crate::framed::CONNECT_TIMEOUT;
use crate::locator::PeerLocator;
use crate::netlayer::{NetConn, Netlayer};
use crate::noise::NoiseIdentity;
use crate::syrup::Value;

/// The one hint this transport reads: the whole URL, exactly as `@endo/ocapn` reads it
/// (`remoteLocation.hints.url`, used verbatim). Endo appends no path and no query, so neither do we.
const URL_HINT: &str = "url";

/// The largest WebSocket message this transport will take, matching [`crate::framed`]'s bound.
/// **The reference has no such limit** — `ws` defaults are permissive and its `pendingWrites` array
/// grows unbounded until authentication — so this is a thing the port adds.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// The challenge length: `DESIGNATOR_CHALLENGE_PAYLOAD_BYTES = 32` in the reference.
const CHALLENGE_LEN: usize = 32;

/// The `init:peer-auth` record's label, and the envelope's, as the reference spells them.
const INIT_PEER_AUTH: &str = "init:peer-auth";
const SIG_ENVELOPE: &str = "desc:sig-envelope";

/// How long the in-band handshake may take. **The reference sets no handshake deadline**, and without
/// one a peer that opens a socket and never sends its challenge holds the connection for ever — which
/// on the node's side is a session slot.
const AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long the TLS handshake and the WebSocket upgrade may take, together. **The reference bounds
/// neither**, and without a bound a peer that connects and then sends nothing holds this connection
/// for ever. Both this and [`AUTH_TIMEOUT`] now run on the connection's **first use** rather than in
/// the accept loop (C349), so the bound is a session's, not the listener's.
const ESTABLISH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The WebSocket layer's own bounds.
///
/// **Without these, this transport's declared bound was not the operative one.** `MAX_MESSAGE_BYTES`
/// was checked *after* `into_data()`, i.e. after tungstenite had already assembled and allocated the
/// whole message, so what actually sized the allocation was tungstenite's defaults — 16 MiB frames and
/// a 64 MiB message — on both the data path **and** the handshake read, which had no check at all
/// (HAZOP rows A5 and A6). Setting them here puts the refusal where `framed` puts it: before the
/// buffer, not after it.
fn ws_config() -> WebSocketConfig {
    // **A builder, because 0.30 made `WebSocketConfig` `#[non_exhaustive]`** — the struct-expression
    // form this used to be is a compile error now, and the two limits are the same two (HAZOP rows A5
    // and A6): they put the refusal before the buffer rather than after it.
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
}

/// The byte stream under the WebSocket layer, as a trait object — the one place the two ways in
/// differ, since a plain listener yields `TcpStream` and a TLS one yields `TlsStream<TcpStream>`.
///
/// **The supertraits are named in a local trait because Rust allows only auto traits to be added to a
/// trait object**: `dyn AsyncRead + AsyncWrite` is not a type, but `dyn ByteStream` is, and `dyn
/// ByteStream: AsyncRead + AsyncWrite` follows from the bound.
trait ByteStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> ByteStream for T {}
type BoxedIo = Box<dyn ByteStream>;

/// A `websocket` endpoint. The same value dials and accepts.
pub struct WebsocketNetlayer {
    listener: TcpListener,
    /// The node's identity. Only the Ed25519 half is used here — what a responder signs the challenge
    /// with, and what a dialler checks it against. See [`NoiseIdentity::sign`].
    identity: NoiseIdentity,
    /// `Some` accepts `wss://`; `None` accepts `ws://`, which is what Endo speaks and what the interop
    /// harness needs.
    tls: Option<Arc<tokio_rustls::rustls::ServerConfig>>,
}

impl WebsocketNetlayer {
    /// Bind a plain `ws://` listener. `addr` is `host:port`; `127.0.0.1:0` asks the OS for a free port.
    pub async fn bind(addr: &str, identity: NoiseIdentity) -> io::Result<WebsocketNetlayer> {
        Ok(WebsocketNetlayer {
            listener: TcpListener::bind(addr).await?,
            identity,
            tls: None,
        })
    }

    /// Bind a `wss://` listener. Inbound connections complete a TLS handshake before the WebSocket
    /// one; outbound ones still choose by the URL's scheme.
    pub async fn bind_tls(
        addr: &str,
        identity: NoiseIdentity,
        tls: Arc<tokio_rustls::rustls::ServerConfig>,
    ) -> io::Result<WebsocketNetlayer> {
        Ok(WebsocketNetlayer {
            listener: TcpListener::bind(addr).await?,
            identity,
            tls: Some(tls),
        })
    }

    /// The bound address, including the port chosen for `:0`.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// The location to advertise: **the designator Endo derives and the `url` hint it reads, and
    /// nothing else.**
    ///
    /// **Two rules, and the transport owns both (AUDIT C245).** Endo identifies a location by
    /// `ocapn://<designator>.<transport>?<sorted hints>` — *every* hint is part of the key it resolves
    /// a session under (`@endo/ocapn/src/client/util.js`), and the designator it *decodes* as base32.
    /// The session is stored under the location the peer advertised and looked up under the one the
    /// peer dialled, so an advertised location that differs from the dialled one in **either** field
    /// means the pending promise never resolves and the fetch that would follow it is never sent.
    /// That is what recorded the websocket transport as a hanging one, and it was this function's
    /// shape rather than the peer's client.
    ///
    /// So the designator is `base32(Ed25519 verifying key)` — the peer's own convention, which it
    /// derives independently — and the key is **not** repeated as a `verify` hint, because a hint is
    /// part of the identity and an extra one makes this location a different location. A dialler still
    /// has to know this key to prefix its challenge, and it has to learn it out of band either way:
    /// the hint was a convenience, never a source.
    ///
    /// **`host` is the host a *peer* should dial, when the operator has named one** (HAZOP row C237).
    /// The bound address is the default and is right for a specific one — but a bind to `0.0.0.0` or
    /// `::` yields a URL no other host can use, so the node's caller passes the advertised host
    /// instead of deriving it from `local_addr()`. The port is always the bound one: `:0` is legal, and
    /// the chosen port is the only useful thing to advertise.
    pub fn location(&self, host: Option<&str>) -> io::Result<PeerLocator> {
        let local = self.local_addr()?;
        let scheme = if self.tls.is_some() { "wss" } else { "ws" };
        let host = host.unwrap_or_else(|| "").to_string();
        let host = if host.is_empty() {
            local.ip().to_string()
        } else {
            host
        };
        Ok(PeerLocator {
            designator: rchain_shared::base32::encode(&self.identity.verifying_key()),
            transport: "websocket".to_string(),
            hints: std::collections::BTreeMap::from([(
                URL_HINT.to_string(),
                format!("{scheme}://{host}:{}", local.port()),
            )]),
        })
    }
}

#[async_trait]
impl Netlayer for WebsocketNetlayer {
    async fn new_outgoing_connection(&self, locator: &PeerLocator) -> io::Result<Box<dyn NetConn>> {
        let url = locator.hints.get(URL_HINT).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "websocket needs a `url` hint, the whole ws:// or wss:// address",
            )
        })?;
        // **The peer's Ed25519 key, which the challenge/response checks its signature against.** A dial
        // that cannot name the peer cannot tell an impostor from it, so this is refused, not skipped.
        let peer = peer_key(locator)?;

        let (stream, _response) = tokio::time::timeout(
            CONNECT_TIMEOUT,
            connect_async_with_config(url, Some(ws_config()), false),
        )
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("connecting to {url} took longer than {CONNECT_TIMEOUT:?}"),
            )
        })?
        .map_err(io::Error::other)?;
        let peer_addr = match stream.get_ref() {
            tokio_tungstenite::MaybeTlsStream::Plain(tcp) => tcp.peer_addr().ok(),
            // A TLS stream's address is not exposed here; the dial policy reads an unknown origin as
            // "cannot be judged", which is the behaviour it had before this transport existed.
            _ => None,
        };
        let mut conn = WsConn::new(stream, peer_addr);
        authenticate_as_client(&mut conn, &peer).await?;
        Ok(Box::new(conn))
    }

    /// **The socket is accepted and nothing else** (C349). The transport, the WebSocket upgrade and
    /// the in-band challenge run on the connection's first use — `recv` — each bounded, exactly as
    /// [`crate::noise`] runs its handshake in the session task.
    ///
    /// Before this, all three ran *here*, inside the node's accept loop, so one peer that connected
    /// and then stayed silent held that loop for up to `ESTABLISH_TIMEOUT + AUTH_TIMEOUT` (40 s).
    async fn accept_incoming_connection(&self) -> io::Result<Box<dyn NetConn>> {
        let (tcp, peer) = self.listener.accept().await?;
        Ok(Box::new(PendingWsConn {
            tcp: Some(tcp),
            tls: self.tls.clone(),
            identity: self.identity.clone(),
            peer,
            ready: None,
        }))
    }
}

/// **A connection this node has accepted but not yet established** (C349).
///
/// The accept is a socket accept; the TLS handshake, the WebSocket upgrade and the server's side of
/// the challenge happen on the first `recv`. A peer that connects and goes silent therefore costs one
/// queued session slot bounded by [`ESTABLISH_TIMEOUT`] + [`AUTH_TIMEOUT`], not the accept loop — the
/// price the `noise` transport already pays.
struct PendingWsConn {
    /// The socket, until the first use consumes it.
    tcp: Option<tokio::net::TcpStream>,
    tls: Option<Arc<tokio_rustls::rustls::ServerConfig>>,
    identity: NoiseIdentity,
    peer: SocketAddr,
    /// The established connection, once the first use has run.
    ready: Option<WsConn<BoxedIo>>,
}

impl PendingWsConn {
    /// Run the transport, the upgrade and the server's challenge, **once**. A second call is the
    /// established connection.
    async fn ensure(&mut self) -> io::Result<&mut WsConn<BoxedIo>> {
        if self.ready.is_none() {
            let tcp = self
                .tcp
                .take()
                .ok_or_else(|| io::Error::other("the websocket socket was already consumed"))?;
            let establishing = async {
                // The transport first, then the WebSocket layer. The two arms produce different
                // stream types, so the byte stream is boxed *before* the WebSocket layer is built —
                // after which everything downstream is one type.
                let io: BoxedIo = match &self.tls {
                    Some(config) => {
                        let acceptor = tokio_rustls::TlsAcceptor::from(config.clone());
                        Box::new(acceptor.accept(tcp).await?)
                    }
                    None => Box::new(tcp),
                };
                accept_async_with_config(io, Some(ws_config()))
                    .await
                    .map_err(io::Error::other)
            };
            let stream = tokio::time::timeout(ESTABLISH_TIMEOUT, establishing)
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("the websocket upgrade took longer than {ESTABLISH_TIMEOUT:?}"),
                    )
                })??;
            let mut conn = WsConn::new(stream, Some(self.peer));
            authenticate_as_server(&mut conn, &self.identity).await?;
            self.ready = Some(conn);
        }
        // Total rather than an `expect`: the arm is unreachable, and the type system's gate is right
        // that an unreachable panic is still a panic site in production code.
        match self.ready.as_mut() {
            Some(conn) => Ok(conn),
            None => Err(io::Error::other(
                "the websocket connection is not established",
            )),
        }
    }
}

#[async_trait]
impl NetConn for PendingWsConn {
    async fn send(&mut self, message: &[u8]) -> io::Result<()> {
        // The client speaks first in this protocol, so the established path is reachable only after a
        // `recv`. Before that, sending is the same refusal as an unauthenticated connection — the
        // reference's rule, kept rather than buffered.
        match self.ready.as_mut() {
            Some(conn) => conn.send(message).await,
            None => Err(io::Error::other(
                "the websocket peer-authentication handshake has not completed",
            )),
        }
    }

    fn peer_address(&self) -> Option<SocketAddr> {
        Some(self.peer)
    }

    fn verified_peer(&self) -> Option<[u8; 32]> {
        // The accepting side never learns a proven key: the reference's handshake has only the server
        // prove itself (see `WsConn::verified`).
        self.ready.as_ref().and_then(|conn| conn.verified)
    }

    async fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.ensure().await?.recv().await
    }
}

/// The peer's Ed25519 verifying key, from the locator's `verify` hint (base16).
///
/// **The locator convention is unsettled**, exactly as it is for `noise`: Endo names a peer by a base32
/// public key in the location's `designator`, and this port reads a hint instead because the convention
/// is pinned by nothing reachable. Stated rather than implied.
fn peer_key(locator: &PeerLocator) -> io::Result<[u8; 32]> {
    let encoded = locator.hints.get("verify").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "websocket needs a `verify` hint: the peer's Ed25519 verifying key, base16",
        )
    })?;
    rchain_shared::base16::decode(encoded)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "websocket `verify` is not 32 bytes of base16",
            )
        })
}

/// The client's half: send the challenge, read the envelope, check the signature over our own bytes.
async fn authenticate_as_client<S>(conn: &mut WsConn<S>, peer: &[u8; 32]) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    use rand::Rng;
    let mut challenge = [0u8; CHALLENGE_LEN];
    rand::rng().fill_bytes(&mut challenge);
    let bytes = peer_auth(&challenge).to_bytes();

    conn.send_raw(Message::Binary(bytes.clone().into())).await?;
    let reply = conn.next_raw(AUTH_TIMEOUT).await?;
    let envelope = Value::from_bytes(&reply)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let signature = signature_of_envelope(&envelope)?;
    // **The signature covers the bytes the server received**, which are the bytes we sent — so this
    // checks our own challenge, not a re-encoding of it.
    if !Ed25519::verify_bytes(&bytes, &signature, peer) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the websocket peer did not prove it holds the key this dial named",
        ));
    }
    conn.authenticated = true;
    // The key we dialled *by* is the key the peer proved — a dialler names the peer it expects, so a
    // successful check is that name being confirmed (HAZOP row C242).
    conn.verified = Some(*peer);
    Ok(())
}

/// The server's half: read the challenge, sign **the bytes received**, answer with the envelope.
async fn authenticate_as_server<S>(conn: &mut WsConn<S>, identity: &NoiseIdentity) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let received = conn.next_raw(AUTH_TIMEOUT).await?;
    // Decoded as well as kept: the record wrapper is what keeps this from being a signing oracle, so a
    // frame that is not an `init:peer-auth` is refused rather than signed.
    let record = Value::from_bytes(&received)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    if !is_peer_auth(&record) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the first websocket message was not an `init:peer-auth`",
        ));
    }
    let signature = identity
        .sign(&received)
        .map_err(|e| io::Error::other(format!("signing the challenge: {e}")))?;
    // **The gcrypt s-expression, not raw bytes**, because that is what the reference's
    // `OcapnSignature` codec reads: `['sig-val ['eddsa ['r …] ['s …]]]`. A bytestring here is a parse
    // failure on the peer — which is exactly how this was found, by the interop run and not by these
    // module's own tests, which agreed with themselves.
    let signature = crate::session::signature_syrup(&signature)
        .map_err(|e| io::Error::other(format!("encoding the signature: {e}")))?;
    let envelope = Value::Record(vec![
        Value::Symbol(SIG_ENVELOPE.to_string()),
        record,
        signature,
    ]);
    conn.send_raw(Message::Binary(envelope.to_bytes().into()))
        .await?;
    conn.authenticated = true;
    Ok(())
}

/// `<init:peer-auth payload:bytes>` — the record the client opens with.
fn peer_auth(challenge: &[u8]) -> Value {
    Value::Record(vec![
        Value::Symbol(INIT_PEER_AUTH.to_string()),
        Value::Bytes(challenge.to_vec()),
    ])
}

/// Whether a value is an `init:peer-auth` record with **exactly one** bytestring field **of the
/// challenge's length**.
///
/// **The length and the arity are checked, not just the label.** The responder signs these bytes, and
/// the record wrapper is what stops it being a signing oracle over a peer's choice — so accepting a
/// zero-byte "challenge", a multi-megabyte one, or a record with extra fields is accepting a signing
/// request the protocol never made (HAZOP row A7). `CHALLENGE_LEN` is the reference's
/// `DESIGNATOR_CHALLENGE_PAYLOAD_BYTES`, which the client uses to build its own.
fn is_peer_auth(value: &Value) -> bool {
    matches!(value,
        Value::Record(fields)
            if fields.len() == 2
                && fields[0] == Value::Symbol(INIT_PEER_AUTH.to_string())
                && matches!(&fields[1], Value::Bytes(bytes) if bytes.len() == CHALLENGE_LEN)
    )
}

/// The signature out of a `desc:sig-envelope` record.
fn signature_of_envelope(value: &Value) -> io::Result<Vec<u8>> {
    let missing = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the websocket peer's answer was not a `desc:sig-envelope`",
        )
    };
    match value {
        Value::Record(fields)
            if fields.first() == Some(&Value::Symbol(SIG_ENVELOPE.to_string())) =>
        {
            let signature = fields.get(2).ok_or_else(missing)?;
            // The peer's gcrypt s-expression, read back into the 64 bytes it stands for — the mirror
            // of what `authenticate_as_server` writes, and the reason the two are paired here rather
            // than each writing its own shape.
            crate::session::signature_bytes(signature).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the `desc:sig-envelope` signature is not the gcrypt form: {e}"),
                )
            })
        }
        _ => Err(missing()),
    }
}

/// One WebSocket connection. Messages are whole frames: one CapTP message each, with nothing added.
struct WsConn<S> {
    stream: WebSocketStream<S>,
    /// Captured where the socket was made, so the dial policy can judge this peer (Law 62) — unlike
    /// `unix`'s deliberate `None`.
    peer: Option<SocketAddr>,
    /// Set once the in-band handshake has completed. An unauthenticated connection carries no CapTP
    /// traffic — the reference's rule, enforced here by **refusing** rather than by buffering (the
    /// reference's `pendingWrites` grows without bound until auth succeeds).
    authenticated: bool,
    /// The key this connection *proved*, if it proved one — the server's, when this node dialled
    /// (HAZOP rows C242/C243). **`None` on the accepting side, and that is the protocol's shape rather
    /// than a gap in this port**: the reference's handshake has only the server prove itself, so a
    /// session this node *accepted* over `websocket` has no name its peer had to prove, and is booked
    /// under the designator the peer asserted. A transport that can name its peer does; this one
    /// cannot, and says so.
    verified: Option<[u8; 32]>,
}

impl<S> WsConn<S> {
    fn new(stream: WebSocketStream<S>, peer: Option<SocketAddr>) -> WsConn<S> {
        WsConn {
            stream,
            peer,
            authenticated: false,
            verified: None,
        }
    }
}

impl<S> WsConn<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn send_raw(&mut self, message: Message) -> io::Result<()> {
        self.stream.send(message).await.map_err(io::Error::other)
    }

    /// The next whole message, bounded by `limit`. `next_raw` is the handshake's read, where a close
    /// before an answer is an error rather than an end of stream.
    async fn next_raw(&mut self, limit: std::time::Duration) -> io::Result<Vec<u8>> {
        let next = tokio::time::timeout(limit, self.stream.next())
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the websocket peer did not answer in time",
                )
            })?;
        match next {
            None => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the websocket peer closed before it answered",
            )),
            Some(Err(e)) => Err(io::Error::other(e)),
            // **Only a binary frame is a challenge.** `into_data` hands back a text frame's payload
            // and a close frame's reason bytes, so the handshake used to sign whatever a peer put in
            // a *close* — and signing bytes of the peer's choosing is what the record wrapper exists
            // to prevent (HAZOP row A4). Anything that is not a binary message is refused here.
            Some(Ok(Message::Binary(bytes))) => Ok(bytes.to_vec()),
            Some(Ok(other)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("a websocket frame that is not a binary message: {other:?}"),
            )),
        }
    }
}

#[async_trait]
impl<S> NetConn for WsConn<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn send(&mut self, message: &[u8]) -> io::Result<()> {
        if !self.authenticated {
            return Err(io::Error::other(
                "the websocket peer-authentication handshake has not completed",
            ));
        }
        if message.len() > MAX_MESSAGE_BYTES {
            // Refused before the frame is built, the rule `framed` pins for netstrings.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a websocket message exceeds the transport's bound",
            ));
        }
        self.send_raw(Message::Binary(message.to_vec().into()))
            .await
    }

    fn peer_address(&self) -> Option<SocketAddr> {
        self.peer
    }

    fn verified_peer(&self) -> Option<[u8; 32]> {
        self.verified
    }

    async fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            let Some(next) = self.stream.next().await else {
                return Ok(None);
            };
            match next.map_err(io::Error::other)? {
                Message::Binary(bytes) => {
                    if bytes.len() > MAX_MESSAGE_BYTES {
                        // Belt to the configuration's braces: `ws_config` refuses this before the
                        // buffer, and this refuses it again if that ever stops being true.
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "a websocket message exceeds the transport's bound",
                        ));
                    }
                    return Ok(Some(bytes.to_vec()));
                }
                // **A close is the end of the stream, at a message boundary.** `into_data` would hand
                // back the close frame's *reason bytes* as though they were a message, and it does the
                // same for a text frame and for ping/pong — so this transport used to feed a peer's
                // close reason to the CapTP dispatch, and its own doc said the opposite (HAZOP row A4).
                Message::Close(_) => return Ok(None),
                // Ping/Pong are the WebSocket layer's own keepalive, and tungstenite answers them.
                Message::Ping(_) | Message::Pong(_) => continue,
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("a websocket frame that is not a binary message: {other:?}"),
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> NoiseIdentity {
        NoiseIdentity::generate().expect("a fresh identity")
    }

    /// A listener and the locator a client dials it by — which names it by the key its
    /// challenge/response is checked against.
    ///
    /// **The dialler adds the `verify` hint itself**, which is the whole shape of C245: the advertised
    /// location carries the key only as its `designator` (base32, the peer's convention), and the
    /// hint a dialler needs to prefix its challenge is learnt out of band rather than read off the
    /// advertisement.
    async fn listening() -> (WebsocketNetlayer, PeerLocator) {
        let server_identity = identity();
        let server = WebsocketNetlayer::bind("127.0.0.1:0", server_identity.clone())
            .await
            .expect("bind");
        let mut location = server.location(None).expect("location");
        location.hints.insert(
            "verify".to_string(),
            rchain_shared::base16::encode(&server_identity.verifying_key()),
        );
        (server, location)
    }

    /// **The in-band handshake, then messages both ways.** The server proves it holds the key the dial
    /// named, and only then does either side send CapTP traffic.
    #[tokio::test]
    async fn a_websocket_session_authenticates_and_carries_messages_both_ways() {
        let (server, locator) = listening().await;
        let inbound = tokio::spawn(async move {
            let mut conn = server.accept_incoming_connection().await.expect("accept");
            let message = conn.recv().await.expect("recv").expect("a message");
            conn.send(b"pong").await.expect("send");
            message
        });

        let client = WebsocketNetlayer::bind("127.0.0.1:0", identity())
            .await
            .expect("bind the dialing side");
        let mut conn = client
            .new_outgoing_connection(&locator)
            .await
            .expect("the server should prove its identity");
        conn.send(b"ping").await.expect("send");
        assert_eq!(conn.recv().await.expect("recv").expect("a reply"), b"pong");
        assert_eq!(inbound.await.expect("join"), b"ping");
    }

    /// **A peer that cannot prove the key the dial named is refused** — the whole point of the
    /// challenge/response, without which `ws://` is an unauthenticated channel to whoever answers.
    #[tokio::test]
    async fn a_peer_that_cannot_prove_the_key_it_was_dialled_by_is_refused() {
        let (server, mut locator) = listening().await;
        // Name the server by *somebody else's* key: it will sign with its own, and the check fails.
        locator.hints.insert(
            "verify".to_string(),
            rchain_shared::base16::encode(&identity().verifying_key()),
        );

        // The accept no longer runs the handshake (C349); the server has to use the connection for the
        // client's challenge to be answered at all, which is what the client checks against.
        let accepting = tokio::spawn(async move {
            let mut conn = server.accept_incoming_connection().await.expect("accept");
            conn.recv().await
        });
        let client = WebsocketNetlayer::bind("127.0.0.1:0", identity())
            .await
            .expect("bind the dialing side");
        let err = match client.new_outgoing_connection(&locator).await {
            Ok(_) => panic!("a peer that cannot prove the key it was dialled by must be refused"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("did not prove"),
            "the refusal names what failed: {err}"
        );
        let _ = accepting.await;
    }

    /// **A peer that connects and says nothing does not hold the accept loop** (C349).
    ///
    /// The accept is a socket accept and the upgrade runs on first use. A second client therefore
    /// completes a full handshake and message while the first is still silent; before the fix the
    /// silent peer held the loop for `ESTABLISH_TIMEOUT + AUTH_TIMEOUT` (40 s), so this test could not
    /// have got its second accept.
    #[tokio::test]
    async fn a_silent_client_does_not_hold_the_accept() {
        let (server, locator) = listening().await;
        let addr = server.local_addr().expect("the listener's address");
        let server = Arc::new(server);

        // 1. A silent peer: connected, never upgraded, never speaking.
        let silent = tokio::net::TcpStream::connect(addr).await.expect("connect");

        // 2. The accept returns without waiting for it.
        let accepting = Arc::clone(&server);
        let first = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            accepting.accept_incoming_connection(),
        )
        .await
        .expect("the accept must not wait for the peer's upgrade")
        .expect("accept");

        // 3. A well-behaved client completes a handshake and a message while the first is silent.
        let client = tokio::spawn(async move {
            let client = WebsocketNetlayer::bind("127.0.0.1:0", identity())
                .await
                .expect("bind the dialing side");
            let mut conn = client
                .new_outgoing_connection(&locator)
                .await
                .expect("the server proves its identity on first use");
            conn.send(b"ping").await.expect("send");
        });

        let accepting = Arc::clone(&server);
        let mut second = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            accepting.accept_incoming_connection(),
        )
        .await
        .expect("the second accept must not be behind the silent peer")
        .expect("accept");
        assert_eq!(
            second.recv().await.expect("recv").expect("a message"),
            b"ping",
            "the second session's handshake and message must complete"
        );
        client.await.expect("the dialing side completes");

        drop(first);
        drop(silent);
    }

    /// **Only a `init:peer-auth` is signed.** The record wrapper is what keeps the responder from
    /// being an oracle over bytes a peer chooses, so the gate is exercised directly rather than only
    /// through a socket.
    #[test]
    fn only_a_peer_auth_record_is_accepted_as_a_challenge() {
        assert!(is_peer_auth(&peer_auth(&[0u8; CHALLENGE_LEN])));
        assert!(!is_peer_auth(&Value::Symbol(INIT_PEER_AUTH.to_string())));
        // The right label with no payload is not a challenge either.
        assert!(!is_peer_auth(&Value::Record(vec![Value::Symbol(
            INIT_PEER_AUTH.to_string()
        )])));
        assert!(!is_peer_auth(&Value::Record(vec![
            Value::Symbol("desc:export".to_string()),
            Value::Bytes(vec![0u8; 4]),
        ])));
    }

    /// The envelope's signature is read from the field it belongs in, and a value that is not an
    /// envelope is refused rather than guessed at.
    #[test]
    fn the_envelope_yields_its_signature_and_nothing_else_does() {
        // The signature is the gcrypt s-expression the reference reads, not raw bytes — a bytestring
        // here is what the interop run caught this module getting wrong.
        let good = Value::Record(vec![
            Value::Symbol(SIG_ENVELOPE.to_string()),
            peer_auth(&[0u8; CHALLENGE_LEN]),
            crate::session::signature_syrup(&[7u8; 64]).expect("encodable"),
        ]);
        assert_eq!(
            signature_of_envelope(&good).expect("a signature"),
            vec![7u8; 64]
        );
        // And raw bytes are refused rather than accepted as a short signature.
        let raw = Value::Record(vec![
            Value::Symbol(SIG_ENVELOPE.to_string()),
            peer_auth(&[0u8; CHALLENGE_LEN]),
            Value::Bytes(vec![7u8; 64]),
        ]);
        assert!(signature_of_envelope(&raw).is_err());
        assert!(signature_of_envelope(&Value::Symbol(SIG_ENVELOPE.to_string())).is_err());
        assert!(signature_of_envelope(&Value::Int(0.into())).is_err());
    }
}
