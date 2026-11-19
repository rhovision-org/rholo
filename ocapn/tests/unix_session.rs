//! A whole CapTP session over the `unix` transport.
//!
//! `session_round_trip.rs` does this over `tcp-testing-only`; this is the same run over a Unix domain
//! socket, because the transport is claimed to be a drop-in — same handshake, same framing, same
//! export table, nothing above [`Netlayer`] knowing which socket it is on. A claim like that is worth
//! a test rather than a doc comment: the two transports share only `crate::framed` and the trait, so
//! this is where a divergence would show.

// The transport itself is compiled only on Unix (issue #615), so this target exists only there.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use rchain_ocapn::bootstrap::Bootstrap;
use rchain_ocapn::captp::{Deliver, Desc};
use rchain_ocapn::conn::{Act, Export, Identity, Session};
use rchain_ocapn::locator::PeerLocator;
use rchain_ocapn::netlayer::Netlayer;
use rchain_ocapn::syrup::Value;

const ECHO_SWISS: &[u8] = b"IO58l1laTyhcrgDKbEzFOO32MDd6zE5w";

/// Replies with its own arguments: enough to prove a delivery arrived and was answered.
struct Echo;

#[async_trait]
impl Export for Echo {
    async fn deliver(&self, args: &[Value]) -> Result<Act, String> {
        Ok(Act::value(Value::List(args.to_vec())))
    }
}

/// A socket path short enough for `sun_path` (108 bytes on Linux), removed on drop.
struct TempSocket(PathBuf);

impl TempSocket {
    fn new(tag: &str) -> TempSocket {
        let path =
            std::env::temp_dir().join(format!("rchain-ocapn-{}-{}.sock", tag, std::process::id()));
        let _ = std::fs::remove_file(&path);
        TempSocket(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn locator(path: &Path) -> PeerLocator {
    PeerLocator {
        designator: "peer".into(),
        transport: "unix".into(),
        hints: [("path".to_string(), path.display().to_string())].into(),
    }
}

fn bootstrap_with_echo() -> Arc<dyn Export> {
    let mut directory: BTreeMap<Vec<u8>, Arc<dyn Export>> = BTreeMap::new();
    directory.insert(ECHO_SWISS.to_vec(), Arc::new(Echo));
    Arc::new(Bootstrap::new(directory))
}

#[tokio::test]
async fn two_sessions_handshake_and_the_client_fetches_over_unix() {
    let server_sock = TempSocket::new("session");
    let listener = rchain_ocapn::unix::UnixNetlayer::bind(server_sock.path())
        .await
        .unwrap();
    let peer = locator(server_sock.path());

    let (id_tx, id_rx) = tokio::sync::oneshot::channel();
    let peer_for_server = peer.clone();
    let server = tokio::spawn(async move {
        let conn = listener.accept_incoming_connection().await.unwrap();
        let identity = Identity::from_seed([1u8; 32], peer_for_server).unwrap();
        let mut session = Session::accept(conn, &identity, bootstrap_with_echo())
            .await
            .unwrap();
        let _ = id_tx.send(session.id);
        let _ = session.run().await;
    });

    let dialer_sock = TempSocket::new("session-dialer");
    let dialer = rchain_ocapn::unix::UnixNetlayer::bind(dialer_sock.path())
        .await
        .unwrap();
    let conn = dialer.new_outgoing_connection(&peer).await.unwrap();
    let identity = Identity::from_seed([2u8; 32], locator(Path::new(""))).unwrap();
    let mut client = Session::dial(conn, &identity, Arc::new(Bootstrap::default()))
        .await
        .unwrap();

    // Both sides derive the same session id from each other's Syrup-encoded key — the same
    // computation the TCP transport runs, over a socket with no addresses at all.
    assert_eq!(client.id, id_rx.await.unwrap());

    let fetch = Deliver {
        to: Desc::Export(0u64.into()),
        args: vec![
            Value::Symbol("fetch".into()),
            Value::Bytes(ECHO_SWISS.to_vec()),
        ],
        answer_pos: None,
        resolve_me_desc: Some(Desc::ImportObject(0u64.into())),
    };
    client.send_message(&fetch.to_syrup()).await.unwrap();

    let reply = client.recv_message().await.unwrap().unwrap();
    let delivered = Deliver::from_syrup(&reply).unwrap();
    assert_eq!(delivered.to, Desc::Export(0u64.into()));
    assert_eq!(delivered.args[0], Value::Symbol("fulfill".into()));
    assert_eq!(
        delivered.args[1],
        Desc::ImportObject(1u64.into()).to_syrup()
    );

    // A fetch of an unknown swiss number breaks rather than hanging, exactly as over TCP.
    let missing = Deliver {
        to: Desc::Export(0u64.into()),
        args: vec![
            Value::Symbol("fetch".into()),
            Value::Bytes(b"no-such-object".to_vec()),
        ],
        answer_pos: None,
        resolve_me_desc: Some(Desc::ImportObject(1u64.into())),
    };
    client.send_message(&missing.to_syrup()).await.unwrap();
    let reply = client.recv_message().await.unwrap().unwrap();
    let delivered = Deliver::from_syrup(&reply).unwrap();
    assert_eq!(delivered.args[0], Value::Symbol("break".into()));

    drop(client);
    server.await.unwrap();
}
