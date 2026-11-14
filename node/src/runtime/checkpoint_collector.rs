//! **The checkpoint-attestation collector** (sync/verification separation).
//!
//! A joiner accepts the finalized state by verifying a certificate: a checkpoint statement plus the
//! signatures of a supermajority of the bonded stake. Those signatures are produced by validators
//! signing **their own** current checkpoint at `GET /api/v1/checkpoint-attestation`; this task
//! gathers them from connected peers, groups them by digest, and persists the first certificate that
//! is quorate.
//!
//! Three properties decide the shape:
//!
//! * **The node signs only its own statement.** The route takes no digest, so the collector cannot
//!   turn a peer into a signing oracle. A peer that is one fringe ahead or behind signs a different
//!   digest; its attestation is dropped here rather than refused, because lag is ordinary.
//! * **The bond set is the trust anchor.** The current build produces genesis-rooted checkpoints, so
//!   signatures are weighed against the **genesis active bonds**. Before certifying, the collector
//!   reads the anchor state's active bonds and refuses a certificate if they differ: weighing a
//!   transition's signatures under the old partition would count the wrong stake. That refusal is a
//!   named log line, not a silently weaker quorum.
//! * **A failure is not fatal.** No peer answering, an unavailable state, or a short quorum all mean
//!   "not yet"; the loop tries again on the next tick. A certificate is written only after
//!   verification succeeds and is read back by the store.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use rchain_block_storage::checkpoint_store::CheckpointStore;
use rchain_casper::api::block_api::BlockApi;
use rchain_casper::checkpoint::{
    check_bond_set_unchanged, checkpoint_digest, verify_genesis_rooted_certificate,
};
use rchain_casper::engine::checkpoint_source::CheckpointSource;
use rchain_casper::protocol::comm_util::CommUtil;
use rchain_casper::runtime_manager::RuntimeManager;
use rchain_comm::peer_node::PeerNode;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block::state_hash::StateHash;
use rchain_models::casper::protocol::checkpoint::{CheckpointAttestation, CheckpointCertificate};
use rchain_shared::base16;
use rchain_shared::log::{Log, LogSource};

use crate::web::node_list::NODE_LIST_HTTP_PORT_OFFSET;

/// How often the collector looks for a new certificate. Shorter than a finality round so two
/// consecutive ticks can observe the same finalized fringe on peers that agree.
const COLLECT_INTERVAL: Duration = Duration::from_secs(2);

/// Per-request bound. An attestation is a few hundred bytes; a peer that does not answer promptly
/// is absent for this tick, not a stall in the loop.
const ATTESTATION_TIMEOUT: Duration = Duration::from_secs(2);

/// The largest set of peers queried in one tick. The quorum is reached by stake, not by breadth, so
/// a bound costs nothing and keeps one tick's fan-out independent of the connection count.
const MAX_ATTESTATION_PEERS: usize = 32;

/// The body cap for one attestation response.
const MAX_ATTESTATION_BYTES: usize = 8 * 1024;

/// The body cap for one certificate response. A certificate carries one attestation per bonded
/// validator; the cap is generous and still independent of what a peer chooses to send.
const MAX_CERTIFICATE_BYTES: usize = 1024 * 1024;

/// **The production source**: the bootstrap's `/api/v1/checkpoint`, on its client port.
pub struct HttpCheckpointSource {
    client: reqwest::Client,
    url: String,
}

impl HttpCheckpointSource {
    /// The bootstrap's HTTP endpoint is the protocol port plus the published offset — the same
    /// derivation the node list and the peer probe use, so a node published there is the one asked.
    pub fn from_bootstrap(peer: &PeerNode) -> Result<Self, String> {
        let port = u16::from(peer.endpoint.tcp_port)
            .checked_add(NODE_LIST_HTTP_PORT_OFFSET)
            .ok_or_else(|| {
                format!(
                    "bootstrap {}'s client port overflows u16",
                    peer.endpoint.host
                )
            })?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(ATTESTATION_TIMEOUT)
            .timeout(ATTESTATION_TIMEOUT)
            .user_agent(concat!("rchain-rnode/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("checkpoint source client: {e}"))?;
        Ok(HttpCheckpointSource {
            client,
            url: format!("http://{}:{}/api/v1/checkpoint", peer.endpoint.host, port),
        })
    }
}

#[async_trait::async_trait]
impl CheckpointSource for HttpCheckpointSource {
    async fn fetch_certificate(&self) -> Result<CheckpointCertificate, String> {
        let response = self
            .client
            .get(&self.url)
            .send()
            .await
            .map_err(|e| format!("GET {}: {e}", self.url))?;
        if !response.status().is_success() {
            return Err(format!("GET {}: status {}", self.url, response.status()));
        }
        let mut response = response;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("GET {}: {e}", self.url))?
        {
            if body.len() + chunk.len() > MAX_CERTIFICATE_BYTES {
                return Err(format!(
                    "GET {}: the certificate exceeds the {MAX_CERTIFICATE_BYTES}-byte cap",
                    self.url
                ));
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body)
            .map_err(|e| format!("GET {}: not a checkpoint certificate: {e}", self.url))
    }
}

pub struct CheckpointCollector {
    block_api: Arc<dyn BlockApi>,
    store: CheckpointStore,
    runtime: Arc<RuntimeManager>,
    comm_util: Arc<CommUtil>,
    log: Arc<dyn Log>,
    client: reqwest::Client,
    shard_id: String,
}

impl CheckpointCollector {
    pub fn new(
        block_api: Arc<dyn BlockApi>,
        store: CheckpointStore,
        runtime: Arc<RuntimeManager>,
        comm_util: Arc<CommUtil>,
        log: Arc<dyn Log>,
        shard_id: String,
    ) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(ATTESTATION_TIMEOUT)
            .timeout(ATTESTATION_TIMEOUT)
            .user_agent(concat!("rchain-rnode/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("checkpoint collector client: {e}"))?;
        Ok(CheckpointCollector {
            block_api,
            store,
            runtime,
            comm_util,
            log,
            client,
            shard_id,
        })
    }

    /// Spawn the loop. It lives as long as the node; a tick that fails logs and returns.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut last_logged: Option<Blake2b256Hash> = None;
            loop {
                match self.collect_once().await {
                    Ok(Some(verified)) => {
                        if last_logged != Some(verified.digest) {
                            last_logged = Some(verified.digest);
                            self.log.info(
                                LogSource::new("coop.rchain.node.runtime.CheckpointCollector"),
                                &format!(
                                    "{}: checkpoint certificate accepted at anchor {} \
                                     ({} of {} stake signed, digest {})",
                                    self.shard_id,
                                    verified.checkpoint.anchor_height,
                                    verified.supporting_stake,
                                    verified.total_stake,
                                    verified.digest.to_hex()
                                ),
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(e) => self.log.warn(
                        LogSource::new("coop.rchain.node.runtime.CheckpointCollector"),
                        &format!(
                            "{}: no checkpoint certificate this tick: {e}",
                            self.shard_id
                        ),
                    ),
                }
                tokio::time::sleep(COLLECT_INTERVAL).await;
            }
        })
    }

    /// One tick: compute this node's current checkpoint, gather attestations, verify, persist.
    /// `Ok(None)` means there is nothing new to certify (no checkpoint yet, or already certified).
    pub async fn collect_once(
        &self,
    ) -> Result<Option<rchain_casper::checkpoint::VerifiedCheckpoint>, String> {
        let checkpoint = self.block_api.current_checkpoint().await?;
        let digest =
            checkpoint_digest(&checkpoint).map_err(|e| format!("current checkpoint: {e}"))?;
        if self.store.get(&digest).await?.is_some() {
            return Ok(None);
        }

        let trusted = self.runtime.genesis_pos().active_bonds();
        self.refuse_if_bonds_drifted(&checkpoint.anchor_post_state, &trusted)
            .await?;

        let mut attestations = Vec::new();
        if let Ok(own) = self.block_api.checkpoint_attestation().await {
            attestations.push(own);
        }
        attestations.extend(self.fetch_attestations(&digest).await);
        if attestations.is_empty() {
            return Err("no attestations available".to_string());
        }
        // A peer on another fringe signs another digest. Keep only signatures over *this*
        // statement; the verifier would refuse the whole certificate for a foreign digest.
        attestations.retain(|a| a.checkpoint_digest.eq_ignore_ascii_case(&digest.to_hex()));

        let certificate = CheckpointCertificate {
            checkpoint,
            attestations,
        };
        let verified =
            verify_genesis_rooted_certificate(&certificate, &trusted).map_err(|e| e.to_string())?;
        self.store.put(&verified.digest, &certificate).await?;
        Ok(Some(verified))
    }

    /// The certificate requires signatures carried by the bond set the verifier trusts. A node that
    /// cannot read the anchor state (a joiner mid-sync) certifies nothing this tick.
    async fn refuse_if_bonds_drifted(
        &self,
        anchor_post_state: &str,
        trusted: &std::collections::BTreeMap<
            rchain_models::validator::Validator,
            rchain_shared::refined::NonNegI64,
        >,
    ) -> Result<(), String> {
        let bytes = base16::decode(anchor_post_state)
            .ok_or_else(|| "anchor post-state is not hex".to_string())?;
        let state = StateHash::try_from(bytes.as_slice())
            .map_err(|e| format!("anchor post-state is not a state root: {e}"))?;
        let at_anchor = self
            .runtime
            .compute_bonds(&state)
            .await
            .map_err(|e| format!("the anchor state's bonds are unavailable: {e}"))?;
        check_bond_set_unchanged(trusted, &at_anchor).map_err(|e| e.to_string())
    }

    /// Fetch each connected peer's attestation for its own current checkpoint. A peer that is
    /// unreachable, slow, or malformed is absent, never an error for the tick.
    async fn fetch_attestations(&self, digest: &Blake2b256Hash) -> Vec<CheckpointAttestation> {
        let peers = self.comm_util.connected_peers().await;
        let mut seen = BTreeSet::new();
        let mut attestations = Vec::new();
        for peer in peers.into_iter().take(MAX_ATTESTATION_PEERS) {
            let Some(port) =
                u16::from(peer.endpoint.tcp_port).checked_add(NODE_LIST_HTTP_PORT_OFFSET)
            else {
                continue;
            };
            if !seen.insert((peer.endpoint.host.clone(), port)) {
                continue;
            }
            let url = format!(
                "http://{}:{}/api/v1/checkpoint-attestation",
                peer.endpoint.host, port
            );
            let Ok(response) = self.client.get(&url).send().await else {
                continue;
            };
            if !response.status().is_success() {
                continue;
            }
            let Ok(body) = response.bytes().await else {
                continue;
            };
            if body.len() > MAX_ATTESTATION_BYTES {
                continue;
            }
            let Ok(attestation) = serde_json::from_slice::<CheckpointAttestation>(&body) else {
                continue;
            };
            if attestation
                .checkpoint_digest
                .eq_ignore_ascii_case(&digest.to_hex())
            {
                attestations.push(attestation);
            }
        }
        attestations
    }
}
