//! The node's OCapN listener and its bridge (issue #249): a foreign peer dials a running node.
//!
//! Two gates, because they fail independently. The first is the listener's own: a node configured
//! with `api-server.ocapn-listen` answers a CapTP handshake and serves its fixtures. The second is
//! the bridge: a delivery to a chain-backed capability becomes a signed deploy, and that deploy
//! lands in a block, and the value the deployed contract wrote to the deploy's reply channel comes
//! back as the CapTP promise's fulfilment — the balance it reads is the wallet's, less the phlo that
//! very deploy spent.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use rchain_casper::protocol::client::{GrpcProposeService, ProposeService};
use rchain_node::api::ocapn::{ERTP_SWISS, REV_VAULT_BALANCE_SWISS};
use rchain_ocapn::bootstrap::Bootstrap;
use rchain_ocapn::captp::{Deliver, Desc};
use rchain_ocapn::conn::{Identity, Session};
use rchain_ocapn::locator::PeerLocator;
use rchain_ocapn::netlayer::Netlayer;
use rchain_ocapn::noise::NoiseNetlayer;
use rchain_ocapn::syrup::Value;
use rchain_ocapn::tcp_testing_only::TcpTestingOnly;
use rchain_ocapn::unix::UnixNetlayer;
use rchain_ocapn::websocket::WebsocketNetlayer;

/// The echo fixture's swiss number, as the conformance suite spells it.
const ECHO_SWISS: &[u8] = b"IO58l1laTyhcrgDKbEzFOO32MDd6zE5w";

/// Wait until the node's OCapN port accepts a connection. `start` returns as soon as the program is
/// spawned, and the listeners bind after the store replay — the same gap `wait_for_genesis` covers
/// for the HTTP API. The probe connection is dropped at once; the listener treats the close as a
/// session that ended, which it is.
async fn wait_for_ocapn(port: u16) {
    for _ in 0..300 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the OCapN listener on 127.0.0.1:{port} never came up");
}

/// Wait until the node is out of read-only mode, i.e. genesis has been created and announced. The
/// OCapN port binds at "replay complete — starting listeners", which is *before* that, so a propose
/// driven straight after `wait_for_ocapn` is answered `ReadOnlyMode`. `deploy_block.rs` waits the
/// same way, over the HTTP API.
async fn wait_for_genesis(base: &str) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Ok(resp) = client.get(format!("{base}/api/blocks")).send().await {
            if resp.status().is_success() {
                if let Ok(serde_json::Value::Array(blocks)) = resp.json::<serde_json::Value>().await
                {
                    if !blocks.is_empty() {
                        return;
                    }
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "genesis never appeared: {base}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn locator(port: u16) -> PeerLocator {
    PeerLocator {
        designator: "rnode".to_string(),
        transport: "tcp-testing-only".to_string(),
        hints: BTreeMap::from([
            ("host".to_string(), "127.0.0.1".to_string()),
            ("port".to_string(), port.to_string()),
        ]),
    }
}

/// A `unix` locator: the transport name and the socket path, and no host at all — which the dial
/// policy reads as "cannot be judged", the right answer for a peer the filesystem admitted.
fn unix_locator(path: &std::path::Path) -> PeerLocator {
    PeerLocator {
        designator: "rnode".to_string(),
        transport: "unix".to_string(),
        hints: BTreeMap::from([("path".to_string(), path.display().to_string())]),
    }
}

/// Wait until the node's `unix` OCapN socket accepts a connection. There is no port to poll — the
/// socket file is the bind, and a successful connect is the proof it is listening.
async fn wait_for_ocapn_unix(path: &std::path::Path) {
    for _ in 0..300 {
        if tokio::net::UnixStream::connect(path).await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!(
        "the OCapN unix listener at {} never came up",
        path.display()
    );
}

#[test]
fn a_peer_dials_the_node_and_fetches_a_fixture() {
    let dir = common::temp_dir("ocapn-listener");
    // [http, admin-http, grpc-internal, protocol, grpc-external, ocapn]
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.api_server.ocapn_listen = Some(format!("127.0.0.1:{}", ports[5]));

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;

        // Dial the node the way any OCapN peer would: a fresh session key, our own empty bootstrap.
        let dialer = TcpTestingOnly::bind("127.0.0.1:0")
            .await
            .expect("bind the dialing side");
        let connection = dialer
            .new_outgoing_connection(&locator(ports[5]))
            .await
            .expect("dial the node");
        let identity = Identity::fresh(locator(0)).expect("a session key");
        let mut client = Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
            .await
            .expect("the node should complete the handshake");

        // Fetch the echo fixture from the node's bootstrap (its export 0), asking for the reply at
        // our own export 0 — the shape the conformance suite's `fetch_object` uses.
        let fetch = Deliver {
            to: Desc::Export(0u64.into()),
            args: vec![
                Value::Symbol("fetch".into()),
                Value::Bytes(ECHO_SWISS.to_vec()),
            ],
            answer_pos: None,
            resolve_me_desc: Some(Desc::ImportObject(0u64.into())),
        };
        client
            .send_message(&fetch.to_syrup())
            .await
            .expect("send the fetch");

        let reply = client
            .recv_message()
            .await
            .expect("read the reply")
            .expect("a reply, not a closed connection");
        let delivered = Deliver::from_syrup(&reply).expect("a delivery");
        assert_eq!(delivered.to, Desc::Export(0u64.into()));
        assert_eq!(
            delivered.args[0],
            Value::Symbol("fulfill".into()),
            "the fixture should have been fulfilled, not refused"
        );
        assert!(
            matches!(delivered.args[1], Value::Record(_)),
            "the node should hand back a descriptor for the object; got {:?}",
            delivered.args[1]
        );

        drop(client);
        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A live session does not outlive a node that is asked to stop** (HAZOP row C231).
///
/// The session tasks are spawned and detached, so `serve_ocapn` returning on the stop word used to
/// leave every live session running: the listeners' sockets closed and the sessions the operator was
/// shutting down carried on until the process died. A drain that drains the wrong half is worse than
/// none, because it reports success. The peer's side is the observable — a dropped session's socket
/// reads as a clean close — and the fetch before the stop is the control: a session that had already
/// ended would make the assertion below pass for the wrong reason.
#[test]
fn a_live_session_ends_when_the_node_is_asked_to_stop() {
    let dir = common::temp_dir("ocapn-shutdown-session");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.api_server.ocapn_listen = Some(format!("127.0.0.1:{}", ports[5]));

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;

        let dialer = TcpTestingOnly::bind("127.0.0.1:0")
            .await
            .expect("bind the dialing side");
        let connection = dialer
            .new_outgoing_connection(&locator(ports[5]))
            .await
            .expect("dial the node");
        let identity = Identity::fresh(locator(0)).expect("a session key");
        let mut client = Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
            .await
            .expect("the node should complete the handshake");

        // The control: the session is live and answering.
        let fetch = Deliver {
            to: Desc::Export(0u64.into()),
            args: vec![
                Value::Symbol("fetch".into()),
                Value::Bytes(ECHO_SWISS.to_vec()),
            ],
            answer_pos: None,
            resolve_me_desc: Some(Desc::ImportObject(0u64.into())),
        };
        client
            .send_message(&fetch.to_syrup())
            .await
            .expect("send the fetch");
        client
            .recv_message()
            .await
            .expect("read the reply")
            .expect("the session answers while the node is serving");

        // The operator stops the node, and the session ends with it rather than outliving it.
        node.request_stop();
        let ended = tokio::time::timeout(std::time::Duration::from_secs(30), client.recv_message())
            .await
            .expect("the session must end when the node is asked to stop, not hang")
            .expect("a close is not an error");
        assert!(
            ended.is_none(),
            "the peer sees the connection closed, not another message: {ended:?}"
        );

        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The node serves a peer over `unix`** (issue #249): the transport that is *not* a testing one,
/// authenticated by the socket's file mode rather than by a network address.
///
/// This pins the **transport**, and deliberately stops at the bootstrap: the peer dials over UDS,
/// completes the CapTP handshake, and fetches the node's **ERTP capability** from its bootstrap. The
/// bridge's own round trip — a delivery becoming a deploy and a block — is the two tests below, and it
/// is transport-agnostic: a delivery is handled inside the session loop, on whichever transport the
/// session arrived, so those tests' transport does not need duplicating here.
#[test]
fn a_peer_dials_the_node_over_unix_and_reaches_its_ertp_capability() {
    let dir = common::temp_dir("ocapn-unix");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    // A key, so the node publishes its chain-backed capabilities — ERTP among them — on the bootstrap.
    conf.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    let node_socket = dir.join("ocapn.sock");
    conf.api_server.ocapn_listen_unix = Some(node_socket.display().to_string());
    // One transport at a time: this node listens on unix only.
    conf.api_server.ocapn_listen = None;

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn_unix(&node_socket).await;

        // A dialing side needs a listener of its own (the trait bears on one); it is never accepted on.
        let client_socket = dir.join("client.sock");
        let dialer = UnixNetlayer::bind(&client_socket)
            .await
            .expect("bind the dialing side");
        let connection = dialer
            .new_outgoing_connection(&unix_locator(&node_socket))
            .await
            .expect("dial the node over unix");
        let identity = Identity::fresh(unix_locator(&client_socket)).expect("a session key");
        let mut client = Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
            .await
            .expect("the node should complete the handshake over unix");

        // Fetch the ERTP capability from the node's bootstrap (its export 0), reply to our export 0 —
        // the same shape the first test uses for the echo fixture.
        let fetch = Deliver {
            to: Desc::Export(0u64.into()),
            args: vec![
                Value::Symbol("fetch".into()),
                Value::Bytes(ERTP_SWISS.to_vec()),
            ],
            answer_pos: None,
            resolve_me_desc: Some(Desc::ImportObject(0u64.into())),
        };
        client
            .send_message(&fetch.to_syrup())
            .await
            .expect("send the fetch");

        let reply = client
            .recv_message()
            .await
            .expect("read the reply")
            .expect("a reply, not a closed connection");
        let delivered = Deliver::from_syrup(&reply).expect("a delivery");
        assert_eq!(
            delivered.args[0],
            Value::Symbol("fulfill".into()),
            "the ERTP capability should be fulfilled over unix, not refused; got {:?}",
            delivered.args.get(1)
        );
        assert!(
            matches!(delivered.args[1], Value::Record(_)),
            "the node should hand back a descriptor for the capability; got {:?}",
            delivered.args[1]
        );

        drop(client);
        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The node serves a peer over `noise`** — the transport a peer *not on this host* can use, and the
/// only one that both encrypts the channel and authenticates both ends.
///
/// This is the node-level half of `ocapn/src/noise.rs`: the netlayer's own tests handshake two of our
/// endpoints, and `spec/audit/evidence/ocapn-noise/` handshakes ours against Agoric's core, but
/// neither says the *node* can be configured to serve one. It reads the identity file the node wrote —
/// that is where the name a dialler must check comes from — and fetches the ERTP capability through
/// the encrypted session.
#[test]
fn a_peer_dials_the_node_over_noise_and_reaches_its_ertp_capability() {
    let dir = common::temp_dir("ocapn-noise");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    let identity_path = dir.join("noise-identity.key");
    conf.api_server.ocapn_listen_noise = Some(format!("127.0.0.1:{}", ports[5]));
    conf.api_server.ocapn_identity_key = Some(identity_path.display().to_string());
    // One transport at a time: this node listens on noise only.
    conf.api_server.ocapn_listen = None;

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;

        // The node writes its identity on first use. Reading it is how a dialler learns the name it
        // has to put in the SYN's cleartext prefix — a responder refuses a SYN naming another node.
        let stored = std::fs::read(&identity_path).expect("the node should write its identity");
        assert_eq!(stored.len(), 64, "an Ed25519 seed then an X25519 static");
        let node_identity = rchain_ocapn::noise::NoiseIdentity::new(
            stored[..32].try_into().expect("32 bytes"),
            stored[32..].try_into().expect("32 bytes"),
        )
        .expect("a valid identity");
        let verify = rchain_shared::base16::encode(&node_identity.verifying_key());

        let dialer = NoiseNetlayer::bind(
            "127.0.0.1:0",
            rchain_ocapn::noise::NoiseIdentity::generate().expect("a fresh identity"),
        )
        .await
        .expect("bind the dialing side");
        let locator = PeerLocator {
            designator: verify.clone(),
            transport: "noise".to_string(),
            hints: std::collections::BTreeMap::from([
                ("host".to_string(), "127.0.0.1".to_string()),
                ("port".to_string(), ports[5].to_string()),
                ("verify".to_string(), verify),
            ]),
        };
        let connection = dialer
            .new_outgoing_connection(&locator)
            .await
            .expect("dial the node over noise");
        let identity = Identity::fresh(locator.clone()).expect("a session key");
        let mut client = Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
            .await
            .expect("the node should complete the Noise handshake");

        let fetch = Deliver {
            to: Desc::Export(0u64.into()),
            args: vec![
                Value::Symbol("fetch".into()),
                Value::Bytes(ERTP_SWISS.to_vec()),
            ],
            answer_pos: None,
            resolve_me_desc: Some(Desc::ImportObject(0u64.into())),
        };
        client
            .send_message(&fetch.to_syrup())
            .await
            .expect("send the fetch");
        let reply = client
            .recv_message()
            .await
            .expect("read the reply")
            .expect("a reply, not a closed connection");
        let delivered = Deliver::from_syrup(&reply).expect("a delivery");
        assert_eq!(
            delivered.args[0],
            Value::Symbol("fulfill".into()),
            "the ERTP capability should be fulfilled over noise; got {:?}",
            delivered.args.get(1)
        );

        drop(client);
        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The node serves a peer over `websocket`** — the transport `@endo/ocapn` speaks, and so the one a
/// published Agoric peer *could* reach. Weaker than `noise` (no TLS, and only the server proves
/// itself).
///
/// **It dials with this repository's own client**, so it asserts self-consistency and not agreement
/// with a foreign peer — which is exactly the class of defect the interop run found and this test
/// could not (the gcrypt `desc:sig-envelope` encoding). The live peer is `endo-spike/run-3.txt`, and
/// that run stops at the handshake; the test's earlier doc claimed otherwise.
///
/// The dialler has to name the node by its Ed25519 key, exactly as over `noise`: the in-band
/// challenge is a signature the dialler checks against that key, and a node that could not produce it
/// would be indistinguishable from an impostor.
#[test]
fn a_peer_dials_the_node_over_websocket_and_reaches_its_ertp_capability() {
    let dir = common::temp_dir("ocapn-ws");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    let identity_path = dir.join("ocapn-identity.key");
    conf.api_server.ocapn_listen_websocket = Some(format!("127.0.0.1:{}", ports[5]));
    conf.api_server.ocapn_identity_key = Some(identity_path.display().to_string());
    // One transport at a time.
    conf.api_server.ocapn_listen = None;

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;

        let stored = std::fs::read(&identity_path).expect("the node should write its identity");
        assert_eq!(stored.len(), 64, "an Ed25519 seed then an X25519 static");
        let node_identity = rchain_ocapn::noise::NoiseIdentity::new(
            stored[..32].try_into().expect("32 bytes"),
            stored[32..].try_into().expect("32 bytes"),
        )
        .expect("a valid identity");
        let verify = rchain_shared::base16::encode(&node_identity.verifying_key());

        let dialer = WebsocketNetlayer::bind(
            "127.0.0.1:0",
            rchain_ocapn::noise::NoiseIdentity::generate().expect("a fresh identity"),
        )
        .await
        .expect("bind the dialing side");
        let locator = PeerLocator {
            designator: verify.clone(),
            transport: "websocket".to_string(),
            hints: std::collections::BTreeMap::from([
                ("url".to_string(), format!("ws://127.0.0.1:{}", ports[5])),
                ("verify".to_string(), verify),
            ]),
        };
        let connection = dialer
            .new_outgoing_connection(&locator)
            .await
            .expect("the node should answer the in-band challenge");
        let identity = Identity::fresh(locator.clone()).expect("a session key");
        let mut client = Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
            .await
            .expect("the node should complete the CapTP handshake over websocket");

        let fetch = Deliver {
            to: Desc::Export(0u64.into()),
            args: vec![
                Value::Symbol("fetch".into()),
                Value::Bytes(ERTP_SWISS.to_vec()),
            ],
            answer_pos: None,
            resolve_me_desc: Some(Desc::ImportObject(0u64.into())),
        };
        client
            .send_message(&fetch.to_syrup())
            .await
            .expect("send the fetch");
        let reply = client
            .recv_message()
            .await
            .expect("read the reply")
            .expect("a reply, not a closed connection");
        let delivered = Deliver::from_syrup(&reply).expect("a delivery");
        assert_eq!(
            delivered.args[0],
            Value::Symbol("fulfill".into()),
            "the ERTP capability should be fulfilled over websocket; got {:?}",
            delivered.args.get(1)
        );

        drop(client);
        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Two sessions contend for one capability's share, and one of them gets nothing** (W-N8-02).
///
/// The row: the bridge's bridged-deploy budget is spent **across sessions**, so one peer asking takes
/// what another peer needed. This arm is that claim in the form that holds still.
///
/// **Three readings went into getting here, and the two that did not hold are the finding.** (1) A
/// flood of ~110 deliveries in four seconds left the honest session served — but **541** on a loaded
/// CI runner got it refused with the share's own sentence. (2) A *single* spend followed by a single
/// ask was then served here, three runs over, and refused on CI. The reason both flipped is one
/// number: the share refills at **one per second**, so what a second ask sees is where in the refill
/// window it lands — and that is decided by how long the first delivery's *reply* took (a block round
/// trip), which is the runner's speed and not the rule's behaviour.
///
/// **So the arm is contention: two asks with no window between them.** Both sessions send, neither
/// waits for its answer, and the node takes them in whatever order it reaches them. One spends the
/// share and the other draws the bound's sentence, whichever order that happens in — which is the row
/// as a property rather than as a race: the honest peer that asks second gets nothing, and nothing
/// gives it priority.
///
/// Falsified by giving each session its own share: both asks are then served, and the assertion below
/// names that reading.
#[test]
fn two_sessions_contend_for_one_capabilitys_share_and_one_gets_nothing() {
    let dir = common::temp_dir("ocapn-starvation");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.dev_mode = true;
    conf.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    conf.api_server.ocapn_listen = Some(format!("127.0.0.1:{}", ports[5]));
    conf.propose_on_deploy = true;

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;
        wait_for_genesis(&format!("http://127.0.0.1:{}", ports[0])).await;

        async fn dial(port: u16, seed: u8, designator: &str) -> Session {
            let dialer = TcpTestingOnly::bind("127.0.0.1:0")
                .await
                .expect("bind the dialing side");
            let connection = dialer
                .new_outgoing_connection(&locator(port))
                .await
                .expect("dial the node");
            // **Two designators, not two keys under one name.** Both sessions dialling at once under
            // the same designator is a *crossed hello* to the node, which aborts one of them
            // ("Crossed hellos mitigated" — measured on the first run of this arm) and leaves the
            // other with a closed session. A seeded identity rather than `Identity::fresh` keeps the
            // keys distinct as well: the row is about one peer denying another, which a shared
            // identity would confound.
            let mut mine = locator(0);
            mine.designator = designator.to_string();
            let identity = Identity::from_seed([seed; 32], mine).expect("a session key");
            Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
                .await
                .expect("the node should complete the handshake")
        }

        let ask = |capability: Desc| Deliver {
            to: capability,
            args: vec![Value::Symbol("makeIssuerKit".into())],
            answer_pos: None,
            resolve_me_desc: Some(Desc::ImportObject(1u64.into())),
        };

        let mut first = dial(ports[5], 11, "contender-a").await;
        let mut second = dial(ports[5], 12, "contender-b").await;
        let first_capability = fetch(&mut first, ERTP_SWISS).await;
        let second_capability = fetch(&mut second, ERTP_SWISS).await;

        // **Both ask, neither waits.** The window between them is one round trip on this host — a
        // microsecond — so the two land inside one refill second whatever the machine is doing.
        first
            .send_message(&ask(first_capability).to_syrup())
            .await
            .expect("the first session asks");
        second
            .send_message(&ask(second_capability).to_syrup())
            .await
            .expect("the second session asks");

        // Read both answers. The served one waits for its block; the refused one gets a break at once.
        let mut served = 0;
        let mut denied = 0;
        let mut denial = String::new();
        for (session, who) in [(&mut first, "first"), (&mut second, "second")] {
            let reply =
                tokio::time::timeout(std::time::Duration::from_secs(120), session.recv_message())
                    .await
                    .unwrap_or_else(|_| panic!("{who} was never answered — a break is an answer"))
                    .expect("read the reply")
                    .expect("a reply, not a closed connection");
            // **A refusal reaches a peer in either of two shapes** — wrapped in the delivery that
            // resolves onto it, or as the bare `[<break> reason>]` record — and an arm that knew only
            // one of them read a refusal as a parse error. Measured here, on the first run of this
            // arm.
            let (label, detail) = match Deliver::from_syrup(&reply) {
                Ok(delivered) => (
                    delivered.args.first().cloned(),
                    delivered.args.get(1).cloned(),
                ),
                Err(_) => match &reply {
                    Value::Record(fields) => (fields.first().cloned(), fields.get(1).cloned()),
                    other => panic!("{who} got neither a delivery nor a record: {other:?}"),
                },
            };
            match label {
                Some(Value::Symbol(l)) if l == "fulfill" => served += 1,
                Some(Value::Symbol(l)) if l == "break" => {
                    denied += 1;
                    denial = format!("{detail:?}");
                }
                other => panic!("{who} got something else: {other:?}"),
            }
        }

        assert_eq!(
            served, 1,
            "**one of the two sessions is served and the other is not** (W-N8-02): the capability's \
             share of the bridge's budget is one per second and it is not per session, so the second \
             asker gets nothing ({served} served, {denied} refused: {denial}). Two served would mean \
             each session has its own share — which would refute the row"
        );
        assert_eq!(
            denied, 1,
            "and exactly one is refused, so the contention is the bound and not an error: {denial}"
        );
        assert!(
            denial.contains("1 of the chain bridge's 4"),
            "the refusal is the share's own sentence rather than another bound: {denial}"
        );

        drop(first);
        drop(second);
        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A stranger in the middle reads the CapTP records, a chain-capability delivery among them**
/// (W-N8-03).
///
/// The row: the websocket transport — the one the node offers to *remote* peers, unlike
/// `tcp-testing-only` — carries CapTP as plaintext Syrup on the socket, so a relay between the node
/// and its peer reads (and can rewrite) every record in flight, and nothing cryptographic notices.
///
/// The arm is a relay this test owns: it accepts the dial, opens its own connection to the node, and
/// copies bytes both ways while keeping a copy of what it forwarded. The delivery below asks for the
/// ERTP capability **by its swiss number**, and that is the finding: the number a stranger used to
/// name the capability crosses the wire in the clear, so the relay learns which capability was asked
/// for and could ask for another.
///
/// **What the relay reads, and what it does not.** The node's own direction is unmasked Syrup: the
/// refusal below ("no such export") is on the wire in the clear, which is what the assertion pins.
/// The peer's direction carries websocket's per-frame mask — **framing, not confidentiality**: the
/// key travels in the frame header the relay is already copying, so a relay that cares unmasks it.
/// The cell says both halves rather than claiming the masked half is protection.
///
/// Falsified by giving the transport a confidential channel — a Noise handshake under the websocket
/// as `ocapn/src/noise.rs` does over TCP — after which the captured bytes contain no readable record.
#[test]
fn a_relay_between_the_node_and_a_peer_reads_the_captp_records_in_the_clear() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let dir = common::temp_dir("ocapn-mitm");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    let identity_path = dir.join("ocapn-identity.key");
    conf.api_server.ocapn_listen_websocket = Some(format!("127.0.0.1:{}", ports[5]));
    conf.api_server.ocapn_identity_key = Some(identity_path.display().to_string());
    conf.api_server.ocapn_listen = None;

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;

        let stored = std::fs::read(&identity_path).expect("the node should write its identity");
        let node_identity = rchain_ocapn::noise::NoiseIdentity::new(
            stored[..32].try_into().expect("32 bytes"),
            stored[32..].try_into().expect("32 bytes"),
        )
        .expect("a valid identity");
        let verify = rchain_shared::base16::encode(&node_identity.verifying_key());

        // --- the relay, which is the whole instrument -------------------------------------------
        let relay = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the relay");
        let relay_port = relay.local_addr().expect("the relay's port").port();
        let seen: Arc<tokio::sync::Mutex<Vec<u8>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let relay_seen = seen.clone();
        let node_port = ports[5];
        tokio::spawn(async move {
            let (mut inbound, _) = relay.accept().await.expect("the client dials the relay");
            let mut upstream = tokio::net::TcpStream::connect(("127.0.0.1", node_port))
                .await
                .expect("the relay dials the node");
            let (mut from_client, mut to_client) = inbound.split();
            let (mut from_node, mut to_node) = upstream.split();
            let outbound = {
                let relay_seen = relay_seen.clone();
                async move {
                    let mut buf = [0u8; 8192];
                    while let Ok(n) = from_client.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        relay_seen.lock().await.extend_from_slice(&buf[..n]);
                        if to_node.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                }
            };
            // Both directions are kept: the node's replies are what the assertion reads, and the
            // peer's are what the cell is honest about (see the doc comment).
            let relay_seen = relay_seen.clone();
            let inbound = async move {
                let mut buf = [0u8; 8192];
                while let Ok(n) = from_node.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    relay_seen.lock().await.extend_from_slice(&buf[..n]);
                    if to_client.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            };
            tokio::join!(outbound, inbound);
        });

        // --- the peer, which believes it is talking to the node ----------------------------------
        let dialer = WebsocketNetlayer::bind(
            "127.0.0.1:0",
            rchain_ocapn::noise::NoiseIdentity::generate().expect("a fresh identity"),
        )
        .await
        .expect("bind the dialing side");
        let locator = PeerLocator {
            designator: verify.clone(),
            transport: "websocket".to_string(),
            hints: std::collections::BTreeMap::from([
                ("url".to_string(), format!("ws://127.0.0.1:{relay_port}")),
                ("verify".to_string(), verify),
            ]),
        };
        let connection = dialer
            .new_outgoing_connection(&locator)
            .await
            .expect("the node should answer the in-band challenge through the relay");
        let identity = Identity::fresh(locator.clone()).expect("a session key");
        let mut client = Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
            .await
            .expect("the CapTP handshake should complete through the relay");

        let fetch = Deliver {
            to: Desc::Export(0u64.into()),
            args: vec![
                Value::Symbol("fetch".into()),
                Value::Bytes(ERTP_SWISS.to_vec()),
            ],
            answer_pos: None,
            resolve_me_desc: Some(Desc::ImportObject(0u64.into())),
        };
        client
            .send_message(&fetch.to_syrup())
            .await
            .expect("send the fetch");
        let reply = client
            .recv_message()
            .await
            .expect("read the reply")
            .expect("a reply, not a closed connection");
        let delivered = Deliver::from_syrup(&reply).expect("a delivery");
        assert_eq!(
            delivered.args[0],
            Value::Symbol("fulfill".into()),
            "the session has to *work* through the relay, or there is nothing in the middle to read"
        );

        // A second delivery, to an export the node does not have: the node's **own words** are the
        // reading, because they are composed by the node and cannot have been echoed from here.
        let bogus = Deliver {
            to: Desc::Export(9999u64.into()),
            args: vec![Value::Symbol("knock".into())],
            answer_pos: None,
            resolve_me_desc: Some(Desc::ImportObject(1u64.into())),
        };
        client
            .send_message(&bogus.to_syrup())
            .await
            .expect("send to a bogus export");
        let _ = client.recv_message().await.expect("read the refusal");

        // --- the reading --------------------------------------------------------------------------
        let captured = seen.lock().await.clone();
        assert!(
            !captured.is_empty(),
            "the relay forwarded nothing, so this arm measured its own wiring"
        );
        let needle = b"no such export";
        assert!(
            captured
                .windows(needle.len())
                .any(|window| window == needle),
            "**the relay read the node's records in the clear** (W-N8-03): {} bytes were forwarded in \
             both directions and the node's own refusal — `{}` — is not among them. Not finding it \
             would mean the records are confidential on this transport, which would refute the row",
            captured.len(),
            String::from_utf8_lossy(needle)
        );

        drop(client);
        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// The bridge: a CapTP delivery to a chain-backed capability becomes a signed deploy, and the value
/// that deploy wrote to its reply channel comes back as the CapTP promise's fulfilment.
/// **This is also the arm for W-N8-01, and the arm is the deviation.**
///
/// The row: a stranger's delivery to `rho:rchain:revVault/getBalance` over `tcp-testing-only` is
/// answered by a deploy the **node** signs, spending the node's own REV — and the node records
/// nothing about which session asked. The assertion below is that arithmetic and not a shape: the
/// balance the capability returns is the genesis wallet's *less the phlo this very delivery spent*
/// (1_000_000_000_000 − 1_000_000), so the absent 1_000_000 is the node's key paying for a call a
/// peer that proved nothing made. What would refute the row is a caller identity threaded to the
/// capability — the node submitting under *the caller's* name, or refusing — and that is the half
/// named as missing in `node/src/api/ocapn.rs`; nothing here claims it exists.
#[test]
fn a_captp_delivery_to_a_chain_backed_capability_resolves_from_a_block() {
    let dir = common::temp_dir("ocapn-bridge");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.api_server.ocapn_listen = Some(format!("127.0.0.1:{}", ports[5]));
    // The bridge signs with the node's deployer key and needs a block to carry the deploy.
    conf.dev_mode = true;
    conf.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    conf.propose_on_deploy = true;

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;
        wait_for_genesis(&format!("http://127.0.0.1:{}", ports[0])).await;

        let dialer = TcpTestingOnly::bind("127.0.0.1:0")
            .await
            .expect("bind the dialing side");
        let connection = dialer
            .new_outgoing_connection(&locator(ports[5]))
            .await
            .expect("dial the node");
        let identity = Identity::fresh(locator(0)).expect("a session key");
        let mut client = Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
            .await
            .expect("the node should complete the handshake");

        // 1. Fetch the chain-backed capability from the bootstrap.
        let capability = fetch(&mut client, REV_VAULT_BALANCE_SWISS).await;

        // 2. Deliver to it: one REV address in. Everything after this is the node's own doing — it
        //    builds the term, signs the deploy, submits it, and waits for the block.
        let call = Deliver {
            to: capability,
            args: vec![Value::String(common::DEPLOYER_REV_ADDR.to_string())],
            answer_pos: None,
            resolve_me_desc: Some(Desc::ImportObject(1u64.into())),
        };
        client
            .send_message(&call.to_syrup())
            .await
            .expect("send the call");

        // The bridged deploy lands because **the node proposes it**: `propose_on_deploy` is on, and
        // that is the mechanism this test is exercising. So this explicit nudge — the shape
        // `deploy_block.rs` uses, kept as a second chance — normally arrives while the node's own
        // propose is already running, and the node answers `Failure: another propose is in progress`.
        // **That is not a failure here**, and asserting on it is how this test failed twice on CI: the
        // refusal says a propose is in flight, which is a block on its way, and the assertion that
        // matters is the reply below — the only thing that says a block carried the *deploy*. Both
        // answers have been measured here; only these two are acceptable, and neither is required.
        let propose = GrpcProposeService::connect("127.0.0.1", ports[2] as i32, 16 * 1024 * 1024)
            .await
            .expect("propose gRPC");
        match propose.propose(false).await {
            Ok(_) => {}
            Err(errors)
                if errors
                    .iter()
                    .any(|e| e.contains("another propose is in progress")) => {}
            Err(errors) => panic!("propose failed: {errors:?}"),
        }

        // 3. The reply is the balance the deployed `revVault.getBalance` put on its reply channel —
        //    read from a block and carried back over CapTP.
        //
        //    The value is the genesis wallet's balance **less the phlo this very deploy spent**
        //    (1_000_000_000_000 − 1_000_000). That is a stronger check than "a number arrived": the
        //    deploy paid for itself out of the vault it was reading, so the assertion pins the whole
        //    round trip rather than its shape.
        //
        //    **Bounded**, because "no block was produced" has to be a failure with the reason in it
        //    rather than a job that sits here until its timeout: the deploy lands only if a block
        //    carries it, and that is the node's half of this test.
        let reply = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            client.recv_message(),
        )
        .await
        .expect(
            "the bridge should answer within 120s — a delivered call becomes a deploy and a block",
        )
        .expect("read the reply")
        .expect("a reply, not a closed connection");
        let delivered = Deliver::from_syrup(&reply).expect("a delivery");
        assert_eq!(
            delivered.args[0],
            Value::Symbol("fulfill".into()),
            "the bridge refused the delivery: {:?}",
            delivered.args.get(1)
        );
        assert_eq!(
            delivered.args[1],
            Value::Int(999_999_000_000i64.into()),
            "the balance should be the genesis wallet's less this deploy's phlo"
        );

        drop(client);
        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The ERTP round trip over CapTP** (issue #249, clause 4): a peer holds an RChain *issuer* and
/// calls it.
///
/// What makes this different from the REV balance capability above is the **direction the objects
/// travel**. A balance is data, and data crosses CapTP as Syrup. An issuer, a purse and a brand are
/// *capabilities* — unforgeable names and bundles — and `par_value::par_to_value` refuses them by
/// design, because mapping one to data would copy the authority. So the bridge registers what a call
/// returns into the chain's registry in the same deploy that produced it, and hands the peer a CapTP
/// **descriptor** for each capability inside that reply. The peer then calls *those*.
///
/// The test walks the path clause 4 names: the contract, a kit, the issuer, a purse, and finally the
/// balance of that purse — a value that came out of a block, objects deep, from a reference the peer
/// was handed rather than one it knew the name of.
///
/// **This is also the arm for W-N8-04, and the arm is the deviation.**
///
/// The row: `[makeIssuerKit "X"]` delivered to `rho:rchain:ertp` over `tcp-testing-only` **does** mint
/// a kit under the node's key and hand the caller a capability, for a peer that proved nothing —
/// `tcp-testing-only` is the transport with no key proof at all, and this client is
/// `Identity::fresh`, an ephemeral key with no bond and no registration. The three descriptors below
/// are that finding reproduced: a stranger asked the node's own ERTP contract for a kit and got a
/// brand, a mint and an issuer back. The row stays CONFIRMED — what would refute it is a registry
/// insert gated on the caller's identity, which is `W-N8-01`'s missing half, and nothing here
/// pretends otherwise.
#[test]
fn a_captp_peer_holds_an_ertp_issuer_and_calls_it() {
    let dir = common::temp_dir("ocapn-ertp");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.api_server.ocapn_listen = Some(format!("127.0.0.1:{}", ports[5]));
    conf.dev_mode = true;
    conf.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    conf.propose_on_deploy = true;

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;
        wait_for_genesis(&format!("http://127.0.0.1:{}", ports[0])).await;

        let dialer = TcpTestingOnly::bind("127.0.0.1:0")
            .await
            .expect("bind the dialing side");
        let connection = dialer
            .new_outgoing_connection(&locator(ports[5]))
            .await
            .expect("dial the node");
        let identity = Identity::fresh(locator(0)).expect("a session key");
        let mut client = Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
            .await
            .expect("the node should complete the handshake");

        // 1. The ERTP contract itself — a capability whose *methods ride the message*, which is what
        //    makes an object with several arms holdable.
        let ertp = fetch(&mut client, ERTP_SWISS).await;

        // 2. `makeIssuerKit` → a descriptor for each member of the kit. Three capabilities came back,
        //    so three descriptors did: the bridge found them by inspecting the reply.
        let kit = call_many(&mut client, &ertp, "makeIssuerKit", &[]).await;
        assert_eq!(
            kit.len(),
            3,
            "a kit is a brand, a mint and an issuer — three capabilities, three descriptors: {kit:?}"
        );
        let issuer = kit[2].clone();

        // 3. The issuer makes a purse — **one** capability, so the reply is one descriptor rather
        //    than a list of one.
        let purse = call(&mut client, &issuer, "makeEmptyPurse", &[]).await;

        // 4. **The purse answers.** `getCurrentAmount` is data, so it comes back as a value — and it
        //    is the answer of a *live ERTP purse* on chain, reached through a reference this peer was
        //    handed over CapTP and never knew the name of.
        // It is a *value* reply, so it arrives in its Syrup form: the Rholang pair `(true, 0)` as the
        // **tagged record** a tuple crosses as — `<desc:tagged 'rho:tuple' [true, 0]>` (AUDIT C226;
        // the law is `Rchain.syrup_decode_encode`). This is what makes the pair come back a pair:
        // sent as a bare list it would arrive at a contract's `@(brand, value)` pattern as an `EList`
        // and match nothing, which is why a peer could hold a purse and not fund it.
        let reply = call_value(&mut client, &purse, "getCurrentAmount", &[]).await;
        let Value::Record(parts) = &reply else {
            panic!("a purse answers the tagged tuple, got {reply:?}");
        };
        assert_eq!(parts[0], Value::Symbol("desc:tagged".into()), "{reply:?}");
        assert_eq!(parts[1], Value::Symbol("rho:tuple".into()), "{reply:?}");
        let Value::List(fields) = &parts[2] else {
            panic!("the tuple's fields are a list, got {reply:?}");
        };
        assert_eq!(fields.len(), 2, "a purse answers a pair: {reply:?}");
        assert_eq!(
            fields[0],
            Value::Bool(true),
            "a fresh purse must read zero, not refuse: {reply:?}"
        );
        assert_eq!(
            fields[1],
            Value::Int(0.into()),
            "a fresh purse is empty: {reply:?}"
        );

        drop(client);
        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A capability the peer holds, passed back as an argument** (C224 item 4).
///
/// The gap this closes: a peer could *hold* a capability — the kit above proves it — but could not
/// hand one to a contract, because a capability is a descriptor and `par_value` refuses the label it
/// carries. So the ERTP arms that take one were unreachable, and `amountMath.make(brand, value)` is
/// the cheapest of them to demonstrate: the brand is a capability the peer was handed, the value is
/// data, and the contract answers with the amount it made *from both*.
///
/// Before the fix this delivery broke with `a Symbol — Rholang has no symbol for it to land in`,
/// which named the label rather than the situation.
#[test]
fn a_captp_peer_passes_a_capability_back_as_an_argument() {
    let dir = common::temp_dir("ocapn-ertp-arg");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.api_server.ocapn_listen = Some(format!("127.0.0.1:{}", ports[5]));
    conf.dev_mode = true;
    conf.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    conf.propose_on_deploy = true;

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;
        wait_for_genesis(&format!("http://127.0.0.1:{}", ports[0])).await;

        let dialer = TcpTestingOnly::bind("127.0.0.1:0")
            .await
            .expect("bind the dialing side");
        let connection = dialer
            .new_outgoing_connection(&locator(ports[5]))
            .await
            .expect("dial the node");
        let identity = Identity::fresh(locator(0)).expect("a session key");
        let mut client = Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
            .await
            .expect("the node should complete the handshake");

        // A kit, so the peer holds a brand to pass back.
        let ertp = fetch(&mut client, ERTP_SWISS).await;
        let kit = call_many(&mut client, &ertp, "makeIssuerKit", &[]).await;
        let (brand, issuer) = (kit[0].clone(), kit[2].clone());

        // The issuer makes the amountMath; the amountMath is an object, so one descriptor.
        let amount_math = call(&mut client, &issuer, "getAmountMath", &[]).await;

        // **The brand crosses as an argument.** `kit[0]` is the peer's handle on the brand, which is
        // the node's own export — so it is sent back as `<desc:export N>`, and the node resolves it to
        // the registry location it minted for the kit.
        // The reply is an amount `(brand, 10)` — **one** capability in it, so one descriptor: the
        // contract put the brand the peer passed into the value it answered with. A `break` here
        // would panic inside the helper with the reason, which is what this asserted before the fix.
        let amount = call(
            &mut client,
            &amount_math,
            "make",
            &[brand.to_syrup(), Value::Int(10.into())],
        )
        .await;
        assert_ne!(
            amount, brand,
            "the amount is a new object holding the brand, not the brand itself"
        );

        drop(client);
        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// **An amount a peer builds itself** (AUDIT C226's observable).
///
/// The gap this closes: a peer could pass a capability as an argument (the test above), but not a
/// capability *inside* a value — an ERTP amount is `(brand, value)`, and the brand is a capability
/// it holds. `amountMath.getValue` matches its argument against `@(brand, value)`, so the pair has to
/// arrive as a **tuple** with the brand **named**: sent as a bare list it would arrive an `EList`
/// and match nothing, and with the brand as data it could not be the same brand the contract knows.
///
/// Before the fix the delivery broke with "a capability nested inside a value cannot be named yet".
#[test]
fn a_captp_peer_sends_an_amount_and_the_contract_reads_it() {
    let dir = common::temp_dir("ocapn-ertp-amount");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.api_server.ocapn_listen = Some(format!("127.0.0.1:{}", ports[5]));
    conf.dev_mode = true;
    conf.dev.deployer_private_key = Some(common::VALIDATOR_PRIV_HEX.to_string());
    conf.propose_on_deploy = true;

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;
        wait_for_genesis(&format!("http://127.0.0.1:{}", ports[0])).await;

        let dialer = TcpTestingOnly::bind("127.0.0.1:0")
            .await
            .expect("bind the dialing side");
        let connection = dialer
            .new_outgoing_connection(&locator(ports[5]))
            .await
            .expect("dial the node");
        let identity = Identity::fresh(locator(0)).expect("a session key");
        let mut client = Session::dial(connection, &identity, Arc::new(Bootstrap::default()))
            .await
            .expect("the node should complete the handshake");

        let ertp = fetch(&mut client, ERTP_SWISS).await;
        let kit = call_many(&mut client, &ertp, "makeIssuerKit", &[]).await;
        let (brand, issuer) = (kit[0].clone(), kit[2].clone());
        let amount_math = call(&mut client, &issuer, "getAmountMath", &[]).await;

        // **The amount, as the peer builds it**: the tagged tuple a Rholang pair crosses as, whose
        // first field is the brand *reference* the peer holds.
        let amount = Value::Record(vec![
            Value::Symbol("desc:tagged".into()),
            Value::Symbol("rho:tuple".into()),
            Value::List(vec![brand.to_syrup(), Value::Int(10.into())]),
        ]);
        let reply = call_value(&mut client, &amount_math, "getValue", &[amount]).await;

        // `(true, 10)`: the contract read the brand out of the tuple, found it *its own* (`if (aBrand
        // == brand)`), and answered the value. A brand that had crossed as data could not have been
        // the same name, and one inside a bare list would not have reached the pattern.
        let Value::Record(parts) = &reply else {
            panic!("getValue answers a tagged tuple, got {reply:?}");
        };
        let Value::List(fields) = &parts[2] else {
            panic!("the tuple's fields are a list, got {reply:?}");
        };
        assert_eq!(
            fields,
            &vec![Value::Bool(true), Value::Int(10.into())],
            "the contract must read the amount the peer built: {reply:?}"
        );

        drop(client);
        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// Call `method` on `capability` and read the reply, which must be **one** descriptor.
///
/// One object is one descriptor, not a one-element list: `E(obj).makeEmptyPurse()` should hand the
/// caller a purse, and a caller made to destructure a collection to reach the only thing in it is a
/// worse peer. So this reads a bare delivery, not the list [`call_many`] reads.
async fn call(client: &mut Session, capability: &Desc, method: &str, args: &[Value]) -> Desc {
    let mut call_args = vec![Value::Symbol(method.to_string())];
    call_args.extend_from_slice(args);
    let reply = deliver_and_read(client, capability, call_args).await;
    match &reply {
        Value::Record(_) => match Desc::from_syrup(&reply).expect("a descriptor") {
            Desc::ImportObject(position) => Desc::Export(position),
            other => panic!("`{method}` answered {other:?}, not a descriptor"),
        },
        other => panic!("`{method}` answered {other:?}, not one descriptor"),
    }
}

/// Call `method` on `capability` and read a reply that must be **several** descriptors.
async fn call_many(
    client: &mut Session,
    capability: &Desc,
    method: &str,
    args: &[Value],
) -> Vec<Desc> {
    let mut call_args = vec![Value::Symbol(method.to_string())];
    call_args.extend_from_slice(args);
    let reply = deliver_and_read(client, capability, call_args).await;
    match &reply {
        Value::List(descriptors) => descriptors
            .iter()
            .map(|d| match Desc::from_syrup(d).expect("a descriptor") {
                Desc::ImportObject(position) => Desc::Export(position),
                other => panic!("expected a descriptor, got {other:?}"),
            })
            .collect(),
        other => panic!("`{method}` answered {other:?}, not descriptors"),
    }
}

/// Call `method` on `capability` and read a **value** reply.
async fn call_value(
    client: &mut Session,
    capability: &Desc,
    method: &str,
    args: &[Value],
) -> Value {
    let mut call_args = vec![Value::Symbol(method.to_string())];
    call_args.extend_from_slice(args);
    deliver_and_read(client, capability, call_args).await
}

/// Deliver `args` to `capability`, and return what the node fulfilled the delivery with.
async fn deliver_and_read(client: &mut Session, capability: &Desc, args: Vec<Value>) -> Value {
    let call = Deliver {
        to: capability.clone(),
        args,
        answer_pos: None,
        resolve_me_desc: Some(Desc::ImportObject(1u64.into())),
    };
    client
        .send_message(&call.to_syrup())
        .await
        .expect("send the call");
    let reply = client
        .recv_message()
        .await
        .expect("read the reply")
        .expect("a reply, not a closed connection");
    let delivered = Deliver::from_syrup(&reply).expect("a delivery");
    match delivered.args.as_slice() {
        [Value::Symbol(verb), value] if verb == "fulfill" => value.clone(),
        [Value::Symbol(verb), reason] if verb == "break" => {
            panic!("the bridge broke the call: {reason:?}")
        }
        other => panic!("expected a fulfilment, got {other:?}"),
    }
}

/// Fetch an object by swiss number from the peer's bootstrap, returning the descriptor to address
/// it by.
async fn fetch(client: &mut Session, swiss: &[u8]) -> Desc {
    let request = Deliver {
        to: Desc::Export(0u64.into()),
        args: vec![Value::Symbol("fetch".into()), Value::Bytes(swiss.to_vec())],
        answer_pos: None,
        resolve_me_desc: Some(Desc::ImportObject(0u64.into())),
    };
    client
        .send_message(&request.to_syrup())
        .await
        .expect("send the fetch");
    let reply = client
        .recv_message()
        .await
        .expect("read the fetch reply")
        .expect("a reply");
    // **The raw message is in the failure**, because "not a delivery" does not say what arrived —
    // and a refusal reaches a peer in more than one shape.
    let delivered = match Deliver::from_syrup(&reply) {
        Ok(delivered) => delivered,
        Err(e) => panic!("the fetch reply is not a delivery ({e}): {reply:?}"),
    };
    assert_eq!(delivered.args[0], Value::Symbol("fulfill".into()));
    match Desc::from_syrup(&delivered.args[1]).expect("a descriptor") {
        Desc::ImportObject(position) => Desc::Export(position),
        other => panic!("expected a descriptor for the object, got {other:?}"),
    }
}

/// **The node starts a session of its own** (issue #249): `POST /api/v1/ocapn/dial` on the admin
/// server makes the node dial a peer the caller names and fetch the object at the swiss number it
/// gives.
///
/// The peer is the **node's own OCapN listener**, dialled over loopback under a designator of the
/// test's choosing — so the assertion pins the whole path (admin route → the dialer → the enlivener's
/// dial-and-fetch → a real CapTP session → a descriptor back) with no second node to stand up. The
/// object fetched is the echo fixture the listener publishes on every bootstrap.
///
/// The second case is the one a caller gets wrong: a **link-local target is refused with its reason**,
/// and the guard is the *target policy* rather than an origin — a dial the node starts has no peer
/// origin, so Law 62's rule has nothing to judge and the metadata endpoint must be refused by the
/// address rule alone. The route's own gate (`enable-ocapn-dial`) is asserted in `web::http`'s unit
/// tests, where the two statuses (404 and 503) can be told apart without a running node.
/// **The first case is the arm for W-N4-07, and it is the deviation rather than its absence.**
///
/// The row: with `enable-ocapn-dial` on, `POST /api/v1/ocapn/dial` with a caller-named **loopback**
/// target makes the node start a connection to it. That is case 1 below — `127.0.0.1:<ocapn port>`
/// is the caller's address, the feature is on, and the node dials and fetches. The row's refutation
/// would be a target policy that refuses a caller-named internal target **by default**; what the
/// policy refuses unconditionally is link-local and the metadata endpoint (case 2), and loopback and
/// private ranges are permitted unless `ocapn-deny-local-dial` is set
/// (`ocapn/src/dial_policy.rs`'s module doc). So the row is CONFIRMED and this is its falsifier
/// taken, not a claim that the surface is closed.
///
#[test]
fn the_admin_route_makes_the_node_dial_a_peer_and_refuses_a_link_local_target() {
    let dir = common::temp_dir("ocapn-dial");
    let ports = common::free_ports(6);
    let mut conf = common::deploy_conf(&dir, &ports);
    conf.api_server.ocapn_listen = Some(format!("127.0.0.1:{}", ports[5]));
    conf.api_server.enable_ocapn_dial = true;
    let dial = format!("http://127.0.0.1:{}/api/v1/ocapn/dial", ports[1]);
    // `base16`, because that is how the route takes a swiss number.
    let swiss = rchain_shared::base16::encode(ECHO_SWISS);

    common::test_runtime().block_on(async {
        let node = common::start(&conf, ports[2], ports[0]).await;
        wait_for_ocapn(ports[5]).await;
        let client = reqwest::Client::new();

        // 1. The node dials its own listener and fetches the echo fixture from its bootstrap.
        let ok = client
            .post(&dial)
            .json(&serde_json::json!({
                "designator": "self",
                "transport": "tcp-testing-only",
                "hints": { "host": "127.0.0.1", "port": ports[5].to_string() },
                "swiss": swiss,
            }))
            .send()
            .await
            .expect("the admin server should answer");
        assert_eq!(ok.status(), 200, "the dial should have fetched the fixture");
        let body: serde_json::Value = ok.json().await.expect("a JSON body");
        assert_eq!(body["peer"], "self.tcp-testing-only");
        assert!(
            body["fetched"].as_str().unwrap_or("").contains("Export"),
            "the answer names the object the peer exported: {body}"
        );

        // 2. **A link-local target is refused with its reason.** The metadata endpoint is never a
        //    CapTP peer, and the refusal is the policy's, before any connection is made.
        let refused = client
            .post(&dial)
            .json(&serde_json::json!({
                "designator": "metadata",
                "transport": "tcp-testing-only",
                "hints": { "host": "169.254.169.254", "port": "80" },
                "swiss": swiss,
            }))
            .send()
            .await
            .expect("the admin server should answer");
        assert_eq!(refused.status(), 502);
        let reason = refused.text().await.expect("a body");
        assert!(
            reason.contains("link-local"),
            "the refusal names why: {reason}"
        );

        node.shutdown().await;
    });
    let _ = std::fs::remove_dir_all(&dir);
}
