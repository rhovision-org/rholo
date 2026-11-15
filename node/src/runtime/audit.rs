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
use rchain_casper::engine::recovery::RecoveryReplay;
use rchain_casper::interpreter_util::ShardHealth;
use rchain_casper::proto_util::hash_block;
use rchain_casper::protocol::comm_util::ConnectionsCell;
use rchain_comm::discovery::node_discovery::NodeDiscovery;
use rchain_models::block_hash::BlockHash;
use rchain_models::casper::protocol::casper_message::{BlockMessage, FinalizedFringe};
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
    /// The state root the certificate claimed.
    pub claimed_root: String,
    /// The root the replay derived, when it ran far enough to have one.
    pub derived_root: Option<String>,
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

/// Reconstruct the full block from a node's `RawBlock` response: the encoded preimage has the hash
/// zeroed and the signature emptied by construction (`to_bytes_cleared`), so both are restored from
/// the response's separate fields. The result is the block `hash_block` was taken over.
pub fn reconstruct_block(raw: &RawBlock) -> Result<BlockMessage, String> {
    let encoded = base16::decode(&raw.encoded)
        .ok_or_else(|| "the raw block's `encoded` is not base16".to_string())?;
    let mut block = BlockMessage::from_bytes(&encoded)
        .map_err(|e| format!("the raw preimage does not decode as a block: {e}"))?;
    let hash = base16::decode(&raw.block_hash)
        .ok_or_else(|| "the raw block's `blockHash` is not base16".to_string())?;
    block.block_hash = BlockHash::try_from(hash.as_slice())
        .map_err(|e| format!("the raw block's `blockHash` is not a hash: {e}"))?;
    block.sig = base16::decode(&raw.sig)
        .ok_or_else(|| "the raw block's `sig` is not base16".to_string())?;
    block.sig_algorithm = raw.sig_algorithm.clone();
    if hash_block(&block) != block.block_hash {
        return Err(format!(
            "the reconstructed body does not hash to {}",
            block.block_hash.to_hex()
        ));
    }
    Ok(block)
}

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
    let mut headers: Vec<rchain_models::casper::protocol::casper_message::BlockHeader> = Vec::new();
    for height in required_heights(certificate.checkpoint.anchor_height) {
        let infos: Vec<LightBlockInfo> =
            match get_json(&client, &format!("{base}/api/v1/blocks/{height}/{height}")).await {
                Ok(infos) => infos,
                Err(e) => {
                    return Ok(incomplete(
                        &verified,
                        height,
                        format!("height {height}: {e}"),
                    ))
                }
            };
        let mut at_height = BTreeSet::new();
        for info in infos {
            let raw: RawBlock = match get_json(
                &client,
                &format!("{base}/api/v1/block/{}/raw", info.block_hash),
            )
            .await
            {
                Ok(raw) => raw,
                Err(e) => {
                    return Ok(incomplete(
                        &verified,
                        height,
                        format!("body {}: {e}", info.block_hash),
                    ))
                }
            };
            let block = match reconstruct_block(&raw) {
                Ok(block) => block,
                Err(e) => return Ok(incomplete(&verified, height, e)),
            };
            if i64::from(block.block_number) != height {
                return Ok(incomplete(
                    &verified,
                    height,
                    format!(
                        "body {} is at height {}",
                        block.block_hash.to_hex(),
                        block.block_number
                    ),
                ));
            }
            headers.push(
                rchain_models::casper::protocol::casper_message::BlockHeader {
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
                },
            );
            store
                .put(&[(block.block_hash, block.clone())])
                .await
                .map_err(|e| format!("the staging block store refused a body: {e}"))?;
            at_height.insert(block.block_hash);
        }
        if at_height.is_empty() {
            return Ok(incomplete(
                &verified,
                height,
                format!("no block at height {height}"),
            ));
        }
        heights.insert(height, at_height);
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
                    claimed_root: certificate.checkpoint.anchor_post_state.clone(),
                    derived_root: None,
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
                    claimed_root: certificate.checkpoint.anchor_post_state.clone(),
                    derived_root: None,
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
        Ok(_) => AuditReport {
            verdict: "AGREE".to_string(),
            checkpoint_digest: verified.digest.to_hex(),
            anchor: anchor.to_hex(),
            anchor_height: certificate.checkpoint.anchor_height,
            claimed_root: certificate.checkpoint.anchor_post_state.clone(),
            derived_root: Some(certificate.checkpoint.anchor_post_state.clone()),
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
            claimed_root: certificate.checkpoint.anchor_post_state.clone(),
            derived_root: None,
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
        claimed_root: verified.checkpoint.anchor_post_state.clone(),
        derived_root: None,
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
    use rchain_crypto::hash::blake2b256_hash::Blake2b256Hash;
    use rchain_models::casper::protocol::casper_message::RholangState;

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
