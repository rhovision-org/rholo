//! Connection bookkeeping and the protocol handshake/connect flow.
//!
//! Mirrors `comm/src/main/scala/coop/rchain/comm/rp/Connect.scala`. The cats-effect `Ref` connection
//! cell becomes `&mut Vec<PeerNode>`.

use std::collections::HashSet;

use rand::seq::SliceRandom;

use crate::discovery::NodeDiscovery;
use crate::errors::CommErr;
use crate::peer_node::PeerNode;
use crate::rp::protocol_helper;
use crate::rp::rp_conf::RPConf;
use crate::transport::transport_layer::TransportLayer;

/// Bound on the size of the `connections` table. Inbound handshakes (`add_conn`) grow the table
/// without bound; a peer that keeps asserting fresh node ids could otherwise exhaust memory. At the
/// cap the oldest entries are evicted (mirrors the `PeerTable` bucket eviction style).
pub const MAX_CONNECTIONS: usize = 1024;

/// How long one `find_and_connect` round may spend dialling (AUDIT C136).
///
/// A dial that fails costs the full `DEFAULT_SEND_TIMEOUT` and gains nothing, so bounding the number
/// of *dials* does not bound the round; bounding the clock does. Thirty seconds is chosen against
/// the loop it runs in — the discovery round repeats on `peers_discovery.lookup_interval`, so an
/// unfinished round is not lost work, it is work the next round picks up.
pub const CONNECT_ROUND_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// Shuffle the connections and take up to `max` (port of `ConnectionsCell.random`).
pub fn random_connections(connections: &[PeerNode], max: usize) -> Vec<PeerNode> {
    let mut shuffled: Vec<PeerNode> = connections.to_vec();
    shuffled.shuffle(&mut rand::rng());
    shuffled.truncate(max);
    shuffled
}

/// Append `to_be_added`, first removing any existing peer with the same id (port of `addConn`).
///
/// The result is capped at [`MAX_CONNECTIONS`]; when the cap is exceeded the oldest (front) entries
/// are evicted, so a peer asserting fresh node ids cannot grow the connection table without bound.
pub fn add_conn(connections: &[PeerNode], to_be_added: &[PeerNode]) -> Vec<PeerNode> {
    let ids: HashSet<Vec<u8>> = to_be_added.iter().map(|p| p.key().to_vec()).collect();
    let mut rest: Vec<PeerNode> = connections
        .iter()
        .filter(|p| !ids.contains(p.key()))
        .cloned()
        .collect();
    rest.extend(to_be_added.iter().cloned());
    if rest.len() > MAX_CONNECTIONS {
        let evicted = rest.len() - MAX_CONNECTIONS;
        rest.drain(..evicted);
        // A deliberately tolerant instrumentation of a pure function: the count is process-wide and
        // advisory, and a relaxed atomic is cheaper than threading a counter through every caller.
        for _ in 0..evicted {
            crate::diagnostics::stats().note_eviction();
        }
    }
    rest
}

/// Remove every peer whose id is in `to_be_removed` (port of `removeConn`).
pub fn remove_conn(connections: &[PeerNode], to_be_removed: &[PeerNode]) -> Vec<PeerNode> {
    let ids: HashSet<Vec<u8>> = to_be_removed.iter().map(|p| p.key().to_vec()).collect();
    connections
        .iter()
        .filter(|p| !ids.contains(p.key()))
        .cloned()
        .collect()
}

/// Move `connection` (matched by id) to the end of the list, or leave unchanged if absent (port of
/// `refreshConn`).
pub fn refresh_conn(connections: &[PeerNode], connection: &PeerNode) -> Vec<PeerNode> {
    let (matched, rest): (Vec<PeerNode>, Vec<PeerNode>) = connections
        .iter()
        .cloned()
        .partition(|p| p.key() == connection.key());
    if matched.is_empty() {
        rest
    } else {
        let mut out = rest;
        out.extend(matched);
        out
    }
}

/// Send a protocol handshake to a peer (port of `connect`).
pub async fn connect<T: TransportLayer + ?Sized>(
    transport: &T,
    conf: &RPConf,
    peer: &PeerNode,
) -> CommErr<()> {
    let ph = protocol_helper::protocol_handshake(&conf.local, &conf.network_id);
    transport.send(peer, ph).await
}

/// Ping the first `num_of_connections_pinged` peers (over the read-only snapshot), returning
/// `(to_ping, successful, failed)`. The caller applies `removeConn(toPing).addConn(successful)` to
/// the *current* connections under a brief write lock, so the outbound sends never hold the
/// connection write-lock (port of `clearConnections`; the Scala `ConnectionsCell.update` is the
/// brief mutation step).
pub async fn clear_connections<T: TransportLayer + ?Sized>(
    transport: &T,
    conf: &RPConf,
    connections: &[PeerNode],
) -> (Vec<PeerNode>, Vec<PeerNode>, usize) {
    let num = conf.clear_connections.num_of_connections_pinged;
    let to_ping: Vec<PeerNode> = connections.iter().take(num).cloned().collect();
    let mut successful = Vec::new();
    let mut failed = 0usize;
    for peer in &to_ping {
        let hb = protocol_helper::heartbeat(&conf.local, &conf.network_id);
        crate::diagnostics::stats().note_heartbeat_ping();
        match transport.send(peer, hb).await {
            Ok(_) => successful.push(peer.clone()),
            Err(_) => {
                failed += 1;
                crate::diagnostics::stats().note_heartbeat_failure();
            }
        }
    }
    (to_ping, successful, failed)
}

/// Discover peers, connect to those not already connected, and return the successful ones (port of
/// `findAndConnect`).
pub async fn find_and_connect<T: TransportLayer + ?Sized>(
    node_discovery: &dyn NodeDiscovery,
    conf: &RPConf,
    transport: &T,
    connections: &[PeerNode],
) -> Vec<PeerNode> {
    let connected: HashSet<Vec<u8>> = connections.iter().map(|p| p.key().to_vec()).collect();
    let peers: Vec<PeerNode> = node_discovery
        .peers()
        .into_iter()
        .filter(|p| !connected.contains(p.key()))
        .collect();
    // **The round is bounded, in two directions, and neither was there before (AUDIT C136).** The
    // loop dialled every peer the Kademlia table offered — up to 5120 for a 32-byte id — one at a
    // time, each bounded only by `DEFAULT_SEND_TIMEOUT` (5 s). `MAX_CONNECTIONS` capped the *result*
    // (`add_conn`), so it bounded the table and not the work: a peer answering a lookup with
    // addresses that accept no connection put the RP loop here for up to ~7 hours, and heartbeats,
    // block requests and connection refresh do not run while it waits.
    //
    // `room` is the first bound and it is not a heuristic: a connection the table cannot hold has no
    // effect, so dialling for one is work that cannot pay. `CONNECT_ROUND_BUDGET` is the second, for
    // the case `room` does not cover — a *failing* dial costs the full timeout and returns nothing,
    // so the number of dials is not the number of connections gained. Whatever a round does not
    // reach is offered to the next one, which runs on `peers_discovery.lookup_interval`.
    let budget = dial_budget(peers.len(), connections.len());
    let stats = crate::diagnostics::stats();
    stats.note_connect_round();
    let deadline = std::time::Instant::now() + CONNECT_ROUND_BUDGET;
    let mut result = Vec::new();
    let mut attempted = 0usize;
    for peer in peers.iter().take(budget) {
        if std::time::Instant::now() >= deadline {
            break;
        }
        attempted += 1;
        stats.note_dial_attempt();
        match connect(transport, conf, peer).await {
            Ok(()) => result.push(peer.clone()),
            Err(_) => stats.note_dial_failure(),
        }
    }
    // A round cut short by its clock also leaves peers undialled; count them with the room shortfall.
    stats.note_dial_budget_skipped(peers.len().saturating_sub(attempted));
    result
}

/// The most peers one round may dial (AUDIT C136).
///
/// Two things bound it and they answer different questions. The **room** is the first and it is not a
/// heuristic: a connection the table cannot hold has no effect on it, so dialling for one is work
/// that cannot pay — and `MAX_CONNECTIONS` used to be applied by `add_conn` to the *result*, which
/// bounded the table and not the loop. The **number of attempts** is the second: a dial that fails
/// costs the full `DEFAULT_SEND_TIMEOUT` and returns nothing, so the loop is additionally bounded by
/// `CONNECT_ROUND_BUDGET` on the clock. This function is the first bound alone, which is why it is
/// pure and testable without a transport.
fn dial_budget(offered: usize, connections: usize) -> usize {
    offered.min(MAX_CONNECTIONS.saturating_sub(connections))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer_node::NodeIdentifier;

    fn peer(id: u8, host: &str) -> PeerNode {
        PeerNode::from(
            NodeIdentifier::new(vec![id]),
            host.into(),
            rchain_shared::refined::Port::new(40400),
            rchain_shared::refined::Port::new(40404),
        )
    }

    /// **AUDIT C136.** The round dials no more peers than the connection table can hold.
    ///
    /// The first arm is the finding's own shape: a Kademlia table offering the thousands it admits
    /// (5120 for a 32-byte id) while the connection table is empty, where the old loop dialled all of
    /// them one at a time. The last arm is the one that shows the bound is the *room* and not a
    /// constant: a table one short of the cap has room for exactly one more peer.
    #[test]
    fn a_connect_round_dials_no_more_than_the_table_can_hold() {
        assert_eq!(
            dial_budget(5120, 0),
            MAX_CONNECTIONS,
            "a table with room for 1024 cannot be improved by 5120 dials"
        );
        assert_eq!(dial_budget(5120, MAX_CONNECTIONS - 1), 1);
        assert_eq!(
            dial_budget(5120, MAX_CONNECTIONS),
            0,
            "a full table has nothing to gain from any dial at all"
        );
        assert_eq!(
            dial_budget(5, 0),
            5,
            "fewer offers than room: dial what there is"
        );
        assert_eq!(dial_budget(0, 7), 0, "an empty offer list dials nothing");
    }
    #[test]
    fn add_conn_appends_new_and_dedupes_existing() {
        let a = peer(1, "a");
        let b = peer(2, "b");
        let c = peer(3, "c");
        let a2 = peer(1, "a2"); // same id, new endpoint
        assert_eq!(
            add_conn(&[a.clone(), b.clone()], &[c.clone()]),
            vec![a.clone(), b.clone(), c.clone()]
        );
        assert_eq!(
            add_conn(&[a.clone(), b.clone()], &[a2.clone()]),
            vec![b, a2]
        );
    }

    #[test]
    fn remove_conn_removes_by_id() {
        let a = peer(1, "a");
        let b = peer(2, "b");
        assert_eq!(
            remove_conn(&[a.clone(), b.clone()], &[b.clone()]),
            vec![a.clone()]
        );
        assert_eq!(
            remove_conn(&[a.clone(), b.clone()], &[peer(9, "x")]),
            vec![a, b]
        );
    }

    #[test]
    fn random_connections_takes_at_most_max() {
        let peers: Vec<PeerNode> = (0..10).map(|i| peer(i, "host")).collect();
        let picked = random_connections(&peers, 3);
        assert_eq!(picked.len(), 3);
        // All picked peers come from the input and are unique (a shuffle is a permutation subset).
        let ids: Vec<Vec<u8>> = picked.iter().map(|p| p.key().to_vec()).collect();
        assert_eq!(ids.len(), 3);
        for p in &picked {
            assert!(peers.contains(p));
        }
    }

    #[test]
    fn random_connections_takes_empty_when_max_zero() {
        let peers: Vec<PeerNode> = (0..5).map(|i| peer(i, "host")).collect();
        assert!(random_connections(&peers, 0).is_empty());
    }

    #[test]
    fn refresh_conn_moves_to_end() {
        let a = peer(1, "a");
        let b = peer(2, "b");
        let c = peer(3, "c");
        assert_eq!(
            refresh_conn(&[a.clone(), b.clone(), c.clone()], &a),
            vec![b.clone(), c.clone(), a.clone()]
        );
        assert_eq!(
            refresh_conn(&[a.clone(), b.clone()], &peer(9, "x")),
            vec![a, b]
        );
    }

    /// **A heartbeating peer survives the eviction it would otherwise be first in line for** (W-N1-07).
    ///
    /// The row: a peer that heartbeats escapes oldest-first eviction, because the table is bounded and
    /// the heartbeat refreshes its position. That property belongs to `refresh_conn` and `add_conn`
    /// **together** — the reorder alone is a curiosity — and the test above pins only the reorder. This
    /// pins the consequence the row asks for: fill to the cap, let the oldest peer speak, add one more,
    /// and the peer that spoke is still there while the one that stayed silent is the one evicted.
    ///
    /// Falsified by making `refresh_conn` a no-op (the speaker keeps the front position and is evicted
    /// first) or by evicting from the back instead of the front.
    #[test]
    fn a_heartbeating_peer_survives_eviction_at_the_cap() {
        // The cap is 1024, so the ids need two bytes: `peer(u8)` cannot name that many.
        let at = |i: usize| {
            PeerNode::from(
                NodeIdentifier::new(vec![(i & 0xff) as u8, (i >> 8) as u8]),
                "peer.example".to_string(),
                rchain_shared::refined::Port::new(40400),
                rchain_shared::refined::Port::new(40404),
            )
        };
        let peers: Vec<PeerNode> = (0..MAX_CONNECTIONS).map(at).collect();
        let speaker = peers[0].clone();
        let silent = peers[1].clone();
        assert_eq!(peers.len(), MAX_CONNECTIONS, "the table starts full");

        let refreshed = refresh_conn(&peers, &speaker);
        assert_eq!(
            refreshed.last().map(|p| p.key().to_vec()),
            Some(speaker.key().to_vec()),
            "the speaker is moved to the end, which is what the test above pins"
        );

        let newcomer = at(MAX_CONNECTIONS + 1);
        let after = add_conn(&refreshed, &[newcomer.clone()]);
        assert_eq!(after.len(), MAX_CONNECTIONS, "the table stays at its bound");
        assert!(
            after.iter().any(|p| p.key() == speaker.key()),
            "**the peer that heartbeated must survive**: eviction is oldest-first, and the heartbeat is              what moved it out of first place (W-N1-07)"
        );
        assert!(
            !after.iter().any(|p| p.key() == silent.key()),
            "and the front peer that did not speak is the one that pays"
        );
        assert!(
            after.iter().any(|p| p.key() == newcomer.key()),
            "the newcomer is admitted, or nothing was evicted and the bound is not doing anything"
        );
    }
}
