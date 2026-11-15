//! **`rnode audit`** — replay a checkpoint's history from genesis and report a verdict.
//!
//! Sync accepts a checkpoint's state on a quorum's word. The audit is the independent counterweight:
//! it downloads the **bodies**, replays them through the validator from genesis in a staging DAG,
//! and compares the state it derives with the state the checkpoint claimed.
//!
//! Three verdicts, and the difference between them is the point:
//!
//! * `AGREE` — the replay derived the checkpoint's root. The quorum's claim is reproducible here.
//! * `DISAGREE` — the replay ran and derived something else. This is an **alarm** (the same class
//!   the 2,432-height campaign measured as a mergeable-channel root mismatch), never a sync block.
//! * `INCOMPLETE` — the audit could not obtain the history. Missing evidence is not agreement.
//!
//! It never writes to the live store: the replay runs against a scratch data directory the caller
//! names, through `setup_shard`, and its staging DAG is the replay's own. `--out` receives an
//! `audit.json` the operator can pack as evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::diagnostics::effects::MetricsRegistry;
use rchain_block_storage::block_store::{self, BlockStore};
use rchain_casper::api::block_api::ProposeHealth;
use rchain_casper::audit::{verify_audit_attestation, AuditExpectation};
use rchain_casper::engine::recovery::RecoveryReplay;
use rchain_casper::interpreter_util::ShardHealth;
use rchain_casper::protocol::comm_util::ConnectionsCell;
use rchain_casper::validator_identity::ValidatorIdentity;
use rchain_comm::discovery::node_discovery::NodeDiscovery;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::audit::{AuditAttestation, SIGNED_HASH_CHAIN_WITNESS};
use rchain_models::casper::protocol::casper_message::{BlockHeader, FinalizedFringe};
use rchain_models::casper::protocol::checkpoint::CheckpointCertificate;
use rchain_models::casper::protocol::deploy_service::{LightBlockInfo, RawBlock};
use rchain_shared::base16;
use rchain_shared::log::{Log, LogSource};
use rchain_shared::store_manager::InMemoryStoreManager;
use serde::{Deserialize, Serialize};

use crate::configuration::model::NodeConf;

/// One audit run's inputs.
#[derive(Clone, Debug)]
pub struct AuditRequest {
    /// The node to read the certificate and bodies from, as `http://host:port` (its client API).
    pub bootstrap: String,
    /// The checkpoint digest to audit; `None` audits whatever certificate the node serves.
    pub checkpoint: Option<String>,
    /// A scratch directory for the replay's runtime. Must not be a live node's data directory.
    pub data_dir: PathBuf,
    /// Where `audit.json` is written.
    pub out: PathBuf,
}

/// The verdict, as written to `audit.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditReport {
    /// `AGREE`, `DISAGREE` or `INCOMPLETE`.
    pub verdict: String,
    pub checkpoint_digest: String,
    pub anchor: String,
    pub anchor_height: i64,
    /// The shard the audited certificate names.
    #[serde(default)]
    pub shard_id: String,
    /// The state root the certificate claimed.
    pub claimed_root: String,
    /// The certificate's metadata commitment; `None` on a legacy certificate.
    #[serde(default)]
    pub metadata_root: Option<String>,
    /// The certificate's validation-window commitment; `None` on a legacy certificate.
    #[serde(default)]
    pub history_root: Option<String>,
    /// The root the replay derived, when it ran far enough to have one.
    pub derived_root: Option<String>,
    /// The replay's cumulative head over the anchor's canonical ancestor closure (Law 112). The same
    /// value an attestation signs; `None` when the closure could not be built.
    #[serde(default)]
    pub replay_head: Option<String>,
    /// How many bodies the replay consumed.
    pub blocks: usize,
    /// **Whether the certificate committed to the historical metadata.** A legacy `None` root is
    /// accepted by the audit (with `metadata_committed: false`) because the replay still checks the
    /// state; a reader must not mistake that for an authenticated header set.
    pub metadata_committed: bool,
    /// **Whether the certificate committed to the validation window.** A legacy `None` is accepted
    /// by the audit — the replay still checks the state — and reported as uncommitted; the sync path
    /// refuses it.
    pub window_committed: bool,
    /// Why the verdict is not AGREE.
    pub reason: Option<String>,
}

/// The heights a completed audit needs: `0..=anchor_height`.
fn required_heights(anchor_height: i64) -> std::ops::RangeInclusive<i64> {
    0..=anchor_height
}

/// Heights one audit body request asks for. The history range caps a response at 64 heights; the
/// per-height fallback uses the same window so the two paths are interchangeable.
const MAX_AUDIT_BODY_BATCH: i64 = 64;

/// The closed height windows an audit fetches, in order: `0..=anchor`, 64 heights at a time.
fn audit_batches(last_height: i64) -> Vec<(i64, i64)> {
    let mut batches = Vec::new();
    let mut height = 0i64;
    while height <= last_height {
        let to = (height + MAX_AUDIT_BODY_BATCH - 1).min(last_height);
        batches.push((height, to));
        height = to + 1;
    }
    batches
}

/// The bodies for `[from, to]` from the history service, when the node serves one complete range.
/// `None` means "use the per-height route": an older node, a refusal, or a gap. The range is a
/// transport optimization; every body is reconstructed and re-hashed by the caller.
async fn fetch_history_range(
    client: &reqwest::Client,
    base: &str,
    from: i64,
    to: i64,
) -> Option<BTreeMap<i64, Vec<RawBlock>>> {
    let response = client
        .post(format!("{base}/api/v1/history/range"))
        .json(&serde_json::json!({
            "from": from,
            "to": to,
            "authorization": serde_json::Value::Null,
        }))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let range: crate::api::dto::HistoryRange = response.json().await.ok()?;
    if !range.complete {
        return None;
    }
    let mut by_height: BTreeMap<i64, Vec<RawBlock>> = BTreeMap::new();
    for entry in range.entries {
        if let Some(body) = entry.body {
            by_height.entry(entry.height).or_default().push(body);
        }
    }
    if (from..=to).any(|height| !by_height.contains_key(&height)) {
        return None;
    }
    Some(by_height)
}

/// One height's bodies via the per-height route: the block list at the height, then each body.
async fn fetch_height_bodies(
    client: &reqwest::Client,
    base: &str,
    height: i64,
) -> Result<Vec<RawBlock>, String> {
    let infos: Vec<LightBlockInfo> =
        get_json(client, &format!("{base}/api/v1/blocks/{height}/{height}"))
            .await
            .map_err(|e| format!("height {height}: {e}"))?;
    let mut bodies = Vec::new();
    for info in infos {
        bodies.push(
            get_json::<RawBlock>(
                client,
                &format!("{base}/api/v1/block/{}/raw", info.block_hash),
            )
            .await
            .map_err(|e| format!("body {}: {e}", info.block_hash))?,
        );
    }
    Ok(bodies)
}

/// Record one height's bodies: reconstruct and re-hash each, check its height, build the header the
/// certificate commits to, and store the body. An empty set is INCOMPLETE, not agreement.
async fn record_height(
    store: &BlockStore,
    height: i64,
    raws: Vec<RawBlock>,
    headers: &mut Vec<BlockHeader>,
    heights: &mut BTreeMap<i64, BTreeSet<BlockHash>>,
) -> Result<(), String> {
    let mut at_height = BTreeSet::new();
    for raw in raws {
        let block = reconstruct_block(&raw)?;
        if i64::from(block.block_number) != height {
            return Err(format!(
                "body {} is at height {}",
                block.block_hash.to_hex(),
                block.block_number
            ));
        }
        headers.push(BlockHeader {
            block_hash: block.block_hash,
            block_num: block.block_number,
            sender: block.sender,
            seq_num: block.seq_num,
            justifications: block.justifications.iter().copied().collect(),
            bonds_map: block.bonds.clone(),
            fringe: BTreeSet::new(),
            fringe_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            pre_state_hash: block.pre_state_hash,
            post_state_hash: block.post_state_hash,
            body_available: true,
            sig: block.sig.clone(),
            sig_algorithm: block.sig_algorithm.clone(),
        });
        store
            .put(&[(block.block_hash, block.clone())])
            .await
            .map_err(|e| format!("the staging block store refused a body: {e}"))?;
        at_height.insert(block.block_hash);
    }
    if at_height.is_empty() {
        return Err(format!("no block at height {height}"));
    }
    heights.insert(height, at_height);
    Ok(())
}

/// **The anchor's transitive justification closure**, from the bodies an audit downloaded. A block
/// that merely shares a height with a certified ancestor is not part of the certified metadata set;
/// a root over every body at every height would falsely disagree with a correct certificate. The
/// missing hash is returned when an ancestor's body is absent — that is INCOMPLETE, not DISAGREE.
fn ancestor_closure(
    anchor: BlockHash,
    headers: &[rchain_models::casper::protocol::casper_message::BlockHeader],
) -> Result<Vec<rchain_models::casper::protocol::casper_message::BlockHeader>, BlockHash> {
    let by_hash: BTreeMap<
        BlockHash,
        &rchain_models::casper::protocol::casper_message::BlockHeader,
    > = headers
        .iter()
        .map(|header| (header.block_hash, header))
        .collect();
    let mut closure = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![anchor];
    while let Some(hash) = stack.pop() {
        if !seen.insert(hash) {
            continue;
        }
        let Some(header) = by_hash.get(&hash) else {
            return Err(hash);
        };
        closure.push((*header).clone());
        stack.extend(header.justifications.iter().copied());
    }
    Ok(closure)
}

/// **Reconstruct a block from a node's `RawBlock` response.** The implementation lives in
/// [`crate::api::raw_block`], shared with the history service; the re-export keeps the audit's
/// callers (and its own tests) naming the operation they use.
pub use crate::api::raw_block::reconstruct_block;

/// Run the audit. The caller prints the verdict and exits by it.
pub async fn run(
    conf: &NodeConf,
    request: &AuditRequest,
    log: &Arc<dyn Log>,
) -> Result<AuditReport, String> {
    let source = LogSource::new("coop.rchain.node.runtime.Audit");
    let base = request.bootstrap.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("rchain-rnode/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("audit client: {e}"))?;

    // **A named digest is fetched by digest**, not compared against whatever the node currently
    // serves: an audit of a historical certificate must still work after the head has moved. The
    // digest self-check below stays: the returned statement must hash to the requested key.
    let certificate_url = match &request.checkpoint {
        Some(digest) => format!("{base}/api/v1/checkpoint/{digest}"),
        None => format!("{base}/api/v1/checkpoint"),
    };
    let certificate: CheckpointCertificate = get_json(&client, &certificate_url)
        .await
        .map_err(|e| format!("the checkpoint certificate was not fetched: {e}"))?;
    let digest = rchain_casper::checkpoint::checkpoint_digest(&certificate.checkpoint)
        .map_err(|e| format!("the served checkpoint is malformed: {e}"))?;
    if let Some(expected) = &request.checkpoint {
        if !expected.eq_ignore_ascii_case(&digest.to_hex()) {
            return Err(format!(
                "the node serves checkpoint {}, not the requested {expected}",
                digest.to_hex()
            ));
        }
    }
    let anchor = BlockHash::try_from(
        base16::decode(&certificate.checkpoint.anchor)
            .ok_or_else(|| "the checkpoint anchor is not base16".to_string())?
            .as_slice(),
    )
    .map_err(|e| format!("the checkpoint anchor is not a hash: {e}"))?;

    // **The replay's runtime, in the caller's scratch directory.** `setup_shard` installs the
    // genesis descriptors this replay starts from, exactly as a node would; the audit never touches
    // the live store because the data directory is the caller's scratch path.
    let mut scratch = conf.clone();
    scratch.storage.data_dir = request.data_dir.clone();
    scratch.casper.validator_private_key = None;
    scratch.casper.validator_private_key_path = None;
    let spec = scratch.casper.shards.primary().clone();
    let parts = crate::runtime::node_runtime::setup_shard(
        &scratch,
        &spec,
        0,
        &rchain_comm::peer_node::NodeIdentifier::new(vec![0xA0u8; 32]),
        Arc::new(tokio::sync::RwLock::new(Vec::new())) as ConnectionsCell,
        Arc::new(StubDiscovery),
        None,
        Arc::new(MetricsRegistry::new()),
        ProposeHealth::new(),
        ShardHealth::new(),
        log.as_ref(),
    )
    .await
    .map_err(|e| format!("the audit runtime could not be built: {e}"))?;

    let verified = rchain_casper::checkpoint::verify_genesis_rooted_certificate(
        &certificate,
        &parts.runtime_manager.genesis_pos().active_bonds(),
    )
    .map_err(|e| format!("the checkpoint certificate was refused: {e}"))?;

    // **The bodies, by height.** Every height `0..=anchor` must be present; a missing one is
    // INCOMPLETE, not a disagreement — the audit has no evidence to disagree with.
    let store: BlockStore = {
        let manager = InMemoryStoreManager::default();
        block_store::create(&manager)
            .await
            .map_err(|e| format!("the staging block store could not be opened: {e}"))?
    };
    let mut heights: BTreeMap<i64, BTreeSet<BlockHash>> = BTreeMap::new();
    // The headers the certificate committed to, reconstructed from the bodies it serves. The
    // projection ignores `fringe`/`fringe_state_hash` (absent from a body); the replay bounds the
    // state those fields describe.
    let mut headers: Vec<BlockHeader> = Vec::new();
    // **Bodies in bounded batches.** A node that serves the history range answers a batch in one
    // request; any other node, refusal or gap falls back to the per-height route. Either way every
    // body is reconstructed and re-hashed, and a batch is processed before the next is fetched, so
    // the audit never holds the whole chain in memory.
    let last_height = *required_heights(certificate.checkpoint.anchor_height).end();
    for (height, to) in audit_batches(last_height) {
        match fetch_history_range(&client, base, height, to).await {
            Some(mut bodies) => {
                for at in height..=to {
                    let raws = bodies.remove(&at).unwrap_or_default();
                    if let Err(reason) =
                        record_height(&store, at, raws, &mut headers, &mut heights).await
                    {
                        return Ok(incomplete(&verified, at, reason));
                    }
                }
            }
            None => {
                for at in height..=to {
                    match fetch_height_bodies(&client, base, at).await {
                        Ok(raws) => {
                            if let Err(reason) =
                                record_height(&store, at, raws, &mut headers, &mut heights).await
                            {
                                return Ok(incomplete(&verified, at, reason));
                            }
                        }
                        Err(reason) => return Ok(incomplete(&verified, at, reason)),
                    }
                }
            }
        }
    }

    // **The replay head** (Law 112): the cumulative hash over the anchor's canonical ancestor
    // closure, sorted by `(height, hash)`, seeded by the checkpoint digest. It is the value an
    // attestation signs; a missing ancestor is not an error here (the report says `None`).
    let replay_head = ancestor_closure(anchor, &headers).ok().map(|closure| {
        base16::encode(&rchain_models::casper::protocol::audit::replay_head(
            verified.digest.as_bytes(),
            closure.iter().map(
                |header| rchain_models::casper::protocol::audit::ReplayStep {
                    block_hash: header.block_hash,
                    height: i64::from(header.block_num),
                    post_state_hash: header.post_state_hash,
                },
            ),
        ))
    });

    // **The replay starts from the genesis post-state** (Law 102's one external state). The genesis
    // body carries its own blessed deploys, so re-executing them installs exactly that state under
    // the same root — no ceremony key is needed, and an auditor that never held the genesis
    // master's key can still reproduce the chain. The derived root must equal the body's claimed
    // post-state; a mismatch means this node's genesis descriptors are not the chain's, which is
    // missing/foreign input, not a divergence.
    let genesis_hash = heights
        .get(&0)
        .and_then(|set| set.iter().next().copied())
        .ok_or_else(|| "the audit downloaded no height-zero block".to_string())?;
    let genesis_body = store
        .get(&[genesis_hash])
        .await?
        .into_iter()
        .flatten()
        .next()
        .ok_or_else(|| "the genesis body was recorded but cannot be read".to_string())?;
    let terms: Vec<_> = genesis_body
        .state
        .deploys
        .iter()
        .map(|processed| processed.deploy.clone())
        .collect();
    let genesis_random =
        rchain_casper::block_random_seed::BlockRandomSeed::random_generator_from_shard_id(
            &genesis_body.shard_id,
        )
        .map_err(|e| format!("the genesis shard id has no random seed: {e}"))?;
    let genesis_data = rchain_rholang::system_processes::BlockData {
        block_number: genesis_body.block_number,
        sender: rchain_crypto::public_key::PublicKey::new(genesis_body.sender.as_bytes().to_vec()),
        seq_num: genesis_body.seq_num,
        timestamp: genesis_body.timestamp,
    };
    match parts
        .runtime_manager
        .compute_genesis(
            &terms,
            &genesis_random,
            genesis_data,
            parts.runtime_manager.genesis_pos(),
            parts.runtime_manager.genesis_vaults(),
        )
        .await
    {
        Ok((_pre, derived_genesis_root, _results)) => {
            if derived_genesis_root.as_bytes() != genesis_body.post_state_hash.as_bytes() {
                return Ok(incomplete(
                    &verified,
                    0,
                    format!(
                        "the genesis body's deploys derive {}, but the body claims {}; this node \
                         cannot establish the chain's genesis post-state",
                        base16::encode(derived_genesis_root.as_bytes()),
                        base16::encode(genesis_body.post_state_hash.as_bytes())
                    ),
                ));
            }
        }
        Err(e) => {
            return Ok(incomplete(
                &verified,
                0,
                format!(
                    "the genesis post-state could not be executed ({e}); the audit has no evidence \
                     to disagree with"
                ),
            ));
        }
    }

    // **The certificate's commitment to the metadata the replay consumed.** A mismatch is a
    // DISAGREE: the bodies this node served are not the headers the quorum signed over. A legacy
    // `None` root is accepted — the replay still checks the state — and reported as uncommitted, so
    // the report never implies an authentication that did not happen.
    let metadata_committed = certificate.checkpoint.metadata_root.is_some();
    let window_committed = certificate.checkpoint.history_root.is_some();
    if metadata_committed || window_committed {
        // **The commitments cover the anchor's ancestor closure, not every block at a height.**
        // The bodies route returns all blocks at a height, including forks; a root over that
        // superset would disagree with a correct certificate. Walk the anchor's justifications,
        // exactly as the producer and the joiner do.
        let closure = match ancestor_closure(anchor, &headers) {
            Ok(closure) => closure,
            Err(hash) => {
                let report = incomplete(
                    &verified,
                    certificate.checkpoint.anchor_height,
                    format!("the anchor's ancestor {} has no body", hash.to_hex()),
                );
                write_report(&request.out, &report).await?;
                return Ok(report);
            }
        };
        if metadata_committed {
            if let Err(e) = rchain_casper::checkpoint::verify_metadata_root(
                &certificate.checkpoint,
                closure.iter(),
            ) {
                let report = AuditReport {
                    verdict: "DISAGREE".to_string(),
                    checkpoint_digest: verified.digest.to_hex(),
                    anchor: anchor.to_hex(),
                    anchor_height: certificate.checkpoint.anchor_height,
                    shard_id: certificate.checkpoint.shard_id.clone(),
                    claimed_root: certificate.checkpoint.anchor_post_state.clone(),
                    metadata_root: certificate.checkpoint.metadata_root.clone(),
                    history_root: certificate.checkpoint.history_root.clone(),
                    derived_root: None,
                    replay_head: replay_head.clone(),
                    blocks: heights.values().map(|set| set.len()).sum(),
                    metadata_committed,
                    window_committed,
                    reason: Some(format!(
                        "the metadata commitment does not match the bodies: {e}"
                    )),
                };
                write_report(&request.out, &report).await?;
                return Ok(report);
            }
        }
        if window_committed {
            if let Err(e) = rchain_casper::checkpoint::verify_validation_window(
                &certificate.checkpoint,
                closure.iter(),
            ) {
                let report = AuditReport {
                    verdict: "DISAGREE".to_string(),
                    checkpoint_digest: verified.digest.to_hex(),
                    anchor: anchor.to_hex(),
                    anchor_height: certificate.checkpoint.anchor_height,
                    shard_id: certificate.checkpoint.shard_id.clone(),
                    claimed_root: certificate.checkpoint.anchor_post_state.clone(),
                    metadata_root: certificate.checkpoint.metadata_root.clone(),
                    history_root: certificate.checkpoint.history_root.clone(),
                    derived_root: None,
                    replay_head: replay_head.clone(),
                    blocks: heights.values().map(|set| set.len()).sum(),
                    metadata_committed,
                    window_committed,
                    reason: Some(format!(
                        "the validation-window commitment does not match the bodies: {e}"
                    )),
                };
                write_report(&request.out, &report).await?;
                return Ok(report);
            }
        }
    }

    let fringe_hashes: Vec<BlockHash> = certificate
        .checkpoint
        .fringe_hashes
        .iter()
        .map(|hex| {
            let bytes = base16::decode(hex)
                .ok_or_else(|| format!("checkpoint fringe hash {hex} is not base16"))?;
            BlockHash::try_from(bytes.as_slice())
                .map_err(|e| format!("checkpoint fringe hash {hex} is not a hash: {e}"))
        })
        .collect::<Result<_, String>>()?;
    let claimed_root = base16::decode(&certificate.checkpoint.anchor_post_state)
        .ok_or_else(|| "the checkpoint's post-state is not base16".to_string())?;
    let target = FinalizedFringe {
        hashes: fringe_hashes,
        state_hash: rchain_models::block::state_hash::StateHash::try_from(claimed_root.as_slice())
            .map_err(|e| format!("the checkpoint's post-state is not a state root: {e}"))?,
        ancestry: Vec::new(),
    };

    let replay = RecoveryReplay {
        runtime: parts.runtime_manager.clone(),
        shard_id: spec.shard_id.to_string(),
        min_phlo_price: spec.genesis_block_data.min_phlo_price,
        max_number_of_parents: spec.genesis_block_data.max_number_of_parents,
        expected_genesis: None,
        anchor: Some(anchor),
        checkpoint_source: None,
        checkpoint_store: None,
    };
    let blocks: usize = heights.values().map(|set| set.len()).sum();
    let report = match replay.verify(&store, &heights, &target, log).await {
        Ok(verified_replay) => AuditReport {
            verdict: "AGREE".to_string(),
            checkpoint_digest: verified.digest.to_hex(),
            anchor: anchor.to_hex(),
            anchor_height: certificate.checkpoint.anchor_height,
            shard_id: certificate.checkpoint.shard_id.clone(),
            claimed_root: certificate.checkpoint.anchor_post_state.clone(),
            metadata_root: certificate.checkpoint.metadata_root.clone(),
            history_root: certificate.checkpoint.history_root.clone(),
            // **The root the replay derived**, not a copy of the claim it was compared with.
            derived_root: Some(base16::encode(verified_replay.derived_root().as_bytes())),
            replay_head: replay_head.clone(),
            blocks,
            metadata_committed,
            window_committed,
            reason: None,
        },
        Err(reason) => AuditReport {
            verdict: "DISAGREE".to_string(),
            checkpoint_digest: verified.digest.to_hex(),
            anchor: anchor.to_hex(),
            anchor_height: certificate.checkpoint.anchor_height,
            shard_id: certificate.checkpoint.shard_id.clone(),
            claimed_root: certificate.checkpoint.anchor_post_state.clone(),
            metadata_root: certificate.checkpoint.metadata_root.clone(),
            history_root: certificate.checkpoint.history_root.clone(),
            derived_root: None,
            replay_head: replay_head.clone(),
            blocks,
            metadata_committed,
            window_committed,
            reason: Some(reason),
        },
    };
    write_report(&request.out, &report).await?;
    log.info(
        source,
        &format!(
            "audit {}: checkpoint {} at anchor {} ({} bodies)",
            report.verdict, report.checkpoint_digest, report.anchor, report.blocks
        ),
    );
    Ok(report)
}

/// The result of an attested audit: the report, and the signed claim when the verdict is `AGREE`.
#[derive(Clone, Debug)]
pub struct AuditOutcome {
    pub report: AuditReport,
    pub attestation: Option<AuditAttestation>,
}

/// **Sign the claim an `AGREE` report supports** (Law 112). A legacy certificate has no
/// commitments to attest and a non-AGREE verdict is not a claim, so both are refused here rather
/// than signed into an unfalsifiable statement.
pub fn attest_report(
    report: &AuditReport,
    signer: &ValidatorIdentity,
    nonce: &str,
    previous: Option<String>,
    sequence: u64,
) -> Result<AuditAttestation, String> {
    if report.verdict != "AGREE" {
        return Err(format!(
            "only an AGREE report is attested; this one is {}",
            report.verdict
        ));
    }
    let metadata_root = report
        .metadata_root
        .clone()
        .ok_or_else(|| "a legacy certificate has no metadata commitment to attest".to_string())?;
    let history_root = report.history_root.clone().ok_or_else(|| {
        "a legacy certificate has no validation-window commitment to attest".to_string()
    })?;
    let derived_root = report
        .derived_root
        .clone()
        .ok_or_else(|| "an AGREE report must carry the root the replay derived".to_string())?;
    let replay_head = report
        .replay_head
        .clone()
        .ok_or_else(|| "an AGREE report must carry its replay head".to_string())?;
    let mut attestation = AuditAttestation {
        witness_type: SIGNED_HASH_CHAIN_WITNESS.to_string(),
        shard_id: report.shard_id.clone(),
        checkpoint_digest: report.checkpoint_digest.clone(),
        anchor: report.anchor.clone(),
        anchor_height: report.anchor_height,
        from_height: 0,
        to_height: report.anchor_height,
        metadata_root,
        history_root,
        derived_root,
        replay_head,
        previous,
        sequence,
        nonce: nonce.to_string(),
        signer: base16::encode(signer.public_key.bytes()),
        sig_algorithm: signer.sig_algorithm.clone(),
        sig: String::new(),
    };
    let prehash = attestation.prehash().map_err(|e| e.to_string())?;
    let signature = signer.attest_audit(&prehash).map_err(|e| e.to_string())?;
    attestation.sig = base16::encode(&signature);
    Ok(attestation)
}

/// **Run the audit and, on `AGREE`, sign the claim.** This is the attested entry point; `run`
/// remains the plain report.
pub async fn run_attested(
    conf: &NodeConf,
    request: &AuditRequest,
    signer: &ValidatorIdentity,
    nonce: &str,
    previous: Option<String>,
    sequence: u64,
    log: &Arc<dyn Log>,
) -> Result<AuditOutcome, String> {
    let report = run(conf, request, log).await?;
    let attestation = if report.verdict == "AGREE" {
        Some(attest_report(&report, signer, nonce, previous, sequence)?)
    } else {
        None
    };
    Ok(AuditOutcome {
        report,
        attestation,
    })
}

/// **Verify an attestation against a report** — the offline checker's rule. The signature, shape
/// and every reproduced fact must match; the chain head is not checked here because the checker
/// has no accepted head to extend (a node's store does that check).
pub fn verify_against_report(
    attestation: &AuditAttestation,
    report: &AuditReport,
) -> Result<rchain_casper::audit::VerifiedAuditAttestation, String> {
    let expectation = AuditExpectation {
        checkpoint_digest: report.checkpoint_digest.clone(),
        anchor: report.anchor.clone(),
        anchor_height: report.anchor_height,
        metadata_root: report
            .metadata_root
            .clone()
            .ok_or_else(|| "the report names no metadata commitment".to_string())?,
        history_root: report
            .history_root
            .clone()
            .ok_or_else(|| "the report names no validation-window commitment".to_string())?,
        derived_root: report
            .derived_root
            .clone()
            .ok_or_else(|| "the report names no derived root".to_string())?,
        replay_head: report
            .replay_head
            .clone()
            .ok_or_else(|| "the report names no replay head".to_string())?,
    };
    verify_audit_attestation(attestation, &expectation, None).map_err(|e| e.to_string())
}

fn incomplete(
    verified: &rchain_casper::checkpoint::VerifiedCheckpoint,
    height: i64,
    reason: String,
) -> AuditReport {
    AuditReport {
        verdict: "INCOMPLETE".to_string(),
        checkpoint_digest: verified.digest.to_hex(),
        anchor: verified.checkpoint.anchor.clone(),
        anchor_height: verified.checkpoint.anchor_height,
        shard_id: verified.checkpoint.shard_id.clone(),
        claimed_root: verified.checkpoint.anchor_post_state.clone(),
        metadata_root: verified.checkpoint.metadata_root.clone(),
        history_root: verified.checkpoint.history_root.clone(),
        derived_root: None,
        replay_head: None,
        blocks: height.max(0) as usize,
        metadata_committed: verified.checkpoint.metadata_root.is_some(),
        window_committed: verified.checkpoint.history_root.is_some(),
        reason: Some(reason),
    }
}

async fn write_report(out: &std::path::Path, report: &AuditReport) -> Result<(), String> {
    tokio::fs::create_dir_all(out)
        .await
        .map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let bytes = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    let path = out.join("audit.json");
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// GET a JSON body with a 64 MiB cap. A body page of light infos is small; the cap is a bound on a
/// hostile peer, not a page-size policy.
async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    const MAX_AUDIT_BODY: usize = 64 * 1024 * 1024;
    let response = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("status {}", response.status()));
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > MAX_AUDIT_BODY {
            return Err(format!("the response exceeds {MAX_AUDIT_BODY} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|e| format!("not the expected JSON: {e}"))
}

/// A discovery with no peers: the audit reads over HTTP and never joins the network.
struct StubDiscovery;

#[async_trait::async_trait]
impl NodeDiscovery for StubDiscovery {
    async fn discover(&self) {}
    fn peers(&self) -> Vec<rchain_comm::peer_node::PeerNode> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rchain_casper::proto_util::hash_block;
    use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
    use rchain_models::casper::protocol::casper_message::{BlockMessage, RholangState};

    fn body(hash_byte: u8, height: i64) -> BlockMessage {
        BlockMessage {
            version: 1,
            shard_id: "root".to_string(),
            block_hash: BlockHash::new([hash_byte; 32]),
            block_number: height.try_into().unwrap(),
            sender: rchain_models::validator::Validator::new([0u8; 65]),
            seq_num: height.try_into().unwrap(),
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            justifications: Vec::new(),
            bonds: std::collections::BTreeMap::new(),
            rejected_deploys: BTreeSet::new(),
            rejected_blocks: BTreeSet::new(),
            rejected_senders: BTreeSet::new(),
            state: RholangState::default(),
            sig_algorithm: "secp256k1".to_string(),
            sig: vec![1u8; 64],
            timestamp: 0,
        }
    }

    /// **The reconstruction is checkable**: the raw response's encoded preimage plus its hash and
    /// signature are the block, and a preimage that does not hash to the claimed hash is refused.
    #[test]
    fn a_raw_block_reconstructs_and_a_tampered_preimage_is_refused() {
        let mut b = body(1, 0);
        b.block_hash = hash_block(&b);
        let raw = RawBlock {
            block_hash: base16::encode(b.block_hash.as_bytes()),
            encoded: base16::encode(&b.to_bytes_cleared()),
            sender: base16::encode(b.sender.as_bytes()),
            sig_algorithm: b.sig_algorithm.clone(),
            sig: base16::encode(&b.sig),
        };
        let rebuilt = reconstruct_block(&raw).expect("the raw response rebuilds the block");
        assert_eq!(rebuilt.block_hash, b.block_hash);
        assert_eq!(rebuilt.sig, b.sig);

        // Tamper with a field the hash covers (the height); changing only `block_hash` would not
        // change the preimage, because the hash field itself is cleared before hashing.
        let mut tampered = raw.clone();
        tampered.encoded = base16::encode(&body(1, 5).to_bytes_cleared());
        assert!(
            reconstruct_block(&tampered).is_err(),
            "a preimage that hashes elsewhere is refused"
        );
    }

    #[test]
    fn an_audit_covers_every_height_including_genesis() {
        assert_eq!(required_heights(3).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        assert_eq!(required_heights(0).collect::<Vec<_>>(), vec![0]);
    }

    /// **The audit's body reads are batched, and the batches tile the chain exactly.** A mature
    /// audit asks for 64-height windows; every height is covered once and none twice, so the range
    /// fast path and the per-height fallback see the same heights.
    #[test]
    fn the_audit_batches_body_reads_in_bounded_height_windows() {
        assert_eq!(audit_batches(0), vec![(0, 0)]);
        assert_eq!(audit_batches(1), vec![(0, 1)]);
        assert_eq!(
            audit_batches(63),
            vec![(0, 63)],
            "one full window below the cap"
        );
        assert_eq!(audit_batches(64), vec![(0, 63), (64, 64)]);
        assert_eq!(audit_batches(150), vec![(0, 63), (64, 127), (128, 150)]);
        // The batches tile 0..=last exactly.
        let last = 2_798;
        let covered: Vec<i64> = audit_batches(last)
            .into_iter()
            .flat_map(|(from, to)| from..=to)
            .collect();
        assert_eq!(covered, (0..=last).collect::<Vec<_>>());
        assert!(audit_batches(last)
            .iter()
            .all(|(from, to)| to - from < MAX_AUDIT_BODY_BATCH));
    }

    /// The digest the report names is the certificate's, not the anchor's.
    #[test]
    fn a_report_names_the_checkpoint_digest_and_its_anchor() {
        let digest = Blake2b256Hash::from_byte_array(&[7u8; 32]);
        let verified = rchain_casper::checkpoint::VerifiedCheckpoint {
            checkpoint: rchain_models::casper::protocol::checkpoint::Checkpoint {
                shard_id: "root".to_string(),
                anchor: base16::encode(&[1u8; 32]),
                anchor_height: 9,
                anchor_post_state: base16::encode(&[2u8; 32]),
                fringe_hashes: vec![base16::encode(&[1u8; 32])],
                parent_digest: None,
                epoch: 0,
                metadata_root: None,
                history_root: None,
            },
            digest,
            supporting_stake: 3,
            total_stake: 3,
        };
        let report = incomplete(&verified, 4, "height 4: no block".to_string());
        assert_eq!(report.verdict, "INCOMPLETE");
        assert_eq!(report.checkpoint_digest, digest.to_hex());
        assert_eq!(report.anchor_height, 9);
        assert_eq!(report.blocks, 4);
    }

    fn header(
        height: i64,
        hash_byte: u8,
        parent: Option<BlockHash>,
    ) -> rchain_models::casper::protocol::casper_message::BlockHeader {
        rchain_models::casper::protocol::casper_message::BlockHeader {
            block_hash: BlockHash::new([hash_byte; 32]),
            block_num: rchain_shared::refined::BlockHeight::try_from(height).expect("height"),
            sender: rchain_models::validator::Validator::new([0u8; 65]),
            seq_num: rchain_shared::refined::SeqNum::try_from(0).expect("seq"),
            justifications: parent.into_iter().collect(),
            bonds_map: BTreeMap::new(),
            fringe: BTreeSet::new(),
            fringe_state_hash: rchain_models::block::state_hash::StateHash::new([0u8; 32]),
            pre_state_hash: rchain_models::block::state_hash::StateHash::new([1u8; 32]),
            post_state_hash: rchain_models::block::state_hash::StateHash::new([2u8; 32]),
            body_available: true,
            sig: Vec::new(),
            sig_algorithm: String::new(),
        }
    }

    /// **The certified metadata set is the anchor's ancestor closure, not every body at a height.**
    /// A sibling at a certified height is not in the set; including it would make a correct
    /// certificate disagree. A missing ancestor is INCOMPLETE evidence, not a disagreement.
    #[test]
    fn the_audit_closure_ignores_a_sibling_at_a_certified_height() {
        let genesis = header(0, 1, None);
        let child = header(1, 2, Some(genesis.block_hash));
        let sibling = header(1, 3, Some(genesis.block_hash));
        let headers = vec![genesis.clone(), child.clone(), sibling.clone()];
        let closure = ancestor_closure(child.block_hash, &headers).expect("closure");
        let hashes: BTreeSet<_> = closure.iter().map(|header| header.block_hash).collect();
        assert_eq!(
            hashes,
            BTreeSet::from([genesis.block_hash, child.block_hash])
        );
        assert!(!hashes.contains(&sibling.block_hash));
        assert_eq!(
            ancestor_closure(sibling.block_hash, &headers[..2]),
            Err(sibling.block_hash),
            "a missing ancestor is INCOMPLETE evidence, not a disagreement"
        );
    }
}
