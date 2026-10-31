//! **The deploy hostlist** — `GET /api/v1/nodes` (Law 90).
//!
//! A wallet knows one node. This module lets that node answer with the other nodes a client can send
//! a deploy to, so load can be spread instead of concentrated on whichever endpoint is hardcoded into
//! a wallet.
//!
//! **Where the entries come from.** The node's *live connections* — never the Kademlia table: a peer
//! the node has discovered but not connected to is not evidence of a deploy service. Each peer's
//! client URL is derived from the published port layout (`protocol + 3`, so 40400 → 40403, the same
//! block the relay uses for `protocol + 1`), and **the derivation is validated rather than assumed**:
//! an entry exists only because that URL answered `GET /api/v1/status`, named itself as the peer we
//! dialled, reported our network and shard, and said it can propose. A guessed port that lands on
//! nothing — or on the wrong co-hosted validator — is dropped, not served.
//!
//! **Why "can propose" is a field and not a capability.** `manualPropose` is a mode flag
//! (`!autopropose && !proposeOnDeploy`), so an unkeyed observer reports it exactly like a manual-mode
//! validator. The `proposer` field on `/api/v1/status` (from `Capabilities::proposer`) is the
//! discriminator; without it a hostlist cannot honour "every entry can accept and include a deploy".
//!
//! **The list is advisory.** A peer reports its own `proposer`, height and pool state over plain
//! HTTP, so it can lie and an on-path observer can forge a well-formed answer. The list narrows a
//! wallet's choice from one hardcoded node to a set of verified candidates; it is not a trust root,
//! and a client still has to handle a 429 from a listed node.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use rchain_casper::api::block_api::BlockApi;
use rchain_casper::protocol::comm_util::ConnectionsCell;
use rchain_comm::peer_node::PeerNode;
use rchain_comm::rp::handle_messages::{is_local_address, is_local_address_resolved};
use rchain_comm::rp::rp_conf::RPConf;
use rchain_shared::lock::Unpoison;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::time::current_millis;

/// **The published HTTP offset**: protocol 40400 → HTTP 40403. The same block the relay reads for
/// gRPC (`+1`); see `deploy_relay_port` in `node_runtime.rs`.
pub const NODE_LIST_HTTP_PORT_OFFSET: u16 = 3;

/// The largest list the configuration may ask for. Kept in the code rather than the config so a
/// mistyped configuration cannot make one response a function of the network's size.
pub const HARD_MAX_ENTRIES: usize = 128;

/// Probes in flight at once. Bounds the egress a single cycle can generate.
const PROBE_CONCURRENCY: usize = 8;

/// Peers probed per cycle. Beyond this the candidate set is truncated deterministically by node id,
/// so a network larger than the bound gets a stable subset rather than a different one each cycle.
const MAX_PROBES_PER_CYCLE: usize = 128;

/// The largest status body the node will read from a peer.
const MAX_BODY_BYTES: usize = 64 * 1024;

const MAX_ADDRESS_BYTES: usize = 512;
const MAX_NETWORK_ID_BYTES: usize = 64;
const MAX_SHARD_ID_BYTES: usize = 128;
const MAX_NODE_VERSION_BYTES: usize = 128;
const MAX_NODE_ID_BYTES: usize = 512;

const SOURCE: LogSource = LogSource::new("coop.rchain.node.node-list");

/// The probe policy. Validated by `Configuration::build` and by `NodeListConfig::validate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeListConfig {
    pub probe_interval: Duration,
    pub probe_timeout: Duration,
    pub max_entries: usize,
}

impl NodeListConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.probe_interval < Duration::from_secs(1) {
            return Err(
                "api-server.node-list-probe-interval must be at least 1 second".to_string(),
            );
        }
        if self.probe_timeout.is_zero() {
            return Err("api-server.node-list-probe-timeout must be greater than zero".to_string());
        }
        if self.probe_timeout > self.probe_interval {
            return Err(
                "api-server.node-list-probe-timeout must not exceed the probe interval, or the \
                 next cycle starts before the last probe has given up"
                    .to_string(),
            );
        }
        if !(1..=HARD_MAX_ENTRIES).contains(&self.max_entries) {
            return Err(format!(
                "api-server.node-list-max-entries must be between 1 and {HARD_MAX_ENTRIES}"
            ));
        }
        Ok(())
    }
}

/// The `GET /api/v1/nodes` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeListResponse {
    pub network_id: String,
    pub primary_shard: String,
    /// When the cycle that produced this list completed, in unix milliseconds.
    pub generated_at: i64,
    /// The serving node's own height, so a client can rank a peer's `latestBlockNumber` against it.
    pub own_latest_block_number: i64,
    pub nodes: Vec<NodeListEntry>,
}

/// One deploy-capable node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeListEntry {
    /// The peer's TLS identity, hex — not its validator key.
    pub node_id: String,
    /// The URL this node actually probed. The only client endpoint served: a derived port that did
    /// not answer is not a claim worth publishing.
    pub http_url: String,
    pub network_id: String,
    pub shard_id: String,
    pub node_version: String,
    /// Peer-reported at probe time; advisory, and not a membership condition.
    pub latest_block_number: i64,
    /// Peer-reported; always `true` for a served entry (Law 90a).
    pub proposer: bool,
    /// Peer-reported pool capacity at probe time; advisory. A client must still handle a 429.
    pub accepts_deploys: bool,
    /// The serving node itself.
    #[serde(rename = "self")]
    pub is_self: bool,
}

/// The parts of a peer the list needs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PeerView {
    pub node_id: String,
    pub host: String,
    pub protocol_port: u16,
}

impl PeerView {
    pub fn from_peer(peer: &PeerNode) -> Self {
        PeerView {
            node_id: peer.id.s_key(),
            host: peer.endpoint.host.clone(),
            protocol_port: u16::from(peer.endpoint.tcp_port),
        }
    }

    /// The HTTP port this peer's client API is expected on. `None` on overflow rather than a wrap.
    pub fn http_port(&self) -> Option<u16> {
        self.protocol_port.checked_add(NODE_LIST_HTTP_PORT_OFFSET)
    }
}

/// The serving node, read fresh each cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnView {
    pub peer: PeerView,
    pub http_port: u16,
    pub network_id: String,
    pub shard_ids: Vec<String>,
    pub primary_shard: String,
    pub latest_block_number: i64,
    pub node_version: String,
    pub proposer: bool,
    pub accepts_deploys: bool,
}

impl OwnView {
    fn entry(&self) -> NodeListEntry {
        NodeListEntry {
            node_id: self.peer.node_id.clone(),
            http_url: http_url(&self.peer.host, self.http_port),
            network_id: self.network_id.clone(),
            shard_id: self.primary_shard.clone(),
            node_version: self.node_version.clone(),
            latest_block_number: self.latest_block_number,
            proposer: self.proposer,
            accepts_deploys: self.accepts_deploys,
            is_self: true,
        }
    }
}

/// What a peer's `/api/v1/status` said, after bounding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusAnswer {
    pub address: String,
    pub network_id: String,
    pub shard_id: String,
    pub proposer: bool,
    pub accepts_deploys: bool,
    pub latest_block_number: i64,
    pub node_version: String,
}

/// One peer's probe result. `answer: None` is a failed, refused or malformed probe — never "probably
/// fine".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeOutcome {
    pub peer: PeerView,
    pub answer: Option<StatusAnswer>,
}

/// **The pure derivation** — the function the Lean model's corpus checks.
///
/// Serves: self when it can propose, then the peers that answered with our network, one of our
/// shards, a `proposer` claim and an address naming *that* peer. Deduped by node id, sorted by node
/// id, truncated to `max_entries` (self included in the bound).
pub fn derive_hostlist(
    own: &OwnView,
    outcomes: &[ProbeOutcome],
    max_entries: usize,
) -> Vec<NodeListEntry> {
    let mut entries: BTreeMap<String, NodeListEntry> = BTreeMap::new();

    let shards: BTreeSet<&str> = own.shard_ids.iter().map(String::as_str).collect();
    for outcome in outcomes {
        // The id is the TLS identity, proven by the handshake, but its length is still the remote
        // end's and a served field is bounded here rather than trusted.
        if outcome.peer.node_id.len() > MAX_NODE_ID_BYTES
            || outcome.peer.host.len() > MAX_ADDRESS_BYTES
        {
            continue;
        }
        let Some(answer) = &outcome.answer else {
            continue;
        };
        if answer.network_id != own.network_id
            || !shards.contains(answer.shard_id.as_str())
            || !answer.proposer
            || !address_names_peer(&answer.address, &outcome.peer)
        {
            continue;
        }
        let Some(port) = outcome.peer.http_port() else {
            continue;
        };
        entries.insert(
            outcome.peer.node_id.clone(),
            NodeListEntry {
                node_id: outcome.peer.node_id.clone(),
                http_url: http_url(&outcome.peer.host, port),
                network_id: answer.network_id.clone(),
                shard_id: answer.shard_id.clone(),
                node_version: answer.node_version.clone(),
                latest_block_number: answer.latest_block_number,
                proposer: true,
                accepts_deploys: answer.accepts_deploys,
                is_self: false,
            },
        );
    }

    // Self wins over a connection that happens to name it.
    let self_entry = own.proposer.then(|| own.entry());
    if let Some(self_entry) = &self_entry {
        entries.remove(&self_entry.node_id);
    }

    let mut out = Vec::with_capacity(max_entries);
    if let Some(self_entry) = self_entry {
        if max_entries > 0 {
            out.push(self_entry);
        }
    }
    for entry in entries.into_values() {
        if out.len() >= max_entries {
            break;
        }
        out.push(entry);
    }
    out
}

/// The peer must answer as *itself*: `/api/v1/status` reports the node id hex, and the older
/// `rnode://` form is accepted too, so a version skew does not silently empty the list. Without this
/// check a co-hosted validator whose `protocol + 3` lands on its neighbour would be listed under the
/// wrong identity.
fn address_names_peer(address: &str, peer: &PeerView) -> bool {
    address == peer.node_id || address == peer_address(peer)
}

fn peer_address(peer: &PeerView) -> String {
    format!(
        "rnode://{}@{}?protocol={}&discovery={}",
        peer.node_id,
        peer.host,
        peer.protocol_port,
        // The discovery port is not part of identity; the address form only needs to be parseable
        // and to name the same node, so the protocol port stands in for it here.
        peer.protocol_port
    )
}

fn http_url(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("http://[{host}]:{port}")
    } else {
        format!("http://{host}:{port}")
    }
}

/// A peer's status answer as it arrives — bounded field by field in [`StatusAnswer::parse`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireStatus {
    address: String,
    network_id: String,
    shard_id: String,
    latest_block_number: i64,
    proposer: bool,
    accepts_deploys: bool,
    version: WireVersion,
}

#[derive(Deserialize)]
struct WireVersion {
    node: String,
}

impl StatusAnswer {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let wire: WireStatus = serde_json::from_slice(bytes).ok()?;
        if wire.address.len() > MAX_ADDRESS_BYTES
            || wire.network_id.len() > MAX_NETWORK_ID_BYTES
            || wire.shard_id.len() > MAX_SHARD_ID_BYTES
            || wire.version.node.len() > MAX_NODE_VERSION_BYTES
            || wire.latest_block_number < 0
        {
            return None;
        }
        Some(StatusAnswer {
            address: wire.address,
            network_id: wire.network_id,
            shard_id: wire.shard_id,
            proposer: wire.proposer,
            accepts_deploys: wire.accepts_deploys,
            latest_block_number: wire.latest_block_number,
            node_version: wire.version.node,
        })
    }
}

/// Resolve `host` to an address of the serving node's own class, or `None`.
///
/// Class equality is `check_peer_on_same_network`'s rule: a public node dials public peers, a devnet
/// dials its own. The resolved **IP** is returned and the caller dials that literal, not the name —
/// so a name that changes its answer between the check and the dial (AUDIT C117's residual) cannot
/// redirect this probe.
async fn resolve_dialable(host: &str, allow_local: bool) -> Option<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return (is_local_address(host) == allow_local).then_some(ip);
    }
    if is_local_address_resolved(host).await != allow_local {
        return None;
    }
    // A name that does not resolve is not a place to dial: the refusal is the decision, not a
    // fallback — the same fail-closed rule `is_local_address_resolved` applies one line up. Written
    // as an explicit match rather than `.ok()?` because the class the audit forbids is precisely
    // the silent discard of a resolution failure.
    let resolved = match tokio::net::lookup_host((host, 0)).await {
        Ok(addrs) => addrs,
        Err(_) => return None,
    };
    let mut addrs: Vec<IpAddr> = resolved
        .map(|a| a.ip())
        .filter(|ip| is_local_address(&ip.to_string()) == allow_local)
        .collect();
    addrs.sort();
    addrs.dedup();
    addrs.into_iter().next()
}

/// Probe one peer's derived HTTP endpoint. Every failure is an absent entry, never a served one.
pub async fn probe_peer(
    client: &reqwest::Client,
    peer: PeerView,
    allow_local: bool,
) -> ProbeOutcome {
    let failed = || ProbeOutcome {
        peer: peer.clone(),
        answer: None,
    };
    let Some(port) = peer.http_port() else {
        return failed();
    };
    let Some(ip) = resolve_dialable(&peer.host, allow_local).await else {
        return failed();
    };
    let url = format!("http://{ip}:{port}/api/v1/status");

    let Ok(mut response) = client.get(&url).send().await else {
        return failed();
    };
    if !response.status().is_success() {
        return failed();
    }
    let json = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !json {
        return failed();
    }

    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > MAX_BODY_BYTES {
                    return failed();
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return failed(),
        }
    }

    match StatusAnswer::parse(&body) {
        Some(answer) => ProbeOutcome {
            peer: peer.clone(),
            answer: Some(answer),
        },
        None => failed(),
    }
}

/// The probe client: no redirects (a peer must not be able to aim the probe), no proxy, bounded
/// connect and total time.
pub fn probe_client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .connect_timeout(timeout)
        .timeout(timeout)
        .user_agent(concat!("rchain-rnode/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("node-list probe client: {e}"))
}

/// The provider: one snapshot, swapped whole at the end of each cycle.
pub struct NodeListProvider {
    config: NodeListConfig,
    connections: ConnectionsCell,
    rp_conf: RPConf,
    block_api: Arc<dyn BlockApi>,
    shard_ids: Vec<String>,
    primary_shard: String,
    http_port: u16,
    client: reqwest::Client,
    snapshot: RwLock<Arc<NodeListResponse>>,
    log: Arc<dyn Log>,
}

impl NodeListProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: NodeListConfig,
        connections: ConnectionsCell,
        rp_conf: RPConf,
        block_api: Arc<dyn BlockApi>,
        shard_ids: Vec<String>,
        primary_shard: String,
        http_port: u16,
        log: Arc<dyn Log>,
    ) -> Result<Arc<Self>, String> {
        config.validate()?;
        let client = probe_client(config.probe_timeout)?;
        let local = PeerView::from_peer(&rp_conf.local);
        if local
            .host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_unspecified())
        {
            return Err(format!(
                "api-server.publish-node-list cannot derive a client URL for the unspecified \
                 host `{}`: set protocol-server.host to the address peers should dial",
                local.host
            ));
        }
        // A self-only snapshot before the first cycle: the route never serves a list that violates
        // "self first" because no cycle has run yet.
        let initial = Arc::new(NodeListResponse {
            network_id: rp_conf.network_id.clone(),
            primary_shard: primary_shard.clone(),
            generated_at: 0,
            own_latest_block_number: 0,
            nodes: Vec::new(),
        });
        Ok(Arc::new(NodeListProvider {
            config,
            connections,
            rp_conf,
            block_api,
            shard_ids,
            primary_shard,
            http_port,
            client,
            snapshot: RwLock::new(initial),
            log,
        }))
    }

    pub fn snapshot(&self) -> Arc<NodeListResponse> {
        self.snapshot.read().unpoison().clone()
    }

    /// Run the loop until the stop signal. The first cycle runs immediately, so a node that has just
    /// started publishes a real list without waiting a whole interval.
    pub fn spawn(self: &Arc<Self>, mut stop: watch::Receiver<bool>) {
        let provider = self.clone();
        rchain_shared::supervise::spawn_supervised("node-list", async move {
            loop {
                provider.run_cycle().await;
                tokio::select! {
                    _ = tokio::time::sleep(provider.config.probe_interval) => {}
                    _ = stop.wait_for(|stop| *stop) => return,
                }
            }
        });
    }

    /// One probe cycle. Public so tests drive it directly instead of waiting on the loop.
    pub async fn run_cycle(&self) -> Arc<NodeListResponse> {
        let own = self.own_view().await;

        // Snapshot and release: the connections lock is not held across any I/O.
        let mut candidates: Vec<PeerView> = {
            let connections = self.connections.read().await;
            connections.iter().map(PeerView::from_peer).collect()
        };
        candidates.sort();
        candidates.dedup();
        candidates.truncate(MAX_PROBES_PER_CYCLE);

        let allow_local = is_local_address_resolved(&self.rp_conf.local.endpoint.host).await;
        let outcomes: Vec<ProbeOutcome> = stream::iter(candidates)
            .map(|peer| probe_peer(&self.client, peer, allow_local))
            .buffer_unordered(PROBE_CONCURRENCY)
            .collect()
            .await;

        // "Served" means connected in the cycle that produced the list: a peer that dropped between
        // the snapshot and here is dropped with it.
        let still_connected: BTreeSet<String> = {
            let connections = self.connections.read().await;
            connections.iter().map(|p| p.id.s_key()).collect()
        };
        let outcomes: Vec<ProbeOutcome> = outcomes
            .into_iter()
            .filter(|outcome| still_connected.contains(&outcome.peer.node_id))
            .collect();

        for outcome in &outcomes {
            if outcome.answer.is_none() {
                self.log.debug(
                    SOURCE,
                    &format!(
                        "peer {} did not answer a node-list probe",
                        outcome.peer.node_id
                    ),
                );
            }
        }

        let nodes = derive_hostlist(&own, &outcomes, self.config.max_entries);
        let response = Arc::new(NodeListResponse {
            network_id: own.network_id.clone(),
            primary_shard: own.primary_shard.clone(),
            generated_at: current_millis(),
            own_latest_block_number: own.latest_block_number,
            nodes,
        });
        *self.snapshot.write().unpoison() = response.clone();
        response
    }

    async fn own_view(&self) -> OwnView {
        let status = self.block_api.status().await;
        let caps = self.block_api.capabilities().await;
        // The serving node's own capacity is **read**, not assumed: a failed read is `false`, which
        // says "cannot vouch" rather than advertising room this node may not have.
        let accepts_deploys = self
            .block_api
            .deploy_pool_stats()
            .await
            .map(|pool| pool.has_room())
            .unwrap_or(false);
        OwnView {
            peer: PeerView::from_peer(&self.rp_conf.local),
            http_port: self.http_port,
            network_id: self.rp_conf.network_id.clone(),
            shard_ids: self.shard_ids.clone(),
            primary_shard: self.primary_shard.clone(),
            latest_block_number: status.latest_block_number,
            node_version: status.version.node.clone(),
            proposer: caps.proposer,
            accepts_deploys,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: &str, host: &str, protocol: u16) -> PeerView {
        PeerView {
            node_id: id.to_string(),
            host: host.to_string(),
            protocol_port: protocol,
        }
    }

    fn answer(peer: &PeerView, network: &str, shard: &str, proposer: bool) -> ProbeOutcome {
        ProbeOutcome {
            peer: peer.clone(),
            answer: Some(StatusAnswer {
                address: peer.node_id.clone(),
                network_id: network.to_string(),
                shard_id: shard.to_string(),
                proposer,
                accepts_deploys: true,
                latest_block_number: 7,
                node_version: "v".to_string(),
            }),
        }
    }

    fn own(proposer: bool) -> OwnView {
        OwnView {
            peer: peer("aa", "127.0.0.1", 40400),
            http_port: 40403,
            network_id: "testnet".to_string(),
            shard_ids: vec!["/root".to_string()],
            primary_shard: "/root".to_string(),
            latest_block_number: 8,
            node_version: "v".to_string(),
            proposer,
            accepts_deploys: true,
        }
    }

    fn ids(entries: &[NodeListEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.node_id.as_str()).collect()
    }

    /// **Law 90a.** Dropping the `proposer` condition serves an observer that reports
    /// `manualPropose: true` — the exact ambiguity the `proposer` field exists to remove.
    #[test]
    fn an_observer_is_not_served() {
        let p = peer("bb", "10.0.0.2", 40400);
        let out = derive_hostlist(&own(true), &[answer(&p, "testnet", "/root", false)], 8);
        assert_eq!(ids(&out), vec!["aa"], "only self is served");
    }

    #[test]
    fn a_mismatched_network_is_not_served() {
        let p = peer("bb", "10.0.0.2", 40400);
        let out = derive_hostlist(&own(true), &[answer(&p, "othernet", "/root", true)], 8);
        assert_eq!(ids(&out), vec!["aa"]);
    }

    #[test]
    fn a_peer_on_another_shard_is_not_served() {
        let p = peer("bb", "10.0.0.2", 40400);
        let out = derive_hostlist(&own(true), &[answer(&p, "testnet", "/child", true)], 8);
        assert_eq!(ids(&out), vec!["aa"]);
    }

    #[test]
    fn an_unreachable_peer_is_not_served() {
        let p = peer("bb", "10.0.0.2", 40400);
        let out = derive_hostlist(
            &own(true),
            &[ProbeOutcome {
                peer: p,
                answer: None,
            }],
            8,
        );
        assert_eq!(ids(&out), vec!["aa"], "a failed probe is an absent entry");
    }

    /// A co-hosted validator whose `protocol + 3` lands on its neighbour answers as *that* node; the
    /// entry must not be published under the wrong identity.
    #[test]
    fn a_peer_answering_for_another_node_id_is_not_served() {
        let p = peer("bb", "10.0.0.2", 40400);
        let mut out = answer(&p, "testnet", "/root", true);
        out.answer.as_mut().unwrap().address = "cc".to_string();
        assert_eq!(ids(&derive_hostlist(&own(true), &[out], 8)), vec!["aa"]);
    }

    #[test]
    fn self_is_first_and_flagged() {
        let p = peer("bb", "10.0.0.2", 40400);
        let out = derive_hostlist(&own(true), &[answer(&p, "testnet", "/root", true)], 8);
        assert_eq!(ids(&out), vec!["aa", "bb"]);
        assert!(out[0].is_self);
        assert!(!out[1].is_self);
        assert_eq!(out[0].http_url, "http://127.0.0.1:40403");
    }

    #[test]
    fn self_is_absent_when_not_a_proposer() {
        let p = peer("bb", "10.0.0.2", 40400);
        let out = derive_hostlist(&own(false), &[answer(&p, "testnet", "/root", true)], 8);
        assert_eq!(ids(&out), vec!["bb"]);
    }

    #[test]
    fn the_list_is_sorted_and_bounded() {
        let b = peer("bb", "10.0.0.2", 40400);
        let c = peer("cc", "10.0.0.3", 40400);
        let d = peer("dd", "10.0.0.4", 40400);
        let outcomes = vec![
            answer(&d, "testnet", "/root", true),
            answer(&b, "testnet", "/root", true),
            answer(&c, "testnet", "/root", true),
        ];
        let out = derive_hostlist(&own(true), &outcomes, 3);
        assert_eq!(ids(&out), vec!["aa", "bb", "cc"], "self first, then by id");
        assert_eq!(
            ids(&derive_hostlist(&own(true), &outcomes, 2)),
            vec!["aa", "bb"],
            "the bound includes self"
        );
    }

    #[test]
    fn a_duplicate_connection_is_served_once() {
        let p = peer("bb", "10.0.0.2", 40400);
        let outcomes = vec![
            answer(&p, "testnet", "/root", true),
            answer(&p, "testnet", "/root", true),
        ];
        assert_eq!(
            ids(&derive_hostlist(&own(true), &outcomes, 8)),
            vec!["aa", "bb"]
        );
    }

    /// The derivation is the only thing that knows the port layout; an overflow must drop the peer,
    /// not wrap into a port that belongs to someone else.
    #[test]
    fn http_port_is_protocol_plus_three_and_overflow_drops() {
        assert_eq!(peer("a", "h", 40400).http_port(), Some(40403));
        assert_eq!(peer("a", "h", 41400).http_port(), Some(41403));
        assert_eq!(peer("a", "h", u16::MAX).http_port(), None);
    }

    #[test]
    fn a_v6_host_is_bracketed() {
        assert_eq!(http_url("::1", 40403), "http://[::1]:40403");
        assert_eq!(http_url("example.com", 40403), "http://example.com:40403");
    }

    #[test]
    fn an_overlong_field_is_dropped() {
        let body = format!(
            r#"{{"address":"aa","networkId":"{}","shardId":"/root","latestBlockNumber":1,"proposer":true,"acceptsDeploys":true,"version":{{"node":"v"}}}}"#,
            "n".repeat(MAX_NETWORK_ID_BYTES + 1)
        );
        assert!(StatusAnswer::parse(body.as_bytes()).is_none());
    }
}

/// Probe-level tests over a real socket: the answer is untrusted input, so the refusals are
/// exercised against a server rather than asserted about a parser in isolation.
#[cfg(test)]
mod probe_tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;

    async fn serve(router: Router) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        port
    }

    fn peer_on(port: u16, id: &str) -> PeerView {
        PeerView {
            node_id: id.to_string(),
            host: "127.0.0.1".to_string(),
            // `probe_peer` derives `port + 3`; undo it so the test server's port is the target.
            protocol_port: port - NODE_LIST_HTTP_PORT_OFFSET,
        }
    }

    fn status(proposer: bool) -> String {
        format!(
            r#"{{"address":"aa","networkId":"testnet","shardId":"/root","latestBlockNumber":3,"proposer":{proposer},"acceptsDeploys":true,"version":{{"node":"v"}}}}"#
        )
    }

    #[tokio::test]
    async fn a_valid_answer_is_accepted_and_an_observer_refused() {
        let port = serve(Router::new().route(
            "/api/v1/status",
            get(|| async {
                axum::Json(serde_json::from_str::<serde_json::Value>(&status(true)).unwrap())
            }),
        ))
        .await;
        let client = probe_client(Duration::from_secs(2)).unwrap();
        let out = probe_peer(&client, peer_on(port, "aa"), true).await;
        assert!(out.answer.as_ref().is_some_and(|a| a.proposer));

        let port = serve(Router::new().route(
            "/api/v1/status",
            get(|| async {
                axum::Json(serde_json::from_str::<serde_json::Value>(&status(false)).unwrap())
            }),
        ))
        .await;
        let out = probe_peer(&client, peer_on(port, "aa"), true).await;
        assert!(out.answer.is_some(), "the answer is parsed…");
        assert!(!out.answer.unwrap().proposer, "…and the filter sees false");
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused() {
        let big = "x".repeat(MAX_BODY_BYTES + 1);
        let port = serve(Router::new().route(
            "/api/v1/status",
            get(move || {
                let big = big.clone();
                async move { axum::Json(serde_json::json!({ "pad": big })) }
            }),
        ))
        .await;
        let client = probe_client(Duration::from_secs(2)).unwrap();
        assert!(probe_peer(&client, peer_on(port, "aa"), true)
            .await
            .answer
            .is_none());
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed() {
        let port = serve(Router::new().route(
            "/api/v1/status",
            get(|| async {
                axum::response::Redirect::temporary("http://127.0.0.1:1/api/v1/status")
            }),
        ))
        .await;
        let client = probe_client(Duration::from_secs(2)).unwrap();
        assert!(probe_peer(&client, peer_on(port, "aa"), true)
            .await
            .answer
            .is_none());
    }

    #[tokio::test]
    async fn a_non_json_answer_is_refused() {
        let port = serve(Router::new().route("/api/v1/status", get(|| async { "not json" }))).await;
        let client = probe_client(Duration::from_secs(2)).unwrap();
        assert!(probe_peer(&client, peer_on(port, "aa"), true)
            .await
            .answer
            .is_none());
    }

    /// A private host on a public node is never dialled (Law 62's rule). The refusal happens before
    /// the request, so no server is needed.
    #[tokio::test]
    async fn a_public_node_does_not_probe_a_private_peer() {
        let client = probe_client(Duration::from_secs(1)).unwrap();
        let out = probe_peer(&client, peer("aa", "10.0.0.2", 40400), false).await;
        assert!(out.answer.is_none());
        let out = probe_peer(&client, peer("aa", "127.0.0.1", 40400), false).await;
        assert!(out.answer.is_none());
    }

    fn peer(id: &str, host: &str, protocol: u16) -> PeerView {
        PeerView {
            node_id: id.to_string(),
            host: host.to_string(),
            protocol_port: protocol,
        }
    }
}
