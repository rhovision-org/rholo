//! **Two nodes, one session**: node A reaches an object on node B over the authenticated transport.
//!
//! Everything in `ocapn_listener.rs` puts a *test client* on one side of the wire. That proves the
//! listener, and it cannot prove the **dial**: the client there is this repository's own
//! `Session::dial`, driven from the test. Here both ends are nodes — A originates the session through
//! its own admin route (`POST /api/v1/ocapn/dial`, `enable-ocapn-dial`), B accepts it, and the object
//! A comes away holding is one B's bootstrap published. That is the shape issue #249 was opened for
//! and the shape a vat on another node has.
//!
//! **Why A listens as well as dials.** A node registers a transport on its dialer only when it also
//! *listens* on it (`serve_ocapn` builds the `MultiNetlayer` from the configured listeners), so a
//! node with no listener has nothing to dial *with* and its admin route answers 503. A therefore
//! carries its own `noise` listener and identity file: it is a node that happens to dial B, not a
//! bare dialer.
//!
//! **What the second half pins.** The same dial is repeated naming a *different* Ed25519 key than
//! the one B holds. B refuses the SYN at its cleartext-prefix check — before any cryptography — so
//! the route answers 502. That is what makes the first half mean "a Noise session with *this* node"
//! rather than "a socket was opened somewhere".

mod common;

/// The echo fixture's swiss number, as the conformance suite spells it.
const ECHO_SWISS: &[u8] = b"IO58l1laTyhcrgDKbEzFOO32MDd6zE5w";

/// Wait until a node's OCapN port accepts a connection. The listeners bind after the store replay,
/// which is well after `start_pair` returns — the same gap `wait_for_ocapn` covers in
/// `ocapn_listener.rs`. The probe is dropped at once: the listener reads the close as a session that
/// ended, which it is.
async fn wait_for_ocapn(port: u16) {
    for _ in 0..600 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the node's OCapN listener on {port} never came up");
}

/// **One node holds a live reference to an object on another**, over `noise`, with no test client in
/// the middle.
///
/// The object fetched is the echo fixture every bootstrap publishes — the same one `ocapn_listener`'s
/// tests fetch — because what is under test here is the *path between two nodes*, not the object. Both
/// nodes are given a deployer key and funded wallets so either could serve a chain-backed capability;
/// the fetch here is deliberately the offline one, so a failure is the transport's and not a block's.
#[test]
fn a_node_dials_another_node_and_holds_the_object_it_gives_back() {
    let dir_a = common::temp_dir("ocapn-two-a");
    let dir_b = common::temp_dir("ocapn-two-b");
    // One allocation for both nodes, split: `free_ports` *releases* the ports it probed, so two calls
    // can hand out the same number and the second node fails to bind.
    let ports = common::free_ports(12);
    let a_ports = &ports[0..6];
    let b_ports = &ports[6..12];

    let mut conf_b = common::deploy_conf(&dir_b, b_ports);
    conf_b.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    // One transport at a time: B listens on `noise` and nothing else.
    conf_b.api_server.ocapn_listen = None;
    conf_b.api_server.ocapn_listen_noise = Some(format!("127.0.0.1:{}", b_ports[5]));
    let b_key = dir_b.join("noise-identity.key");
    conf_b.api_server.ocapn_identity_key = Some(b_key.display().to_string());

    let mut conf_a = common::deploy_conf(&dir_a, a_ports);
    conf_a.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    conf_a.api_server.ocapn_listen = None;
    conf_a.api_server.ocapn_listen_noise = Some(format!("127.0.0.1:{}", a_ports[5]));
    conf_a.api_server.ocapn_identity_key =
        Some(dir_a.join("noise-identity.key").display().to_string());
    conf_a.api_server.enable_ocapn_dial = true;

    common::test_runtime().block_on(async {
        let (a, b) = common::start_pair(
            (&conf_a, a_ports[2], a_ports[0]),
            (&conf_b, b_ports[2], b_ports[0]),
        )
        .await;
        wait_for_ocapn(a_ports[5]).await;
        wait_for_ocapn(b_ports[5]).await;

        // **B's name is the key its handshake proves**, and it is on disk because B wrote it when its
        // listener bound. Reading it is how any dialler learns what to put in the SYN's prefix — a
        // responder refuses a SYN naming another node.
        let stored = std::fs::read(&b_key).expect("B should write its identity");
        assert_eq!(stored.len(), 64, "an Ed25519 seed then an X25519 static");
        let b_identity = rchain_ocapn::noise::NoiseIdentity::new(
            stored[..32].try_into().expect("32 bytes"),
            stored[32..].try_into().expect("32 bytes"),
        )
        .expect("a valid identity");
        let verify = rchain_shared::base16::encode(&b_identity.verifying_key());

        let client = reqwest::Client::new();
        let dial = format!("http://127.0.0.1:{}/api/v1/ocapn/dial", a_ports[1]);
        let swiss = rchain_shared::base16::encode(ECHO_SWISS);
        // The designator is the label *A* gives B and does not have to be B's name: the key in the
        // `verify` hint is the frame. Naming them differently is deliberate — it is what tells this
        // test apart from one that only ever dials a node whose designator happens to be its key.
        let dial_to = |verify: &str| {
            client.post(&dial).json(&serde_json::json!({
                "designator": "node-b",
                "transport": "noise",
                "hints": {
                    "host": "127.0.0.1",
                    "port": b_ports[5].to_string(),
                    "verify": verify,
                },
                "swiss": swiss.clone(),
            }))
        };

        // 1. A dials B, proves B is who it named, and comes away holding B's object.
        let answered = dial_to(&verify)
            .send()
            .await
            .expect("A's admin server should answer");
        let status = answered.status();
        let body: serde_json::Value = answered.json().await.expect("a JSON body");
        assert_eq!(status, 200, "A should have fetched from B: {body}");
        assert_eq!(body["peer"], "node-b.noise");
        assert!(
            body["fetched"].as_str().unwrap_or("").contains("Export"),
            "the answer names the object B exported: {body}"
        );

        // 2. **And it was B's key that was proved.** The same dial naming a different key is refused
        //    at the prefix check, before any cryptography — so half 1 is a session with *this* node
        //    and not merely a socket that opened.
        //
        //    **Two dials is the whole test**, because the route is rate-limited to two a second and a
        //    third would be answered 429 — a test that made three would be measuring the limiter.
        let other = rchain_shared::base16::encode(&[7u8; 32]);
        let refused = dial_to(&other)
            .send()
            .await
            .expect("A's admin server should answer");
        assert_eq!(
            refused.status(),
            502,
            "a dial naming the wrong responder key must not establish"
        );
        // The *reason* is the transport's, which is the part that pins it: a refusal by the **dial
        // policy** would never have opened a connection, and this one did — A reached B's listener and
        // B dropped the socket at the cleartext prefix, so A's fetcher saw the stream end.
        let reason = refused.text().await.expect("a reason");
        assert!(
            reason.contains("ocapn transport"),
            "the refusal should come from the transport, not from the policy: {reason}"
        );

        a.shutdown().await;
        b.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}
