//! The `noise` netlayer — TCP under a Noise `XX` handshake, and the transport OCapN is converging on.
//!
//! **What makes this different from `tcp_testing_only`.** That transport is plaintext and
//! unauthenticated: any peer that can reach the port is trusted, which is why its own README says
//! "HIGHLY INSECURE — DO NOT USE IN PRODUCTION" and why the listener is off unless configured. This
//! one completes a Noise `XX` handshake before a single CapTP byte moves, so the channel is encrypted
//! and **both** ends are authenticated: each side proves it holds an Ed25519 signing key by signing
//! its own X25519 static public key, and `XX` binds those statics to the session. No certificate
//! authority and no daemon are involved — a node needs a key pair and a listening port.
//!
//! **The parameters are Agoric's, not ours, and that is the entire basis on which this is
//! interoperable.** `XX` with X25519, ChaCha20Poly1305 and **BLAKE2s**, an **empty prologue**, the
//! three-flight handshake — SYN 132 B sent behind a 32-byte **cleartext prefix** naming the intended
//! responder, SYNACK 193 B, ACK 64 B — and, the part that is *application* protocol rather than Noise,
//! a payload of the sender's Ed25519 verifying key followed by its signature over its own X25519
//! static public. All of that is pinned by `rust/ocapn_noise` and `packages/ocapn-noise` in Agoric's
//! endo repository, which this module mirrors step for step, and the same `noise-protocol` and
//! `noise-rust-crypto` crates produce the bytes on both sides, so the two implementations are the same
//! code underneath rather than two that have to be argued equivalent.
//!
//! **A dial therefore has to name the peer's Ed25519 key**, in the locator's `verify` hint (base16):
//! the SYN's prefix *is* that key, and a responder refuses a SYN whose prefix is not its own before
//! doing any cryptography — so the prefix is not decoration, it is the frame, and it is also what lets
//! an intermediary route a handshake it must not be able to read.
//!
//! **Interoperability here is a measured fact, not an argument.** `spec/audit/evidence/ocapn-noise/`
//! drives Agoric's own core, fetched at a pinned commit, against this module: the reference accepts
//! our SYN and our ACK and messages decrypt in both directions. What that run does **not** cover — the
//! record framing, for which the reference ships no counterpart — is written into the transcript
//! rather than left for a reader to assume from a green result.
//!
//! **Framing is this module's own, and it chunks.** A Noise transport message carries at most 65535
//! bytes, well under the CapTP messages this codebase produces, so a message longer than one cipher
//! message is split and each chunk is encrypted separately — the cipher state's nonce advances per
//! chunk, so the chunks are ordered and a re-order or a drop is a decryption failure rather than a
//! silent corruption. [`crate::framed`] is not reused: it frames with netstrings, and its own doc
//! already says a transport with different framing writes its own module.
//!
//! **`peer_address` is reported**, unlike the `unix` transport. This is TCP underneath, so the dial
//! policy *can* judge a peer of this transport and Law 62's origin rule applies to it in full — a
//! remote Noise peer may not aim this node at one of the node's own local addresses. That the
//! channel is encrypted says nothing about where the peer is or what it may reach.

use std::io;
use std::net::SocketAddr;

use async_trait::async_trait;
use noise_protocol::patterns::noise_xx;
use noise_protocol::{CipherState, HandshakeState, U8Array};
use noise_rust_crypto::sensitive::Sensitive;
use noise_rust_crypto::{Blake2s, ChaCha20Poly1305, X25519};
use rchain_crypto::signatures::ed25519::Ed25519;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::framed::CONNECT_TIMEOUT;
use crate::locator::PeerLocator;
use crate::netlayer::{NetConn, Netlayer};

/// The three handshake messages, at the lengths the reference asserts.
const SYN_LEN: usize = 132;
const SYNACK_LEN: usize = 193;
const ACK_LEN: usize = 64;

/// **The SYN goes on the wire prefixed with the intended responder's Ed25519 key.** The reference's
/// binding computes `PREFIXED_SYN_LENGTH = 32 + 132` and sends both: the prefix is what lets an
/// intermediary route a SYN it cannot (and must not) decrypt, and the responder reads it and refuses
/// a SYN meant for someone else *before* doing any cryptography. A bare 132-byte SYN would be read as
/// 32 bytes of key and 100 of message, so this is not a nicety — it is the frame.
const PREFIXED_SYN_LEN: usize = VERIFYING_KEY_LEN + SYN_LEN;

/// The handshake payloads: an Ed25519 verifying key, its signature over the sender's X25519 static
/// public key, and the encoding-negotiation bytes the reference reserves (zeros here — it reads them
/// as `f`/unused, and the lengths are what a peer parses).
const INITIATOR_PAYLOAD_LEN: usize = 100;
const RESPONDER_PAYLOAD_LEN: usize = 97;
const VERIFYING_KEY_LEN: usize = 32;
const SIGNATURE_LEN: usize = 64;

/// The AEAD tag every cipher message carries, and the largest plaintext one may hold: a Noise transport
/// message is capped at 65535 bytes, so this is the chunk size, not the message size.
const TAG_LEN: usize = 16;
const MAX_CHUNK_PLAINTEXT: usize = 65535 - TAG_LEN;

/// The largest whole message this transport will reassemble, matching [`crate::framed`]'s bound.
const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// The largest **ciphertext** body this transport will take — which is not [`MAX_MESSAGE_BYTES`].
///
/// A message of [`MAX_MESSAGE_BYTES`] plaintext is not `MAX_MESSAGE_BYTES + one tag` on the wire: it
/// is `ceil(MAX_MESSAGE_BYTES / MAX_CHUNK_PLAINTEXT)` cipher messages, each carrying its own tag. The
/// received bound used to be `MAX_MESSAGE_BYTES + TAG_LEN`, which is in the wrong unit — it refused
/// a plaintext a little under the documented bound and *accepted* up to a tag's worth of padding past
/// it (HAZOP row A3). This is the plaintext bound plus one tag per chunk.
const MAX_CIPHERTEXT_BYTES: usize =
    MAX_MESSAGE_BYTES + TAG_LEN * (1 + MAX_MESSAGE_BYTES / MAX_CHUNK_PLAINTEXT);

/// How long a Noise handshake may take before the connection is abandoned.
///
/// **The peer chooses how long to take**, and the handshake runs on the connection's first use rather
/// than in the accept loop — so without a bound a peer that connects and then stays silent would hold
/// a session slot for ever. Thirty seconds is the bound the CapTP handshake already carries
/// (`ocapn/src/conn.rs`), and a real handshake is a few round trips on any usable link.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

type NoiseHandshake = HandshakeState<X25519, ChaCha20Poly1305, Blake2s>;
type NoiseCipher = CipherState<ChaCha20Poly1305>;

/// A node's Noise identity: the two key pairs the handshake needs, and nothing else.
///
/// Both are **static** — they are the node's identity across sessions and across restarts — which is
/// what distinguishes them from the ephemeral X25519 key the handshake generates per session and from
/// CapTP's own per-session Ed25519 key. The Ed25519 half is what a peer ends up able to name; the
/// X25519 half is what the session's keys are derived from.
#[derive(Clone)]
pub struct NoiseIdentity {
    /// The Ed25519 signing seed (32 bytes).
    signing: [u8; 32],
    /// Its verifying key, which the handshake carries and signs over the X25519 static.
    verifying: [u8; 32],
    /// The X25519 static secret.
    static_secret: [u8; 32],
    /// Its public half — what the signature in the payload covers.
    static_public: [u8; 32],
}

impl NoiseIdentity {
    /// Build an identity from an Ed25519 signing seed and an X25519 static secret.
    ///
    /// The verifying key is **derived rather than taken**, so it cannot disagree with the seed. Both
    /// conversions are total for a 32-byte seed, but they return `Result`s and so are propagated
    /// rather than defaulted: a silent `unwrap_or` here would mint an identity whose *name* is
    /// thirty-two zero bytes, which is a peer that cannot be told apart from any other node that
    /// made the same mistake.
    pub fn new(signing: [u8; 32], static_secret: [u8; 32]) -> Result<NoiseIdentity, String> {
        let verifying: [u8; VERIFYING_KEY_LEN] = Ed25519::to_public_bytes(&signing)
            .map_err(|e| format!("a 32-byte Ed25519 seed must have a public key: {e}"))?
            .try_into()
            .map_err(|_| "an Ed25519 public key is 32 bytes".to_string())?;
        Ok(NoiseIdentity {
            signing,
            verifying,
            static_secret,
            static_public: rchain_crypto::encryption::x25519::public_from_secret(&static_secret),
        })
    }

    /// Draw a fresh identity. A deployment persists these; a test does not need to.
    pub fn generate() -> Result<NoiseIdentity, String> {
        let mut signing = [0u8; 32];
        let mut stat = [0u8; 32];
        rand::Rng::fill_bytes(&mut rand::rng(), &mut signing);
        rand::Rng::fill_bytes(&mut rand::rng(), &mut stat);
        NoiseIdentity::new(signing, stat)
    }

    /// The Ed25519 verifying key — the half a peer names this node by.
    pub fn verifying_key(&self) -> [u8; 32] {
        self.verifying
    }

    /// The X25519 static public key.
    pub fn static_public(&self) -> [u8; 32] {
        self.static_public
    }

    /// Sign `bytes` with the node's Ed25519 key.
    ///
    /// **This is the node's identity, not this transport's.** The Noise handshake names the node by
    /// it, and `websocket`'s in-band challenge/response authenticates with the same key — one identity,
    /// two transports, so a peer that knows the node can reach it either way. The X25519 half is
    /// Noise's alone; this half is what a peer can hold in advance.
    pub fn sign(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        Ed25519::sign_bytes(bytes, &self.signing).map_err(|e| e.to_string())
    }

    /// The bytes this identity is persisted as: the Ed25519 seed, then the X25519 static — 64 bytes,
    /// stored mode `0600` by whoever writes them. Both halves travel together because [`Self::new`]
    /// re-derives everything else, so the file cannot disagree with itself.
    pub fn to_persisted_bytes(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(&self.signing);
        out[32..].copy_from_slice(&self.static_secret);
        out
    }

    /// The payload this side puts in its handshake message: its verifying key, its signature over its
    /// own X25519 static public, and the reserved encoding bytes.
    fn payload(&self, length: usize) -> io::Result<Vec<u8>> {
        let mut payload = vec![0u8; length];
        payload[..VERIFYING_KEY_LEN].copy_from_slice(&self.verifying);
        let signature = Ed25519::sign_bytes(&self.static_public, &self.signing)
            .map_err(|e| io::Error::other(format!("signing the static key: {e}")))?;
        if signature.len() != SIGNATURE_LEN {
            return Err(io::Error::other(
                "Ed25519 produced a signature of the wrong length",
            ));
        }
        payload[VERIFYING_KEY_LEN..VERIFYING_KEY_LEN + SIGNATURE_LEN].copy_from_slice(&signature);
        Ok(payload)
    }
}

/// Read a verifying key and a signature out of a handshake payload, and check the signature covers
/// `static_public` — the property that ties an Ed25519 name to an X25519 key.
fn check_payload(payload: &[u8], static_public: &[u8; 32], who: &str) -> io::Result<[u8; 32]> {
    if payload.len() < VERIFYING_KEY_LEN + SIGNATURE_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the {who} handshake payload is too short to carry a key and a signature"),
        ));
    }
    let verifying: [u8; 32] = payload[..VERIFYING_KEY_LEN]
        .try_into()
        .map_err(|_| io::Error::other("verifying key"))?;
    let signature = &payload[VERIFYING_KEY_LEN..VERIFYING_KEY_LEN + SIGNATURE_LEN];
    if !Ed25519::verify_bytes(static_public, signature, &verifying) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the {who} did not prove it holds the signing key for the static key it offered"
            ),
        ));
    }
    Ok(verifying)
}

/// A `noise` endpoint. The same value dials (as an initiator) and accepts (as a responder); dialling
/// needs no listener, so a purely outgoing peer still binds one on an ephemeral port.
pub struct NoiseNetlayer {
    listener: TcpListener,
    identity: NoiseIdentity,
}

impl NoiseNetlayer {
    /// Bind a listening socket. `addr` is `host:port`; `127.0.0.1:0` asks the OS for a free port.
    pub async fn bind(addr: &str, identity: NoiseIdentity) -> io::Result<NoiseNetlayer> {
        Ok(NoiseNetlayer {
            listener: TcpListener::bind(addr).await?,
            identity,
        })
    }

    /// The bound address, including the port chosen for `:0`.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// The identity this endpoint presents.
    pub fn identity(&self) -> &NoiseIdentity {
        &self.identity
    }
}

#[async_trait]
impl Netlayer for NoiseNetlayer {
    async fn new_outgoing_connection(&self, locator: &PeerLocator) -> io::Result<Box<dyn NetConn>> {
        let host = locator.hints.get("host").ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "noise needs a `host` hint")
        })?;
        let port: u16 = locator
            .hints
            .get("port")
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "noise needs a `port` hint")
            })?
            .parse()
            .map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "noise `port` is not a number")
            })?;
        let stream =
            tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host.as_str(), port)))
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("connecting to {host}:{port} took longer than {CONNECT_TIMEOUT:?}"),
                    )
                })??;
        // **The peer's Ed25519 verifying key, which the SYN must be prefixed with.** The reference's
        // binding takes it as an argument to its write-SYN call, and the responder refuses a SYN whose
        // prefix is not its own key — so a dial that cannot name the peer cannot complete a handshake.
        let peer_verifying = locator.hints.get("verify").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "noise needs a `verify` hint: the peer's Ed25519 verifying key, base16",
            )
        })?;
        let peer_verifying: [u8; VERIFYING_KEY_LEN] = rchain_shared::base16::decode(peer_verifying)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "noise `verify` is not 32 bytes of base16",
                )
            })?;
        Ok(Box::new(NoiseConn::dialing(
            stream,
            self.identity.clone(),
            peer_verifying,
        )))
    }

    /// **The handshake does not happen here.** It runs on the connection's first use — see
    /// [`NoiseConn`] — because an accept that completed a handshake would put a peer's *timing* into
    /// the node's accept loop: one peer that connects and then stays silent would hold every other
    /// peer out while it did, and a peer that connects and drops would fail this call outright and end
    /// the listener. Here the accept is a socket accept and nothing else, so a bad peer costs one
    /// session slot, bounded by [`HANDSHAKE_TIMEOUT`] — the same price it costs on every other
    /// transport.
    async fn accept_incoming_connection(&self) -> io::Result<Box<dyn NetConn>> {
        let (stream, _peer) = self.listener.accept().await?;
        Ok(Box::new(NoiseConn::accepted(stream, self.identity.clone())))
    }
}

/// The initiator's half of the handshake: prefixed SYN, then read SYNACK, verify it, then ACK.
/// Returns the transport ciphers as `(send, recv)`, which is the order the initiator gets them in.
async fn initiate(
    stream: &mut TcpStream,
    identity: &NoiseIdentity,
    peer_verifying: [u8; VERIFYING_KEY_LEN],
) -> io::Result<(NoiseCipher, NoiseCipher, [u8; VERIFYING_KEY_LEN])> {
    let mut handshake = NoiseHandshake::new(
        noise_xx(),
        true,   // initiator
        vec![], // empty prologue
        // `Sensitive` is the crate's own zeroing key wrapper; the static secret is put into it rather
        // than kept beside it, so the working copy the handshake holds is zeroed on drop too.
        Some(Sensitive::from_slice(&identity.static_secret)),
        None, // ephemeral key generated for us
        None, // the responder's static key is learned in the handshake
        None, // no remote ephemeral
    );

    let mut syn = vec![0u8; SYN_LEN];
    handshake
        .write_message(&identity.payload(INITIATOR_PAYLOAD_LEN)?, &mut syn)
        .map_err(|_| io::Error::other("writing the Noise SYN"))?;
    // The frame: the intended responder's key in the clear, then the SYN. See `PREFIXED_SYN_LEN`.
    let mut prefixed = Vec::with_capacity(PREFIXED_SYN_LEN);
    prefixed.extend_from_slice(&peer_verifying);
    prefixed.extend_from_slice(&syn);
    stream.write_all(&prefixed).await?;
    stream.flush().await?;

    let mut synack = vec![0u8; SYNACK_LEN];
    stream.read_exact(&mut synack).await?;
    let mut payload = vec![0u8; RESPONDER_PAYLOAD_LEN];
    handshake.read_message(&synack, &mut payload).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the Noise SYNACK did not verify",
        )
    })?;
    let responder_static = handshake.get_rs().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the SYNACK carried no static key",
        )
    })?;
    // The responder proves it holds the signing key for the static key it just offered. **Kept, not
    // discarded** (HAZOP row C242): it is the name this session can be held to.
    let responder_verifying = check_payload(&payload, &responder_static, "responder")?;

    let mut ack = vec![0u8; ACK_LEN];
    handshake
        .write_message(&[], &mut ack)
        .map_err(|_| io::Error::other("writing the Noise ACK"))?;
    stream.write_all(&ack).await?;
    stream.flush().await?;

    let (send, recv) = handshake.get_ciphers();
    Ok((send, recv, responder_verifying))
}

/// The responder's half: read SYN, answer SYNACK, then read and verify the ACK.
async fn respond(
    stream: &mut TcpStream,
    identity: &NoiseIdentity,
) -> io::Result<(NoiseCipher, NoiseCipher, [u8; VERIFYING_KEY_LEN])> {
    let mut prefixed = vec![0u8; PREFIXED_SYN_LEN];
    stream.read_exact(&mut prefixed).await?;
    let (intended, syn) = prefixed.split_at(VERIFYING_KEY_LEN);
    // **Refused before any cryptography**, as the reference does: a SYN that names another node is not
    // this node's to answer, and answering it would be answering on someone else's behalf.
    if intended != identity.verifying.as_slice() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the SYN named a different node as its intended responder",
        ));
    }

    let mut handshake = NoiseHandshake::new(
        noise_xx(),
        false,  // responder
        vec![], // empty prologue
        // `Sensitive` is the crate's own zeroing key wrapper; the static secret is put into it rather
        // than kept beside it, so the working copy the handshake holds is zeroed on drop too.
        Some(Sensitive::from_slice(&identity.static_secret)),
        None,
        None,
        None,
    );

    let mut initiator_payload = vec![0u8; INITIATOR_PAYLOAD_LEN];
    handshake
        .read_message(&syn, &mut initiator_payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the Noise SYN did not verify"))?;

    let mut synack = vec![0u8; SYNACK_LEN];
    handshake
        .write_message(&identity.payload(RESPONDER_PAYLOAD_LEN)?, &mut synack)
        .map_err(|_| io::Error::other("writing the Noise SYNACK"))?;
    stream.write_all(&synack).await?;
    stream.flush().await?;

    let mut ack = vec![0u8; ACK_LEN];
    stream.read_exact(&mut ack).await?;
    handshake
        .read_message(&ack, &mut [])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the Noise ACK did not verify"))?;

    let initiator_static = handshake.get_rs().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "the ACK carried no static key")
    })?;
    // The initiator proves it holds the signing key for the static key it offered in the SYN.
    // **Kept, not discarded** (HAZOP row C242) — and for an *accepted* session this is the only name
    // that was proved, since the peer's `op:start-session` designator is asserted and public.
    let initiator_verifying = check_payload(&initiator_payload, &initiator_static, "initiator")?;

    // The responder's cipher order is the mirror of the initiator's: what it receives with is the
    // first the handshake yields, and what it sends with is the second.
    let (recv, send) = handshake.get_ciphers();
    Ok((send, recv, initiator_verifying))
}

/// Which half of the handshake this connection is due to run.
enum Half {
    /// We dialled. The peer's Ed25519 key is what the SYN's prefix must name.
    Initiator {
        peer_verifying: [u8; VERIFYING_KEY_LEN],
    },
    /// We accepted. Our own key is what the prefix must name.
    Responder,
}

/// One encrypted connection. Messages are chunked: a `u32` length, then that many cipher-message
/// bytes, each chunk at most [`MAX_CHUNK_PLAINTEXT`] of plaintext.
///
/// **The handshake runs on the first `send` or `recv`, not when the connection is made.** That is not
/// laziness: an accept is called from the node's accept loop, so a handshake completed there would let
/// one peer's *timing* — connect-and-stay-silent, or connect-and-drop — hold that loop and then fail
/// it. Here the loop only ever accepts sockets, and a peer that will not finish a handshake costs one
/// session slot until [`HANDSHAKE_TIMEOUT`], which is what a bad peer costs on every other transport.
struct NoiseConn {
    stream: TcpStream,
    identity: NoiseIdentity,
    /// `Some` until the handshake runs and `None` after, so a connection handshakes at most once and a
    /// second attempt is not representable.
    pending: Option<Half>,
    /// `(send, recv)` once the handshake has completed.
    ciphers: Option<(NoiseCipher, NoiseCipher)>,
    /// **The peer's Ed25519 key, which the handshake verified** — the whole point of the payload
    /// signature. It used to be bound to `_` and dropped (HAZOP row C242), which left the session
    /// named by the peer's own assertion; it is kept now so the name can be the one that was proved.
    verified: Option<[u8; VERIFYING_KEY_LEN]>,
}

impl NoiseConn {
    /// A connection we dialled; the first use runs its handshake.
    fn dialing(
        stream: TcpStream,
        identity: NoiseIdentity,
        peer_verifying: [u8; VERIFYING_KEY_LEN],
    ) -> NoiseConn {
        NoiseConn {
            stream,
            identity,
            pending: Some(Half::Initiator { peer_verifying }),
            ciphers: None,
            verified: None,
        }
    }

    /// A connection we accepted; the first use runs its handshake.
    fn accepted(stream: TcpStream, identity: NoiseIdentity) -> NoiseConn {
        NoiseConn {
            stream,
            identity,
            pending: Some(Half::Responder),
            ciphers: None,
            verified: None,
        }
    }

    /// Run the handshake if it has not run. **Bounded**, because the peer chooses how long to take.
    async fn ensure_ready(&mut self) -> io::Result<()> {
        let Some(half) = self.pending.take() else {
            return Ok(());
        };
        let finished = {
            let NoiseConn {
                stream, identity, ..
            } = self;
            tokio::time::timeout(HANDSHAKE_TIMEOUT, async move {
                match half {
                    Half::Initiator { peer_verifying } => {
                        initiate(stream, identity, peer_verifying).await
                    }
                    Half::Responder => respond(stream, identity).await,
                }
            })
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("the Noise handshake did not finish within {HANDSHAKE_TIMEOUT:?}"),
                )
            })?
        };
        let (send, recv, verified) = finished?;
        self.verified = Some(verified);
        self.ciphers = Some((send, recv));
        Ok(())
    }

    /// Encrypt `message` into as many chunks as it needs, each a separate cipher message.
    fn seal(&mut self, message: &[u8]) -> io::Result<Vec<u8>> {
        let (send, _) = self
            .ciphers
            .as_mut()
            .ok_or_else(|| io::Error::other("the Noise handshake has not run"))?;
        let mut out = Vec::with_capacity(message.len() + TAG_LEN);
        for chunk in message.chunks(MAX_CHUNK_PLAINTEXT) {
            let mut buf = vec![0u8; chunk.len() + TAG_LEN];
            buf[..chunk.len()].copy_from_slice(chunk);
            send.encrypt_in_place(&mut buf, chunk.len());
            out.extend_from_slice(&buf);
        }
        Ok(out)
    }

    /// Decrypt `ciphertext` — the whole body of one framed message — into its plaintext.
    fn open(&mut self, mut ciphertext: Vec<u8>) -> io::Result<Vec<u8>> {
        let (_, recv) = self
            .ciphers
            .as_mut()
            .ok_or_else(|| io::Error::other("the Noise handshake has not run"))?;
        // **Every chunk has to be at least one tag, and a body shorter than one is refused here.**
        // `CipherState::decrypt_in_place` *asserts* `ciphertext_len >= 16` rather than returning an
        // error, so handing it a shorter chunk does not fail the message — it panics the task that
        // owns the session. A peer that completed the handshake and then framed a five-byte body did
        // exactly that, which is what the HAZOP's protocol lens measured; the length is checked
        // before the call rather than caught after it.
        let frames_a_message = |n: usize| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("a Noise transport message of {n} bytes is shorter than one AEAD tag"),
            )
        };
        if ciphertext.len() < TAG_LEN {
            return Err(frames_a_message(ciphertext.len()));
        }
        let mut plain = Vec::with_capacity(ciphertext.len());
        // Chunks are `MAX_CHUNK_PLAINTEXT + TAG_LEN` bytes each, except the last — and the last is
        // where a peer can aim the remainder, so it is checked too.
        //
        // **Consumed by index, not by draining.** `ciphertext.drain(..take)` shifts every byte after
        // the chunk down on *each* iteration, which is quadratic in the chunk count: a body at the
        // 4 MiB cap is 64 chunks, so ~128 MiB of `memmove` for one message (HAZOP row C233). Walking
        // an offset is the same work without the shifting, and `decrypt_in_place` writes in place at
        // the offset it is given.
        let mut offset = 0;
        while offset < ciphertext.len() {
            let take = (ciphertext.len() - offset).min(MAX_CHUNK_PLAINTEXT + TAG_LEN);
            if take < TAG_LEN {
                return Err(frames_a_message(take));
            }
            let written = recv
                .decrypt_in_place(&mut ciphertext[offset..offset + take], take)
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "a Noise transport message did not authenticate",
                    )
                })?;
            plain.extend_from_slice(&ciphertext[offset..offset + written]);
            offset += take;
        }
        Ok(plain)
    }
}

#[async_trait]
impl NetConn for NoiseConn {
    async fn send(&mut self, message: &[u8]) -> io::Result<()> {
        self.ensure_ready().await?;
        // **The send is bounded too.** It was not: only `sealed.len()` was checked, against `u32`, so
        // this transport would encrypt and emit a message up to 4 GiB — one its own receive bound, and
        // every other transport's, refuses. Both directions are bounded at the same number now.
        if message.len() > MAX_MESSAGE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a Noise netlayer message exceeds the transport's bound",
            ));
        }
        let sealed = self.seal(message)?;
        // The length is the *ciphertext's*, because that is what the peer must read before it can
        // decrypt; a peer that lies about it is bounded by the check in `recv`, not by a buffer.
        let len = u32::try_from(sealed.len())
            .map_err(|_| io::Error::other("a Noise message longer than 4 GiB is refused"))?;
        self.stream.write_all(&len.to_be_bytes()).await?;
        self.stream.write_all(&sealed).await?;
        self.stream.flush().await
    }

    fn peer_address(&self) -> Option<SocketAddr> {
        // **Reported, unlike `unix`**: this is TCP underneath, so the dial policy can judge the peer
        // and Law 62's origin rule applies. See the module doc.
        self.stream.peer_addr().ok()
    }

    fn verified_peer(&self) -> Option<[u8; VERIFYING_KEY_LEN]> {
        // **This transport proves a name and is the only one that does.** The handshake's payload is
        // an Ed25519 key plus that key's signature over the sender's X25519 static, so the key is
        // bound to the session — unlike the peer's `op:start-session` designator, which is public and
        // signed by nothing but the peer's own ephemeral session key (HAZOP rows C242/C243).
        self.verified
    }

    async fn recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        self.ensure_ready().await?;
        // **A clean boundary is not a short read** (HAZOP row C228). `read_exact` reports
        // `UnexpectedEof` for both, so reading the header with it would take a peer that closes two
        // bytes into a four-byte length and call it "the peer said goodbye" — the message is lost and
        // the session ends as though the peer had meant to. `framed` pins the same rule for
        // netstrings: `Ok(None)` only when the stream ends *at* a boundary, `UnexpectedEof` when it
        // ends inside one.
        let mut header = [0u8; 4];
        let mut filled = 0;
        while filled < header.len() {
            match self.stream.read(&mut header[filled..]).await {
                Ok(0) if filled == 0 => return Ok(None),
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the peer closed inside a Noise transport length header",
                    ))
                }
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        let len = u32::from_be_bytes(header) as usize;
        if len > MAX_CIPHERTEXT_BYTES {
            // Refused **before** buffering, the rule `framed` pins for netstrings.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a Noise netlayer message exceeds the transport's bound",
            ));
        }
        let mut body = vec![0u8; len];
        self.stream.read_exact(&mut body).await?;
        Ok(Some(self.open(body)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A locator for `addr` naming the peer by its Ed25519 verifying key — which the dialler must
    /// know, because the SYN is prefixed with it.
    fn locator(addr: SocketAddr, verifying: [u8; VERIFYING_KEY_LEN]) -> PeerLocator {
        PeerLocator {
            designator: "peer".into(),
            transport: "noise".into(),
            hints: BTreeMap::from([
                ("host".to_string(), addr.ip().to_string()),
                ("port".to_string(), addr.port().to_string()),
                (
                    "verify".to_string(),
                    rchain_shared::base16::encode(&verifying),
                ),
            ]),
        }
    }

    async fn dialer() -> NoiseNetlayer {
        NoiseNetlayer::bind(
            "127.0.0.1:0",
            NoiseIdentity::generate().expect("a fresh Noise identity"),
        )
        .await
        .expect("bind the dialing side")
    }

    /// **A full handshake, then a message each way** — the transport's own two-sided test, with no
    /// reference involved. What it proves is that the state machine and the framing agree with each
    /// other; what it cannot prove is that they agree with Agoric's, which the interop harness does.
    #[tokio::test]
    async fn a_noise_session_handshakes_and_carries_messages_both_ways() {
        let server_identity = NoiseIdentity::generate().expect("a fresh Noise identity");
        // The dialler has to know the responder's Ed25519 key — the SYN is prefixed with it.
        let peer_key = server_identity.verifying_key();
        let server = NoiseNetlayer::bind("127.0.0.1:0", server_identity)
            .await
            .expect("bind");
        let addr = server.local_addr().expect("addr");

        let inbound = tokio::spawn(async move {
            let mut conn = server.accept_incoming_connection().await.expect("accept");
            let message = conn.recv().await.expect("recv").expect("a message");
            conn.send(b"pong").await.expect("send");
            message
        });

        let mut client = dialer()
            .await
            .new_outgoing_connection(&locator(addr, peer_key))
            .await
            .expect("dial");
        client.send(b"ping").await.expect("send");
        let reply = client.recv().await.expect("recv").expect("a reply");
        assert_eq!(reply, b"pong");
        assert_eq!(inbound.await.expect("join"), b"ping");
    }

    /// **A message larger than one cipher message is chunked and reassembled.** The Noise transport
    /// caps a message at 65535 bytes; a CapTP message is not bounded by that, so this crosses the
    /// chunk boundary in both directions.
    #[tokio::test]
    async fn a_message_larger_than_one_cipher_message_crosses_intact() {
        let server_identity = NoiseIdentity::generate().expect("a fresh Noise identity");
        // The dialler has to know the responder's Ed25519 key — the SYN is prefixed with it.
        let peer_key = server_identity.verifying_key();
        let server = NoiseNetlayer::bind("127.0.0.1:0", server_identity)
            .await
            .expect("bind");
        let addr = server.local_addr().expect("addr");
        let big: Vec<u8> = (0..(MAX_CHUNK_PLAINTEXT * 2 + 777))
            .map(|i| (i % 251) as u8)
            .collect();
        let expected = big.clone();

        let inbound = tokio::spawn(async move {
            let mut conn = server.accept_incoming_connection().await.expect("accept");
            let received = conn.recv().await.expect("recv").expect("a message");
            conn.send(&received).await.expect("echo");
            received
        });

        let mut client = dialer()
            .await
            .new_outgoing_connection(&locator(addr, peer_key))
            .await
            .expect("dial");
        client.send(&big).await.expect("send");
        let echoed = client.recv().await.expect("recv").expect("an echo");
        assert_eq!(echoed, expected);
        assert_eq!(inbound.await.expect("join"), expected);
    }

    /// **A peer that does not hold the signing key for the static key it offers is refused.** The
    /// responder is shown a SYN whose payload names a verifying key that did not sign the static key
    /// the handshake bound — the substitution the signature exists to stop.
    #[tokio::test]
    async fn a_static_key_without_its_signing_key_is_refused() {
        let signer = NoiseIdentity::generate().expect("a fresh identity");
        let other = NoiseIdentity::generate().expect("a fresh identity");
        // A payload that names `signer`'s name but carries `other`'s signature over `other`'s key.
        let mut payload = vec![0u8; INITIATOR_PAYLOAD_LEN];
        payload[..VERIFYING_KEY_LEN].copy_from_slice(&signer.verifying_key());
        let mis_signed = Ed25519::sign_bytes(&other.static_public(), &other.signing)
            .expect("sign with a key this test holds");
        payload[VERIFYING_KEY_LEN..VERIFYING_KEY_LEN + SIGNATURE_LEN].copy_from_slice(&mis_signed);

        let err = check_payload(&payload, &signer.static_public(), "initiator")
            .expect_err("a signature over the wrong key must not verify");
        assert!(
            err.to_string().contains("did not prove"),
            "the refusal names what failed: {err}"
        );
    }

    /// **A body shorter than one AEAD tag is refused, not panicked on.**
    ///
    /// `CipherState::decrypt_in_place` *asserts* `ciphertext_len >= 16` rather than returning an
    /// error, so a peer that completed the handshake and then framed five bytes did not fail the
    /// message — it **panicked the task that owned the session**. This drives the real path: a
    /// handshake over a real socket, then a length header of 5 and five bytes written raw. Before the
    /// guard in `open` this test aborted the task (HAZOP row A1, which the protocol lens measured).
    #[tokio::test]
    async fn a_body_shorter_than_a_tag_is_refused_rather_than_panicking() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server_identity = NoiseIdentity::generate().expect("a fresh identity");
        let client_identity = NoiseIdentity::generate().expect("a fresh identity");
        let peer_key = server_identity.verifying_key();

        let accepting = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            let mut conn = NoiseConn::accepted(tcp, server_identity);
            conn.ensure_ready().await.expect("the server handshake");
            conn
        });
        let mut client = NoiseConn::dialing(
            TcpStream::connect(addr).await.expect("connect"),
            client_identity,
            peer_key,
        );
        client.ensure_ready().await.expect("the client handshake");
        let mut server = accepting.await.expect("join");

        // The attack: a five-byte body, framed as though it were a message. It is written raw
        // because the point is to send something the transport's own `send` would never produce.
        client
            .stream
            .write_all(&5u32.to_be_bytes())
            .await
            .expect("the length header");
        client.stream.write_all(&[0u8; 5]).await.expect("the body");
        client.stream.flush().await.expect("flush");

        let refused = server.recv().await;
        assert!(
            refused.is_err(),
            "a body under one AEAD tag must be refused, not accepted"
        );
        assert!(
            refused
                .expect_err("checked")
                .to_string()
                .contains("shorter than one AEAD tag"),
            "and the refusal says which rule it broke"
        );
    }

    /// **A stream that ends inside a length header is not a clean end of stream** (HAZOP row C228).
    ///
    /// The header used to be read with `read_exact`, which reports `UnexpectedEof` both when the peer
    /// closes *at* a boundary and when it closes two bytes into a four-byte length — and the match
    /// arm turned both into `Ok(None)`, "the peer said goodbye". So a truncated header silently
    /// dropped a message and ended the session as though nothing were wrong. `framed` distinguishes
    /// the two for netstrings; this is the same rule for this transport's own framing.
    #[tokio::test]
    async fn a_stream_that_ends_inside_a_length_header_is_not_a_clean_end() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server_identity = NoiseIdentity::generate().expect("a fresh identity");
        let client_identity = NoiseIdentity::generate().expect("a fresh identity");
        let peer_key = server_identity.verifying_key();

        let accepting = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            let mut conn = NoiseConn::accepted(tcp, server_identity);
            conn.ensure_ready().await.expect("the server handshake");
            conn
        });
        let mut client = NoiseConn::dialing(
            TcpStream::connect(addr).await.expect("connect"),
            client_identity,
            peer_key,
        );
        client.ensure_ready().await.expect("the client handshake");
        let mut server = accepting.await.expect("join");

        // Two bytes of a four-byte length, then the peer goes away.
        client.stream.write_all(&[0u8, 0u8]).await.expect("write");
        client.stream.flush().await.expect("flush");
        drop(client);

        let ended = server.recv().await;
        let error = ended.expect_err("a partial header is an error, not a clean end of stream");
        assert_eq!(
            error.kind(),
            io::ErrorKind::UnexpectedEof,
            "and it says the stream ended inside a message: {error}"
        );
    }

    /// **A clean close at a message boundary *is* a clean end of stream** — the other half of the
    /// rule above, and the reason the distinction has to be made at all: the conformance peers and
    /// every ordinary peer close this way, and reporting an error for it would turn a normal goodbye
    /// into a protocol failure.
    #[tokio::test]
    async fn a_close_at_a_message_boundary_is_a_clean_end() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server_identity = NoiseIdentity::generate().expect("a fresh identity");
        let client_identity = NoiseIdentity::generate().expect("a fresh identity");
        let peer_key = server_identity.verifying_key();

        let accepting = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            let mut conn = NoiseConn::accepted(tcp, server_identity);
            conn.ensure_ready().await.expect("the server handshake");
            conn
        });
        let mut client = NoiseConn::dialing(
            TcpStream::connect(addr).await.expect("connect"),
            client_identity,
            peer_key,
        );
        client.ensure_ready().await.expect("the client handshake");
        let mut server = accepting.await.expect("join");

        // Away with nothing in flight: no header, no body.
        drop(client);
        assert!(
            server
                .recv()
                .await
                .expect("a close at a boundary is not an error")
                .is_none(),
            "and it reads as the end of the stream"
        );
    }
}
