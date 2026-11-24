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

use rchain_block_storage::checkpoint_store::{Adoption, CheckpointStore, Recorded};
use rchain_casper::api::block_api::BlockApi;
use rchain_casper::checkpoint::{
    check_bond_set_unchanged, checkpoint_digest, checkpoint_run_root,
    verify_genesis_rooted_certificate,
};
use rchain_casper::engine::checkpoint_source::CheckpointSource;
use rchain_casper::protocol::comm_util::CommUtil;
use rchain_casper::runtime_manager::RuntimeManager;
use rchain_comm::peer_node::PeerNode;
use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
use rchain_models::block::state_hash::StateHash;
use rchain_models::casper::protocol::checkpoint::{CheckpointAttestation, CheckpointCertificate};
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

/// **The production source**: an ordered list of bootstrap `/api/v1/checkpoint` URLs, on each
/// seed's client port. The first seed to answer is used; a down first seed falls through, which is
/// the same order the connection dialler follows.
pub struct HttpCheckpointSource {
    client: reqwest::Client,
    urls: Vec<String>,
}

impl HttpCheckpointSource {
    fn client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(ATTESTATION_TIMEOUT)
            .timeout(ATTESTATION_TIMEOUT)
            .user_agent(concat!("rchain-rnode/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| format!("checkpoint source client: {e}"))
    }

    /// An explicit client-API base URL (`casper.checkpoint-source`), for a bootstrap on a
    /// non-standard client port or a harness whose ports are assigned dynamically.
    pub fn from_url(base: &str) -> Result<Self, String> {
        Ok(HttpCheckpointSource {
            client: Self::client()?,
            urls: vec![format!("{}/api/v1/checkpoint", base.trim_end_matches('/'))],
        })
    }

    /// One bootstrap seed's HTTP endpoint: the protocol port plus the published offset — the same
    /// derivation the node list and the peer probe use.
    pub fn from_bootstrap(peer: &PeerNode) -> Result<Self, String> {
        Self::from_bootstraps(std::slice::from_ref(peer))
    }

    /// **The ordered seed list.** A down first seed falls through to the next; the URLs are kept
    /// in the authored order so the first live seed is the one that serves the certificate.
    pub fn from_bootstraps(peers: &[PeerNode]) -> Result<Self, String> {
        if peers.is_empty() {
            return Err("no bootstrap seed for the checkpoint source".to_string());
        }
        let mut urls = Vec::with_capacity(peers.len());
        for peer in peers {
            let port = u16::from(peer.endpoint.tcp_port)
                .checked_add(NODE_LIST_HTTP_PORT_OFFSET)
                .ok_or_else(|| {
                    format!(
                        "bootstrap {}'s client port overflows u16",
                        peer.endpoint.host
                    )
                })?;
            urls.push(format!(
                "http://{}:{}/api/v1/checkpoint",
                peer.endpoint.host, port
            ));
        }
        Ok(HttpCheckpointSource {
            client: Self::client()?,
            urls,
        })
    }
}

#[async_trait::async_trait]
impl CheckpointSource for HttpCheckpointSource {
    async fn fetch_certificate(&self) -> Result<CheckpointCertificate, String> {
        self.get_first(|url| url.to_string()).await
    }

    async fn fetch_certificate_at_anchor(
        &self,
        anchor: &str,
    ) -> Result<CheckpointCertificate, String> {
        // The anchor route is a sibling of the head route, on the same client port and with the
        // same body shape. A node that restarted behind its peers asks for the certificate that
        // anchors *its* accepted fringe rather than the head.
        self.get_first(|url| format!("{url}/anchor/{anchor}")).await
    }
}

impl HttpCheckpointSource {
    /// Try the seeds in order; the first success wins. With one seed the error string is exactly
    /// the single URL's error, so existing diagnostics do not change.
    async fn get_first<F: Fn(&str) -> String>(
        &self,
        path: F,
    ) -> Result<CheckpointCertificate, String> {
        let mut errors = Vec::with_capacity(self.urls.len());
        for base in &self.urls {
            match self.get_certificate(&path(base)).await {
                Ok(certificate) => return Ok(certificate),
                Err(error) => errors.push(error),
            }
        }
        Err(errors.join("; "))
    }

    async fn get_certificate(&self, url: &str) -> Result<CheckpointCertificate, String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("GET {url}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("GET {url}: status {}", response.status()));
        }
        let mut response = response;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("GET {url}: {e}"))?
        {
            if body.len() + chunk.len() > MAX_CERTIFICATE_BYTES {
                return Err(format!(
                    "GET {url}: the certificate exceeds the {MAX_CERTIFICATE_BYTES}-byte cap"
                ));
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body)
            .map_err(|e| format!("GET {url}: not a checkpoint certificate: {e}"))
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
        if let Some(stored) = self.store.get(&digest).await? {
            // **A record is not the head.** A crash between the sync's record and its later
            // adoption leaves a statement this node accepted but does not serve; adoption is
            // idempotent, so the first collector tick heals the head instead of waiting for the
            // statement to be re-derived and re-signed.
            match self.store.adopt(&digest, &stored, false).await? {
                Adoption::Adopted => self.log.info(
                    LogSource::new("coop.rchain.node.runtime.CheckpointCollector"),
                    &format!(
                        "{}: adopted the recorded checkpoint at anchor {} (digest {})",
                        self.shard_id,
                        stored.checkpoint.anchor_height,
                        digest.to_hex()
                    ),
                ),
                Adoption::Conflict {
                    head_digest,
                    incoming_digest,
                } => self.report_conflict(head_digest, incoming_digest),
                _ => {}
            }
            return Ok(None);
        }

        // **The trust anchor is the genesis bonded pool, not the genesis active draw** (C403): the
        // active set is a drawn subset that excludes the jailed and honours the active cap, so a
        // jail would look like a bond-set transition and no certificate could ever be certified.
        let trusted = self.runtime.genesis_pos().bonds.clone();
        self.refuse_if_bonds_drifted(&checkpoint, &trusted).await?;

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
        if let Recorded::Conflict { existing_digest } =
            self.store.record(&verified.digest, &certificate).await?
        {
            self.report_conflict(Some(existing_digest), verified.digest);
            return Ok(None);
        }
        match self
            .store
            .adopt(&verified.digest, &certificate, false)
            .await?
        {
            Adoption::Adopted | Adoption::AlreadyHead => Ok(Some(verified)),
            Adoption::Conflict {
                head_digest,
                incoming_digest,
            } => {
                self.report_conflict(head_digest, incoming_digest);
                Ok(None)
            }
            Adoption::Sibling { .. } | Adoption::Stale { .. } => Ok(None),
            Adoption::ForeignShard {
                head_shard,
                incoming_shard,
            } => Err(format!(
                "the collector's certificate names shard {incoming_shard}, but the served head is \
                 {head_shard}"
            )),
        }
    }

    /// **Two statements at one anchor are evidence, not a replacement.** Law 104 says their quorums
    /// share a signer; the store keeps both digest rows and the first anchor mapping, and the head
    /// stays on the statement this node already served.
    fn report_conflict(
        &self,
        head_digest: Option<Blake2b256Hash>,
        incoming_digest: Blake2b256Hash,
    ) {
        rchain_casper::engine::sync_metrics::census().add_checkpoint_conflict();
        self.log.warn(
            LogSource::new("coop.rchain.node.runtime.CheckpointCollector"),
            &format!(
                "{}: two checkpoint statements at one anchor (head {}, incoming {}); the head was \
                 not replaced",
                self.shard_id,
                head_digest
                    .map(|digest| digest.to_hex())
                    .unwrap_or_else(|| "unknown".to_string()),
                incoming_digest.to_hex()
            ),
        );
    }

    /// The certificate requires signatures carried by the bond set the verifier trusts. The pool is
    /// read at the certificate's **run root** — the signed merged fringe state — not at the
    /// anchor block's post-state. A node that cannot read that state (a joiner mid-sync) certifies
    /// nothing this tick.
    async fn refuse_if_bonds_drifted(
        &self,
        checkpoint: &rchain_models::casper::protocol::checkpoint::Checkpoint,
        trusted: &std::collections::BTreeMap<
            rchain_models::validator::Validator,
            rchain_shared::refined::NonNegI64,
        >,
    ) -> Result<(), String> {
        let root = checkpoint_run_root(checkpoint).map_err(|e| e.to_string())?;
        let state = StateHash::try_from(root.as_bytes().as_slice())
            .map_err(|e| format!("the checkpoint's fringe state is not a state root: {e}"))?;
        let at_fringe = self
            .runtime
            .compute_bonded_pool(&state)
            .await
            .map_err(|e| format!("the fringe state's bonded pool is unavailable: {e}"))?;
        check_bond_set_unchanged(trusted, &at_fringe).map_err(|e| e.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_comm::peer_node::NodeIdentifier;
    use rchain_shared::refined::Port;

    fn peer(id: u8, host: &str) -> PeerNode {
        PeerNode::from(
            NodeIdentifier::new(vec![id]),
            host.to_string(),
            Port::new(40400),
            Port::new(40404),
        )
    }

    /// **The seed URLs are the authored order.** The first live seed serves the certificate, so a
    /// reordering here would silently change which bootstrap a joiner trusts.
    #[test]
    fn the_seed_urls_are_kept_in_the_authored_order() {
        let source =
            HttpCheckpointSource::from_bootstraps(&[peer(1, "a.example"), peer(2, "b.example")])
                .expect("two seeds");
        assert_eq!(
            source.urls,
            vec![
                "http://a.example:40403/api/v1/checkpoint".to_string(),
                "http://b.example:40403/api/v1/checkpoint".to_string(),
            ]
        );
    }

    #[test]
    fn an_empty_seed_list_is_refused() {
        assert!(HttpCheckpointSource::from_bootstraps(&[]).is_err());
    }

    /// One seed is the old source exactly: the same URL string and the same single error.
    #[test]
    fn one_seed_keeps_the_single_url_form() {
        let from_seed = HttpCheckpointSource::from_bootstrap(&peer(1, "a.example")).unwrap();
        let from_url = HttpCheckpointSource::from_url("http://a.example:40403").unwrap();
        assert_eq!(from_seed.urls, from_url.urls);
    }

    /// **A down first seed must not stop the certificate fetch.** Both seeds are attempted, in
    /// order, and the joined error names them in that order.
    #[tokio::test]
    async fn a_down_first_seed_falls_through_to_the_second() {
        let source = HttpCheckpointSource {
            client: HttpCheckpointSource::client().expect("client"),
            urls: vec![
                "http://127.0.0.1:1/api/v1/checkpoint".to_string(),
                "http://127.0.0.1:2/api/v1/checkpoint".to_string(),
            ],
        };
        let error = source
            .fetch_certificate()
            .await
            .expect_err("no server is listening");
        let first = error.find("127.0.0.1:1").expect("the first error is kept");
        let second = error.find("127.0.0.1:2").expect("the second error is kept");
        assert!(
            first < second,
            "the errors follow the authored order: {error}"
        );
    }
}
